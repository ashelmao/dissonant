//! `dissonant-net` — moves bytes between the host of a session and its guests.
//!
//! Each guest holds one iroh connection to the host. On it there is one ordered, reliable stream
//! for edits, and unreliable datagrams for cursors, where a late update is worth nothing. iroh
//! dials by public key, connects directly where it can and falls back to a relay where it can't,
//! so nobody has to forward a port, and everything is end-to-end encrypted either way.
//!
//! This crate knows nothing about what the bytes mean. Everything arrives as a [`NetEvent`] on a
//! plain channel, so the caller needs no async of its own.

use data_encoding::BASE64URL_NOPAD;
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tokio::sync::mpsc as tokio_mpsc;

const ALPN: &[u8] = b"dissonant/session/1";
/// A whole project with plugin state can be large. Past this, something has gone wrong.
const MAX_FRAME: usize = 64 * 1024 * 1024;
/// The prefix on an invite, so a pasted scrap of something else can be told from a bad invite.
const INVITE_PREFIX: &str = "dissonant-";
/// How long to wait for a relay before going on with what we have. Without one only people on
/// the same network can connect, which is still worth offering.
const ONLINE_WAIT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a guest keeps trying after losing the host.
const RECONNECT_WINDOW: Duration = Duration::from_secs(60);
const RECONNECT_PAUSE: Duration = Duration::from_secs(2);

/// A connection, numbered by this end. A guest's one connection is [`HOST`].
pub type ConnId = u64;
pub const HOST: ConnId = 0;

#[derive(Debug, Clone, PartialEq)]
pub enum NetEvent {
    Connected(ConnId),
    Message(ConnId, Vec<u8>),
    Datagram(ConnId, Vec<u8>),
    Disconnected(ConnId),
    /// A guest stopped trying to reach the host.
    GaveUp(String),
}

/// Which networks to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// The internet, through n0's public relays.
    Public,
    /// Addresses in the invite only, no relay. For tests.
    Direct,
    /// Through a relay and never directly: what two people behind hostile routers end up with.
    /// For tests.
    RelayOnly,
}

/// What a guest needs to find the host and be let in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    pub addr: EndpointAddr,
    pub secret: String,
}

impl Invite {
    /// The invite as one line of text to paste into a chat.
    pub fn to_code(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("{INVITE_PREFIX}{}", BASE64URL_NOPAD.encode(&json))
    }

    pub fn parse(code: &str) -> Result<Invite, String> {
        let bad = || "that doesn't look like a dissonant invite.".to_string();
        let body = code.trim().strip_prefix(INVITE_PREFIX).ok_or_else(bad)?;
        let json = BASE64URL_NOPAD.decode(body.as_bytes()).map_err(|_| bad())?;
        serde_json::from_slice(&json).map_err(|_| bad())
    }
}

/// A random secret for an invite.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&rand_u64().to_le_bytes());
    bytes[8..].copy_from_slice(&rand_u64().to_le_bytes());
    BASE64URL_NOPAD.encode(&bytes)
}

fn rand_u64() -> u64 {
    // iroh's own secret key generation is a CSPRNG; borrowing its output keeps a second random
    // number crate out of the build.
    let key = iroh::SecretKey::generate();
    let bytes = key.to_bytes();
    u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
}

/// What the writer task for a connection is told to do.
enum Out {
    Frame(Vec<u8>),
    /// Everything queued has gone: close.
    Close,
}

#[derive(Clone)]
struct Link {
    out: tokio_mpsc::UnboundedSender<Out>,
    conn: Connection,
}

/// A running network end. Dropping it without [`Net::shutdown`] cuts connections abruptly.
pub struct Net {
    runtime: Runtime,
    endpoint: Endpoint,
    links: Arc<Mutex<HashMap<ConnId, Link>>>,
    stopping: Arc<AtomicBool>,
}

fn runtime() -> Result<Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("dissonant-net")
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

