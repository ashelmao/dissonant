//! A guest's side of a session.

use crate::protocol::{OpBody, PeerInfo, ToGuest, ToHost, PROTOCOL_VERSION};
use dissonant_core::document::apply_command;
use dissonant_core::{ids, Command, Document, EditError, Patch, Personal, ProjectModel, StepOutcome, TimeEdit, SCHEMA_VERSION};
use std::collections::VecDeque;
use uuid::Uuid;

/// An edit the host hasn't answered yet.
struct Pending {
    seq: u64,
    op_id: Uuid,
    body: OpBody,
}

/// What came of handling something.
#[derive(Debug, Default)]
pub struct GuestEvents {
    pub send: Vec<ToHost>,
    /// The project was loaded from scratch: the app has to reset its engine to it.
    pub loaded: bool,
    /// What the person sees changed by this, as far as the host's edit went.
    pub changed: Option<Patch>,
    pub peers: Option<Vec<PeerInfo>>,
    /// Something to tell the person.
    pub notice: Option<String>,
    /// The session is over for this guest, and why.
    pub ended: Option<String>,
}

pub struct GuestSession {
    client: Uuid,
    secret: String,
    name: String,
    color: String,
    /// The project as the host has confirmed it. What the person sees is this plus `pending`.
    confirmed: ProjectModel,
    pending: VecDeque<Pending>,
    next_seq: u64,
    joined: bool,
    /// Between losing the connection and the host's welcome. Edits are held rather than sent,
    /// because the welcome says which ones the host already has: an edit that overtook an older
    /// one here would make the host take the older one for a repeat and drop it.
    holding: bool,
}

impl GuestSession {
    pub fn new(secret: String, name: String, color: String) -> Self {
        GuestSession {
            client: Uuid::new_v4(),
            secret,
            name,
            color,
            confirmed: ProjectModel::empty(),
            pending: VecDeque::new(),
            next_seq: 1,
            joined: false,
            holding: true,
        }
    }

    pub fn id(&self) -> Uuid {
        self.client
    }

    pub fn joined(&self) -> bool {
        self.joined
    }

    /// The first thing to say on every new connection.
    pub fn hello(&self) -> ToHost {
        ToHost::Hello {
            protocol: PROTOCOL_VERSION,
            schema: SCHEMA_VERSION,
            secret: self.secret.clone(),
            client: self.client,
            name: self.name.clone(),
            color: self.color.clone(),
        }
    }

    /// The connection dropped. Work carries on, and goes out when the host is back.
    pub fn disconnected(&mut self) {
        self.holding = true;
    }

    fn queue(&mut self, op_id: Uuid, body: OpBody) -> Option<ToHost> {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.pending.push_back(Pending { seq, op_id, body: body.clone() });
        (!self.holding).then_some(ToHost::Op { seq, op_id, body })
    }

    /// The person's own edit: shown at once, and sent if it isn't theirs alone.
    pub fn edit(
        &mut self,
        doc: &mut Document,
        command: Command,
        transient: bool,
        label: Option<String>,
    ) -> Result<Option<ToHost>, EditError> {
        if command.is_personal() {
            doc.apply_labeled(command, transient, label)?;
            return Ok(None);
        }
        let op_id = Uuid::new_v4();
        ids::scoped(op_id, || doc.apply_labeled(command.clone(), transient, label))?;
        Ok(self.queue(op_id, OpBody::Command(command)))
    }

    /// The person's own undo or redo.
    pub fn step(&mut self, doc: &mut Document, undo: bool) -> (StepOutcome, Option<ToHost>) {
        let outcome = if undo { doc.undo_step() } else { doc.redo_step() };
        let mut message = None;
        if let StepOutcome::Applied(change) = &outcome {
            let shared = change.without_personal();
            if !shared.is_empty() {
                message = self.queue(Uuid::new_v4(), OpBody::Patch(shared));
            }
        }
        (outcome, message)
    }

    /// Show the confirmed project with everything unanswered replayed on top and this person's
    /// own listening state laid over it.
    fn rebuild(&self, doc: &mut Document, time_edit: Option<TimeEdit>) {
        let mut personal = Personal::of(doc.model());
        if let Some(edit) = time_edit {
            personal.loop_region = edit.shift(personal.loop_region);
            personal.punch_region = edit.shift(personal.punch_region);
        }
        let mut view = self.confirmed.clone();
        for op in &self.pending {
            match &op.body {
                // One that no longer applies is the host's to refuse; until it does, it shows as
                // not having happened.
                OpBody::Command(command) => {
                    let _ = ids::scoped(op.op_id, || apply_command(&mut view, command.clone()));
                }
                OpBody::Patch(patch) => {
                    if let Some(next) = patch.applicable_to(&view).ok().and_then(|p| p.apply(&view).ok()) {
                        view = next;
                        view.mend();
                    }
                }
            }
        }
        personal.overlay(&mut view);
        doc.set_view(view);
    }

    pub fn on_message(&mut self, doc: &mut Document, message: ToGuest) -> GuestEvents {
        let mut out = GuestEvents::default();
        match message {
            ToGuest::Welcome { last_seq, model, peers, .. } => {
                self.confirmed = *model;
                self.holding = false;
                out.peers = Some(peers);
                if self.joined {
                    self.pending.retain(|op| op.seq > last_seq);
                    self.rebuild(doc, None);
                    out.send = self
                        .pending
                        .iter()
                        .map(|op| ToHost::Op { seq: op.seq, op_id: op.op_id, body: op.body.clone() })
                        .collect();
                    out.changed = Some(Patch::default());
                } else {
                    self.joined = true;
                    doc.restore(self.confirmed.clone(), None);
                    out.loaded = true;
                }
            }
            ToGuest::Refused { reason } | ToGuest::Ended { reason } => out.ended = Some(reason),
            ToGuest::Applied { origin, seq, patch, time_edit, .. } => {
                match patch.apply(&self.confirmed) {
                    Ok(next) => self.confirmed = next,
                    // Patches are applied in the order they were made to the state they were
                    // made from, so this can't happen short of a bug. Going on would drift
                    // silently, so end loudly: the person keeps what they see.
                    Err(e) => {
                        out.ended = Some(format!("lost track of the project: {e}"));
                        return out;
                    }
                }
                if origin == self.client {
                    self.pending.retain(|op| op.seq > seq);
                } else {
                    doc.keep_gesture_current(&patch);
                }
                let before = doc.revision();
                self.rebuild(doc, time_edit);
                if doc.revision() != before {
                    out.changed = Some(patch);
                }
            }
            ToGuest::Rejected { seq, reason } => {
                self.pending.retain(|op| op.seq != seq);
                self.rebuild(doc, None);
                out.changed = Some(Patch::default());
                out.notice = Some(format!("that edit didn't go through: {reason}"));
            }
            ToGuest::Peers { peers } => out.peers = Some(peers),
        }
        out
    }

    /// Say goodbye on the way out.
    pub fn bye(&self) -> ToHost {
        ToHost::Bye
    }
}
