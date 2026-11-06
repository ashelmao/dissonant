//! A real session over real connections: the session logic and the network together.

use dissonant_core::{Command, Document, NoteEvent, ProjectModel};
use dissonant_net::{new_secret, Invite, Net, NetEvent, Reach, HOST};
use dissonant_session::{GuestSession, HostSession, ToGuest, ToHost};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

struct Host {
    net: Net,
    rx: Receiver<NetEvent>,
    session: HostSession,
    doc: Document,
}

struct Guest {
    net: Net,
    rx: Receiver<NetEvent>,
    session: GuestSession,
    doc: Document,
}

impl Host {
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            let (conn, message) = match event {
                NetEvent::Message(conn, bytes) => (conn, serde_json::from_slice::<ToHost>(&bytes).unwrap()),
                _ => continue,
            };
            let out = self.session.on_message(&mut self.doc, conn, message);
            for (conn, message) in out.send {
                self.net.send(conn, &serde_json::to_vec(&message).unwrap());
            }
        }
    }
}

impl Guest {
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                NetEvent::Connected(_) => {
                    self.net.send(HOST, &serde_json::to_vec(&self.session.hello()).unwrap());
                }
                NetEvent::Message(_, bytes) => {
                    let message: ToGuest = serde_json::from_slice(&bytes).unwrap();
                    let out = self.session.on_message(&mut self.doc, message);
                    assert_eq!(out.ended, None);
                    for message in out.send {
                        self.net.send(HOST, &serde_json::to_vec(&message).unwrap());
                    }
                }
                _ => {}
            }
        }
    }
}

fn wait(host: &mut Host, guest: &mut Guest, what: &str, done: impl Fn(&Host, &Guest) -> bool) -> Duration {
    let started = Instant::now();
    while !done(host, guest) {
        assert!(started.elapsed() < Duration::from_secs(45), "gave up waiting: {what}");
        host.pump();
        guest.pump();
        std::thread::sleep(Duration::from_millis(2));
    }
    started.elapsed()
}

fn session(reach: Reach) {
    let (tx, rx) = channel();
    let (net, addr) = Net::host(reach, tx).unwrap();
    let secret = new_secret();
    let invite = Invite::parse(&Invite { addr, secret: secret.clone() }.to_code()).unwrap();
    let mut model = ProjectModel::empty();
    model.tempo = 133.0;
    let mut host = Host {
        net,
        rx,
        session: HostSession::new("host".into(), "#fff".into(), secret),
        doc: Document::new(model),
    };
    let (tx, rx) = channel();
    let mut guest = Guest {
        net: Net::join(reach, invite.addr, tx).unwrap(),
        rx,
        session: GuestSession::new(invite.secret, "guest".into(), "#000".into()),
        doc: Document::new(ProjectModel::empty()),
    };

    let joined = wait(&mut host, &mut guest, "the guest to join", |_, g| g.session.joined());
    assert_eq!(guest.doc.model().tempo, 133.0, "the guest has the host's project");
    println!("joined in {joined:?}, path: {:?}", guest.net.path(HOST));

    // A guest's note reaches the host.
    let pattern = guest.doc.model().patterns[0].id;
    let note = NoteEvent::new(0, 240, 64);
    let id = note.id;
    let command = Command::SetNotes { pattern_id: pattern, notes: vec![note] };
    let op = guest.session.edit(&mut guest.doc, command, false, None).unwrap().unwrap();
    guest.net.send(HOST, &serde_json::to_vec(&op).unwrap());
    let took = wait(&mut host, &mut guest, "the note to reach the host", |h, _| {
        h.doc.model().patterns[0].notes.iter().any(|n| n.id == id)
    });
    println!("guest edit reached the host in {took:?}");

    // The host's edit reaches the guest.
    let out = host.session.edit(&mut host.doc, Command::SetTempo { bpm: 90.0 }, false, None).unwrap();
    for (conn, message) in out.send {
        host.net.send(conn, &serde_json::to_vec(&message).unwrap());
    }
    let took = wait(&mut host, &mut guest, "the tempo to reach the guest", |_, g| g.doc.model().tempo == 90.0);
    println!("host edit reached the guest in {took:?}, path: {:?}", guest.net.path(HOST));

    host.net.shutdown();
    guest.net.shutdown();
}

#[test]
fn a_session_works_over_real_connections() {
    session(Reach::Direct);
}

/// Goes out through the public relays, so it needs the internet. Run it with
/// `cargo test -p dissonant-net --test session -- --ignored --nocapture`.
#[test]
#[ignore]
fn a_session_works_across_the_internet() {
    session(Reach::Public);
}

/// The fallback for two people whose routers won't let them connect directly. Needs the internet;
/// run it as above.
#[test]
#[ignore]
fn a_session_works_through_a_relay_alone() {
    session(Reach::RelayOnly);
}