async fn bind(reach: Reach, alpns: Vec<Vec<u8>>) -> Result<Endpoint, String> {
    let endpoint = match reach {
        Reach::Public => Endpoint::builder(presets::N0).alpns(alpns).bind().await,
        Reach::Direct => Endpoint::builder(presets::N0DisableRelay).alpns(alpns).bind().await,
        Reach::RelayOnly => Endpoint::builder(presets::N0).clear_ip_transports().alpns(alpns).bind().await,
    };
    let endpoint = endpoint.map_err(|e| format!("could not open the network: {e}"))?;
    if reach != Reach::Direct && tokio::time::timeout(ONLINE_WAIT, endpoint.online()).await.is_err() {
        log::warn!("no relay reached: only people on this network will be able to connect");
    }
    Ok(endpoint)
}

fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 4);
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

async fn read_frame(recv: &mut RecvStream) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await.ok()?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        log::warn!("dropping a connection over a {len}-byte message");
        return None;
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await.ok()?;
    Some(buf)
}

/// Run one connection until it ends: read frames and datagrams into `events`, write what arrives
/// on `out`. Returns when the connection is gone.
async fn serve(
    id: ConnId,
    conn: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    mut out: tokio_mpsc::UnboundedReceiver<Out>,
    events: mpsc::Sender<NetEvent>,
) {
    let writer = {
        let conn = conn.clone();
        async move {
            while let Some(item) = out.recv().await {
                match item {
                    Out::Frame(bytes) => {
                        if send.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    Out::Close => {
                        let _ = send.finish();
                        // Give the far end a moment to read what was just written.
                        let _ = tokio::time::timeout(Duration::from_secs(1), send.stopped()).await;
                        conn.close(0u32.into(), b"bye");
                        break;
                    }
                }
            }
        }
    };
    let reader = {
        let events = events.clone();
        async move {
            while let Some(bytes) = read_frame(&mut recv).await {
                if events.send(NetEvent::Message(id, bytes)).is_err() {
                    break;
                }
            }
        }
    };
    let datagrams = {
        let conn = conn.clone();
        let events = events.clone();
        async move {
            while let Ok(bytes) = conn.read_datagram().await {
                if events.send(NetEvent::Datagram(id, bytes.to_vec())).is_err() {
                    break;
                }
            }
        }
    };
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
        _ = datagrams => {}
    }
}

impl Net {
    /// Start hosting. Returns the running end and the address to put in an invite.
    pub fn host(reach: Reach, events: mpsc::Sender<NetEvent>) -> Result<(Net, EndpointAddr), String> {
        let runtime = runtime()?;
        let endpoint = runtime.block_on(bind(reach, vec![ALPN.to_vec()]))?;
        let addr = endpoint.addr();
        let links: Arc<Mutex<HashMap<ConnId, Link>>> = Arc::default();
        let stopping = Arc::new(AtomicBool::new(false));
        runtime.spawn(accept_loop(endpoint.clone(), links.clone(), events));
        Ok((Net { runtime, endpoint, links, stopping }, addr))
    }

    /// Connect to a host, and keep reconnecting for a while if the connection drops.
    pub fn join(reach: Reach, addr: EndpointAddr, events: mpsc::Sender<NetEvent>) -> Result<Net, String> {
        let runtime = runtime()?;
        let endpoint = runtime.block_on(bind(reach, vec![]))?;
        let links: Arc<Mutex<HashMap<ConnId, Link>>> = Arc::default();
        let stopping = Arc::new(AtomicBool::new(false));
        runtime.spawn(dial_loop(endpoint.clone(), addr, links.clone(), stopping.clone(), events));
        Ok(Net { runtime, endpoint, links, stopping })
    }

    /// Send a message on a connection, in order, reliably.
    pub fn send(&self, conn: ConnId, bytes: &[u8]) {
        if let Some(link) = self.links.lock().unwrap().get(&conn) {
            let _ = link.out.send(Out::Frame(frame(bytes)));
        }
    }

    /// Send a datagram: it may arrive late, out of order, or not at all.
    pub fn datagram(&self, conn: ConnId, bytes: &[u8]) {
        if let Some(link) = self.links.lock().unwrap().get(&conn) {
            let _ = link.conn.send_datagram(bytes.to_vec().into());
        }
    }

