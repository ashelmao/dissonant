//! What host and guests say to each other, as JSON over one ordered, reliable stream per guest.
//! Cursors don't go through here: they are lossy and newest-wins, so they travel separately.

use dissonant_core::{Command, Patch, ProjectModel, TimeEdit};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Bumped on any change to these messages or to [`Command`]. Everyone in a session has to match,
/// because a command that one side can't read would quietly lose someone's edit.
pub const PROTOCOL_VERSION: u32 = 1;

/// Who somebody is, for the presence bar and their cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub id: Uuid,
    pub name: String,
    pub color: String,
    pub is_host: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpBody {
    Command(Command),
    /// An undo or redo: the change it made on the guest, which the host applies as far as it
    /// still holds.
    Patch(Patch),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ToHost {
    Hello {
        protocol: u32,
        schema: u32,
        secret: String,
        /// Stays the same across reconnects, which is how the host knows who is back.
        client: Uuid,
        name: String,
        color: String,
    },
    Op {
        seq: u64,
        /// Seeds the ids the edit mints, so the guest's prediction matches the real thing.
        op_id: Uuid,
        body: OpBody,
    },
    Bye,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ToGuest {
    /// The whole project, on joining and on every reconnect. `last_seq` is the last op of this
    /// guest's the host has seen, so nothing is applied twice.
    Welcome {
        revision: u64,
        last_seq: u64,
        model: Box<ProjectModel>,
        peers: Vec<PeerInfo>,
    },
    /// Not let in, and why, in words for the person.
    Refused { reason: String },
    /// One edit, in order. `origin` and `seq` let a guest recognise its own edit coming back.
    Applied {
        revision: u64,
        origin: Uuid,
        seq: u64,
        patch: Patch,
        time_edit: Option<TimeEdit>,
    },
    /// The host refused an op; the guest's prediction of it is dropped.
    Rejected { seq: u64, reason: String },
    Peers { peers: Vec<PeerInfo> },
    Ended { reason: String },
}
