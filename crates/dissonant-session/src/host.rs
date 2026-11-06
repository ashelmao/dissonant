//! The host's side of a session.

use crate::protocol::{OpBody, PeerInfo, ToGuest, ToHost, PROTOCOL_VERSION};
use dissonant_core::{ids, Command, Document, EditError, Patch, StepOutcome, TimeEdit, SCHEMA_VERSION};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A connection, as the network layer numbers them.
pub type ConnId = u64;

/// A session holds this many people, host included.
const MAX_PEOPLE: usize = 8;

struct Guest {
    info: PeerInfo,
    /// The last op seen from this guest. Survives a dropped connection.
    last_seq: u64,
    conn: Option<ConnId>,
}

/// What came of handling something: messages to send, and what the app has to do about it.
#[derive(Debug, Default)]
pub struct HostEvents {
    pub send: Vec<(ConnId, ToGuest)>,
    /// The project changed by this. The app's engine has to catch up.
    pub changed: Option<Patch>,
    pub peers_changed: bool,
    /// A connection to close once `send` has gone out.
    pub close: Option<ConnId>,
}

pub struct HostSession {
    id: Uuid,
    name: String,
    color: String,
    secret: String,
    revision: u64,
    guests: HashMap<Uuid, Guest>,
    /// Removed by the host, so they don't simply come back through the link they still have.
    removed: HashSet<Uuid>,
}