    /// Close a connection once what is queued for it has gone out.
    pub fn close(&self, conn: ConnId) {
        if let Some(link) = self.links.lock().unwrap().remove(&conn) {
            let _ = link.out.send(Out::Close);
        }
    }

    /// How this machine is reaching the host right now, for the person: "direct" or "relayed".
    pub fn path(&self, conn: ConnId) -> Option<&'static str> {
        let links = self.links.lock().unwrap();
        let link = links.get(&conn)?;
        let paths = link.conn.paths();
        let selected = paths.iter().find(|p| p.is_selected())?;
        Some(if selected.is_relay() { "relayed" } else { "direct" })
    }

    /// Say goodbye on every connection and stop. Blocks for up to about two seconds, so keep it
    /// off any thread somebody is waiting on.
    pub fn shutdown(self) {
        self.stopping.store(true, Ordering::SeqCst);
        for (_, link) in self.links.lock().unwrap().drain() {
            let _ = link.out.send(Out::Close);
        }
        let endpoint = self.endpoint.clone();
        self.runtime.block_on(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = tokio::time::timeout(Duration::from_millis(1500), endpoint.close()).await;
        });
        self.runtime.shutdown_background();
    }
}

async fn accept_loop(endpoint: Endpoint, links: Arc<Mutex<HashMap<ConnId, Link>>>, events: mpsc::Sender<NetEvent>) {
    let next = Arc::new(AtomicU64::new(HOST + 1));
    while let Some(incoming) = endpoint.accept().await {
        let links = links.clone();
        let events = events.clone();
        let next = next.clone();
        tokio::spawn(async move {
            let Ok(accepting) = incoming.accept() else { return };
            let Ok(conn) = accepting.await else { return };
            let Ok((send, recv)) = conn.accept_bi().await else { return };
            let id = next.fetch_add(1, Ordering::SeqCst);
            let (out, rx) = tokio_mpsc::unbounded_channel();
            links.lock().unwrap().insert(id, Link { out, conn: conn.clone() });
            let _ = events.send(NetEvent::Connected(id));
            serve(id, conn, send, recv, rx, events.clone()).await;
            links.lock().unwrap().remove(&id);
            let _ = events.send(NetEvent::Disconnected(id));
        });
    }
}

async fn dial(endpoint: &Endpoint, addr: &EndpointAddr) -> Result<(Connection, SendStream, RecvStream), String> {
    let conn = tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(addr.clone(), ALPN))
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|e| e.to_string())?;
    let (send, recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
    Ok((conn, send, recv))
}

async fn dial_loop(
    endpoint: Endpoint,
    addr: EndpointAddr,
    links: Arc<Mutex<HashMap<ConnId, Link>>>,
    stopping: Arc<AtomicBool>,
    events: mpsc::Sender<NetEvent>,
) {
    let mut trying_since = Instant::now();
    let mut ever_connected = false;
    while !stopping.load(Ordering::SeqCst) {
        match dial(&endpoint, &addr).await {
            Ok((conn, send, recv)) => {
                ever_connected = true;
                let (out, rx) = tokio_mpsc::unbounded_channel();
                links.lock().unwrap().insert(HOST, Link { out, conn: conn.clone() });
                let _ = events.send(NetEvent::Connected(HOST));
                serve(HOST, conn, send, recv, rx, events.clone()).await;
                links.lock().unwrap().remove(&HOST);
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let _ = events.send(NetEvent::Disconnected(HOST));
                trying_since = Instant::now();
            }
            Err(e) => {
                log::info!("could not reach the host: {e}");
                let window = if ever_connected { RECONNECT_WINDOW } else { CONNECT_TIMEOUT };
                if trying_since.elapsed() >= window {
                    let why = if ever_connected {
                        "lost the connection to the host."
                    } else {
                        "couldn't reach the host. check they're still sharing, and that the invite is current."
                    };
                    let _ = events.send(NetEvent::GaveUp(why.to_string()));
                    return;
                }
            }
        }
        tokio::time::sleep(RECONNECT_PAUSE).await;
    }
}
