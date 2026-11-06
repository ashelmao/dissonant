//! Keeping two people in one pattern from erasing each other.
//!
//! The roll and the chord lane commit whole arrays, built from what they showed when the drag
//! began. If somebody else added a note since, the array doesn't have it, and comparing it with
//! the present would read as deleting their note. So the array is compared with what the editor
//! *started from*, which leaves exactly what this person changed.

use dissonant_core::{Command, ProjectModel};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use uuid::Uuid;

/// How many past states to keep. Only changes to notes or chords use a slot, so a friend dragging
/// a fader doesn't push out the state a long note drag started from.
const KEEP: usize = 64;

/// Recent states of the project, by document revision, for [`narrow`] to compare against.
#[derive(Default)]
pub struct Bases {
    states: VecDeque<(u64, Arc<ProjectModel>)>,
}

impl Bases {
    /// Remember the project as of `revision`, if its notes or chords changed.
    pub fn note(&mut self, revision: u64, model: &ProjectModel) {
        let same = self.states.back().is_some_and(|(_, m)| m.patterns == model.patterns && m.chords == model.chords);
        if same {
            return;
        }
        self.states.push_back((revision, Arc::new(model.clone())));
        if self.states.len() > KEEP {
            self.states.pop_front();
        }
    }

    /// The notes and chords as they stood at `revision`, if that is still remembered.
    pub fn at(&self, revision: u64) -> Option<&ProjectModel> {
        self.states.iter().rev().find(|(r, _)| *r <= revision).map(|(_, m)| m.as_ref())
    }
}

/// Turn a whole-array edit into just what changed since `base`. Anything else passes through.
pub fn narrow(command: Command, base: &ProjectModel) -> Command {
    match command {
        Command::SetNotes { pattern_id, notes } => {
            let Some(pattern) = base.pattern(&pattern_id) else {
                return Command::SetNotes { pattern_id, notes };
            };
            let old: HashMap<Uuid, _> = pattern.notes.iter().map(|n| (n.id, n)).collect();
            let kept: Vec<Uuid> = notes.iter().map(|n| n.id).collect();
            let remove = pattern.notes.iter().map(|n| n.id).filter(|id| !kept.contains(id)).collect();
            let upsert = notes.into_iter().filter(|n| old.get(&n.id).is_none_or(|o| *o != n)).collect();
            Command::PatchNotes { pattern_id, upsert, remove }
        }
        Command::SetChords { chords } => {
            let old: HashMap<Uuid, _> = base.chords.chords().iter().map(|c| (c.id, c)).collect();
            let kept: Vec<Uuid> = chords.iter().map(|c| c.id).collect();
            let remove = base.chords.chords().iter().map(|c| c.id).filter(|id| !kept.contains(id)).collect();
            let upsert = chords.into_iter().filter(|c| old.get(&c.id).is_none_or(|o| *o != c)).collect();
            Command::PatchChords { upsert, remove }
        }
        other => other,
    }
}