impl HostSession {
    pub fn new(name: String, color: String, secret: String) -> Self {
        HostSession {
            id: Uuid::new_v4(),
            name,
            color,
            secret,
            revision: 0,
            guests: HashMap::new(),
            removed: HashSet::new(),
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// Rotate the invite. People already in stay in.
    pub fn set_secret(&mut self, secret: String) {
        self.secret = secret;
    }

    /// Everyone connected, host first.
    pub fn peers(&self) -> Vec<PeerInfo> {
        let host = PeerInfo { id: self.id, name: self.name.clone(), color: self.color.clone(), is_host: true };
        let mut guests: Vec<PeerInfo> = self.guests.values().filter(|g| g.conn.is_some()).map(|g| g.info.clone()).collect();
        guests.sort_by_key(|p| p.id);
        std::iter::once(host).chain(guests).collect()
    }

    fn conns(&self) -> impl Iterator<Item = ConnId> + '_ {
        self.guests.values().filter_map(|g| g.conn)
    }

    fn guest_on(&self, conn: ConnId) -> Option<Uuid> {
        self.guests.iter().find(|(_, g)| g.conn == Some(conn)).map(|(id, _)| *id)
    }

    /// Whoever is on `conn`, for stamping their cursor.
    pub fn client_on(&self, conn: ConnId) -> Option<Uuid> {
        self.guest_on(conn)
    }

    /// Every open connection except `conn`.
    pub fn others(&self, conn: Option<ConnId>) -> Vec<ConnId> {
        self.conns().filter(|c| Some(*c) != conn).collect()
    }

    fn tell_peers(&self, out: &mut HostEvents) {
        let peers = self.peers();
        out.send.extend(self.conns().map(|c| (c, ToGuest::Peers { peers: peers.clone() })));
        out.peers_changed = true;
    }

    /// Tell everyone about a change. It has to go out even when it's empty if a guest is waiting
    /// on its answer.
    fn publish(&mut self, out: &mut HostEvents, origin: Uuid, seq: u64, patch: Patch, time_edit: Option<TimeEdit>) {
        self.revision += 1;
        let message = ToGuest::Applied { revision: self.revision, origin, seq, patch: patch.clone(), time_edit };
        out.send.extend(self.conns().map(|c| (c, message.clone())));
        if !patch.is_empty() {
            out.changed = Some(patch);
        }
    }

    /// The host's own edit, with history as usual.
    pub fn edit(
        &mut self,
        doc: &mut Document,
        command: Command,
        transient: bool,
        label: Option<String>,
    ) -> Result<HostEvents, EditError> {
        let mut out = HostEvents::default();
        let before = doc.model().clone();
        let time_edit = command.time_edit();
        doc.apply_labeled(command, transient, label)?;
        let patch = Patch::between(&before, doc.model()).without_personal();
        if !patch.is_empty() {
            self.publish(&mut out, self.id, 0, patch, time_edit);
        }
        Ok(out)
    }

    /// The host's own undo or redo. Returns the outcome so the app can say when it was blocked.
    pub fn step(&mut self, doc: &mut Document, undo: bool) -> (StepOutcome, HostEvents) {
        let mut out = HostEvents::default();
        let outcome = if undo { doc.undo_step() } else { doc.redo_step() };
        if let StepOutcome::Applied(change) = &outcome {
            let patch = change.without_personal();
            if !patch.is_empty() {
                self.publish(&mut out, self.id, 0, patch, None);
            }
        }
        (outcome, out)
    }

    pub fn on_message(&mut self, doc: &mut Document, conn: ConnId, message: ToHost) -> HostEvents {
        let mut out = HostEvents::default();
        match message {
            ToHost::Hello { protocol, schema, secret, client, name, color } => {
                self.on_hello(doc, conn, protocol, schema, &secret, client, name, color, &mut out);
            }
            ToHost::Op { seq, op_id, body } => {
                if let Some(client) = self.guest_on(conn) {
                    self.on_op(doc, conn, client, seq, op_id, body, &mut out);
                }
            }
            ToHost::Bye => {
                if let Some(client) = self.guest_on(conn) {
                    self.guests.remove(&client);
                    self.tell_peers(&mut out);
                }
                out.close = Some(conn);
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn on_hello(
        &mut self,
        doc: &Document,
        conn: ConnId,
        protocol: u32,
        schema: u32,
        secret: &str,
        client: Uuid,
        name: String,
        color: String,
        out: &mut HostEvents,
    ) {
        let refuse = if protocol != PROTOCOL_VERSION || schema != SCHEMA_VERSION {
            Some("you and the host are on different versions of dissonant. use the same build.".to_string())
        } else if secret != self.secret && !self.guests.contains_key(&client) {
            // Someone already in the session stays in when the invite changes, and that has to
            // hold through a dropped connection too.
            Some("this invite isn't valid any more. ask the host for a new one.".to_string())
        } else if self.removed.contains(&client) {
            Some("the host removed you from this session.".to_string())
        } else if !self.guests.contains_key(&client) && self.peers().len() >= MAX_PEOPLE {
            Some(format!("this session is full ({MAX_PEOPLE} people)."))
        } else {
            None
        };
        if let Some(reason) = refuse {
            out.send.push((conn, ToGuest::Refused { reason }));
            out.close = Some(conn);
            return;
        }
        let info = PeerInfo { id: client, name, color, is_host: false };
        let guest = self.guests.entry(client).or_insert(Guest { info: info.clone(), last_seq: 0, conn: None });
        guest.info = info;
        guest.conn = Some(conn);
        let last_seq = guest.last_seq;
        // Solo is what the host is listening to right now, not a state of the song: a guest who
        // inherited it would join to one track and no idea why.
        let mut model = doc.model().clone();
        for t in &mut model.tracks {
            t.soloed = false;
        }
        out.send.push((
            conn,
            ToGuest::Welcome { revision: self.revision, last_seq, model: Box::new(model), peers: self.peers() },
        ));
        self.tell_peers(out);
    }

    #[allow(clippy::too_many_arguments)]
    fn on_op(
        &mut self,
        doc: &mut Document,
        conn: ConnId,
        client: Uuid,
        seq: u64,
        op_id: Uuid,
        body: OpBody,
        out: &mut HostEvents,
    ) {
        let Some(guest) = self.guests.get_mut(&client) else { return };
        if seq <= guest.last_seq {
            return;
        }
        guest.last_seq = seq;
        match body {
            OpBody::Command(command) => {
                if command.is_personal() || command.adds_audio() {
                    let reason = "that can't be done from a guest's side".to_string();
                    out.send.push((conn, ToGuest::Rejected { seq, reason }));
                    return;
                }
                let time_edit = command.time_edit();
                match ids::scoped(op_id, || doc.apply_external(command)) {
                    Ok(change) => self.publish(out, client, seq, change.without_personal(), time_edit),
                    Err(e) => out.send.push((conn, ToGuest::Rejected { seq, reason: e.to_string() })),
                }
            }
            OpBody::Patch(patch) => {
                let change = doc.apply_external_patch(&patch.without_personal());
                self.publish(out, client, seq, change.without_personal(), None);
            }
        }
    }

    /// A connection dropped. The guest's place is kept, in case it comes back.
    pub fn on_disconnect(&mut self, conn: ConnId) -> HostEvents {
        let mut out = HostEvents::default();
        if let Some(client) = self.guest_on(conn) {
            if let Some(guest) = self.guests.get_mut(&client) {
                guest.conn = None;
            }
            self.tell_peers(&mut out);
        }
        out
    }

    /// Put someone out of the session for good.
    pub fn remove(&mut self, client: Uuid) -> HostEvents {
        let mut out = HostEvents::default();
        self.removed.insert(client);
        if let Some(guest) = self.guests.remove(&client) {
            if let Some(conn) = guest.conn {
                let reason = "the host removed you from this session.".to_string();
                out.send.push((conn, ToGuest::Ended { reason }));
                out.close = Some(conn);
            }
        }
        self.tell_peers(&mut out);
        out
    }

    /// End the session for everyone.
    pub fn end(&mut self) -> HostEvents {
        let mut out = HostEvents::default();
        let reason = "the host ended the session.".to_string();
        out.send.extend(self.conns().map(|c| (c, ToGuest::Ended { reason: reason.clone() })));
        out
    }
}
