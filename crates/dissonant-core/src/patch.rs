//! A change to a [`ProjectModel`], as data.
//!
//! A [`Patch`] records what differs between two models, down to the single field, and it records
//! both sides of every change. That one structure does three jobs:
//!
//! - **Undo.** A history entry is a patch; undoing applies it [`reversed`](Patch::reversed).
//! - **Sharing.** The host of a session runs a command and sends everyone the patch it produced,
//!   so guests never have to reproduce ripple edits, clip splits or any other command logic.
//! - **Not trampling other people.** Because a patch knows what it expects to find, a reversed
//!   patch can be [checked against the present](Patch::applicable_to): undo leaves alone anything
//!   somebody else has changed since.
//!
//! The diff works on the model's JSON form, so a new model field is covered with no change here.
//! An array whose elements all carry a unique `id` is compared item by item; any other array is
//! one value.

use crate::model::ProjectModel;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PatchError {
    #[error("the patch does not fit this project: {0}")]
    Malformed(String),
}

/// The patch can't be reversed because something built on what it would take away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("somebody else has built on this")]
pub struct Conflict;

/// The part of a keyed list whose order changed: the ids from `start`, before and after.
/// Everything outside that window kept its place, so adding one note to a long pattern costs one
/// id here rather than the whole list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub start: usize,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Node {
    /// A value replaced. `None` means absent: a field not written, an item not in its list.
    Change { before: Option<Value>, after: Option<Value> },
    /// Changes inside an object, by key.
    Fields(BTreeMap<String, Node>),
    /// Changes to the items of a keyed list, by id.
    Items { items: BTreeMap<String, Node>, order: Option<Order> },
}

/// See the module docs. The empty patch changes nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Patch(BTreeMap<String, Node>);

fn to_json(m: &ProjectModel) -> Map<String, Value> {
    match serde_json::to_value(m) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn from_json(map: Map<String, Value>) -> Result<ProjectModel, PatchError> {
    serde_json::from_value(Value::Object(map)).map_err(|e| PatchError::Malformed(e.to_string()))
}

fn id_of(item: &Value) -> Option<&str> {
    item.as_object()?.get("id")?.as_str()
}

/// The ids of a list that can be compared item by item, or `None` for one that can't.
fn keyed_ids(list: &[Value]) -> Option<Vec<&str>> {
    let ids: Vec<&str> = list.iter().map(id_of).collect::<Option<_>>()?;
    let unique: HashSet<&&str> = ids.iter().collect();
    (unique.len() == ids.len()).then_some(ids)
}

fn diff_objects(a: &Map<String, Value>, b: &Map<String, Value>) -> BTreeMap<String, Node> {
    let mut out = BTreeMap::new();
    for key in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
        let node = match (a.get(key), b.get(key)) {
            (Some(x), Some(y)) => diff_values(x, y),
            (x, y) => Some(Node::Change { before: x.cloned(), after: y.cloned() }),
        };
        if let Some(node) = node {
            out.insert(key.clone(), node);
        }
    }
    out
}

fn diff_lists(a: &[Value], b: &[Value], ids_a: &[&str], ids_b: &[&str]) -> Option<Node> {
    let by_id: HashMap<&str, &Value> = ids_b.iter().copied().zip(b.iter()).collect();
    let mut items = BTreeMap::new();
    for (id, x) in ids_a.iter().zip(a.iter()) {
        let node = match by_id.get(id) {
            Some(y) => diff_values(x, y),
            None => Some(Node::Change { before: Some(x.clone()), after: None }),
        };
        if let Some(node) = node {
            items.insert((*id).to_string(), node);
        }
    }
    let had: HashSet<&str> = ids_a.iter().copied().collect();
    for (id, y) in ids_b.iter().zip(b.iter()) {
        if !had.contains(id) {
            items.insert((*id).to_string(), Node::Change { before: None, after: Some(y.clone()) });
        }
    }
    let order = (ids_a != ids_b).then(|| {
        let start = ids_a.iter().zip(ids_b).take_while(|(x, y)| x == y).count();
        let room = ids_a.len().min(ids_b.len()) - start;
        let tail = ids_a.iter().rev().zip(ids_b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
        let owned = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect();
        Order {
            start,
            before: owned(&ids_a[start..ids_a.len() - tail]),
            after: owned(&ids_b[start..ids_b.len() - tail]),
        }
    });
    (!items.is_empty() || order.is_some()).then_some(Node::Items { items, order })
}

fn diff_values(a: &Value, b: &Value) -> Option<Node> {
    if a == b {
        return None;
    }
    if let (Value::Object(x), Value::Object(y)) = (a, b) {
        return Some(Node::Fields(diff_objects(x, y)));
    }
    if let (Value::Array(x), Value::Array(y)) = (a, b) {
        if let (Some(ids_x), Some(ids_y)) = (keyed_ids(x), keyed_ids(y)) {
            return diff_lists(x, y, &ids_x, &ids_y);
        }
    }
    Some(Node::Change { before: Some(a.clone()), after: Some(b.clone()) })
}

fn reverse_order(o: &Order) -> Order {
    Order { start: o.start, before: o.after.clone(), after: o.before.clone() }
}

fn reverse_node(node: &Node) -> Node {
    match node {
        Node::Change { before, after } => Node::Change { before: after.clone(), after: before.clone() },
        Node::Fields(fields) => Node::Fields(reverse_fields(fields)),
        Node::Items { items, order } => Node::Items {
            items: reverse_fields(items),
            order: order.as_ref().map(reverse_order),
        },
    }
}

fn reverse_fields(fields: &BTreeMap<String, Node>) -> BTreeMap<String, Node> {
    fields.iter().map(|(k, n)| (k.clone(), reverse_node(n))).collect()
}

fn apply_fields(map: &mut Map<String, Value>, fields: &BTreeMap<String, Node>) {
    for (key, node) in fields {
        match node {
            Node::Change { after: Some(v), .. } => {
                map.insert(key.clone(), v.clone());
            }
            Node::Change { after: None, .. } => {
                map.remove(key);
            }
            _ => {
                if let Some(child) = map.get_mut(key) {
                    apply_node(child, node);
                }
            }
        }
    }
}

fn apply_node(value: &mut Value, node: &Node) {
    match node {
        Node::Change { after: Some(v), .. } => *value = v.clone(),
        Node::Change { after: None, .. } => *value = Value::Null,
        Node::Fields(fields) => {
            if let Value::Object(map) = value {
                apply_fields(map, fields);
            }
        }
        Node::Items { items, order } => {
            if let Value::Array(list) = value {
                apply_items(list, items, order.as_ref());
            }
        }
    }
}

/// Apply changes to a keyed list. When the list is the one the patch was made from, the result is
/// exact. When it isn't (a checked undo, or a gesture's base being kept up with other people),
/// items that are gone are skipped and new ones land at the end; the caller re-sorts.
fn apply_items(list: &mut Vec<Value>, items: &BTreeMap<String, Node>, order: Option<&Order>) {
    let ids: Vec<String> = list.iter().filter_map(|v| id_of(v).map(str::to_string)).collect();
    let mut pool: HashMap<String, Value> = ids.iter().cloned().zip(std::mem::take(list)).collect();
    let mut added = Vec::new();
    for (id, node) in items {
        match node {
            Node::Change { after: None, .. } => {
                pool.remove(id);
            }
            Node::Change { after: Some(v), .. } => {
                if pool.insert(id.clone(), v.clone()).is_none() {
                    added.push(id.clone());
                }
            }
            _ => {
                if let Some(item) = pool.get_mut(id) {
                    apply_node(item, node);
                }
            }
        }
    }
    let fits = order.is_some_and(|o| {
        ids.len() >= o.start + o.before.len() && ids[o.start..o.start + o.before.len()] == o.before[..]
    });
    let sequence: Vec<String> = match order {
        Some(o) if fits => {
            let mut s = ids[..o.start].to_vec();
            s.extend(o.after.iter().cloned());
            s.extend(ids[o.start + o.before.len()..].iter().cloned());
            s
        }
        _ => ids.into_iter().chain(added).collect(),
    };
    list.extend(sequence.iter().filter_map(|id| pool.remove(id)));
    // Whatever the window didn't mention still belongs in the list.
    let mut rest: Vec<(String, Value)> = pool.into_iter().collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    list.extend(rest.into_iter().map(|(_, v)| v));
}

/// Keep what still applies to `current`. A field that moved on is left out; an item this patch
/// would add or remove that moved on refuses the whole patch, since other work may hang off it.
fn check_node(node: &Node, current: Option<&Value>, structural: bool) -> Result<Option<Node>, Conflict> {
    match node {
        Node::Change { before, .. } => {
            if current == before.as_ref() {
                Ok(Some(node.clone()))
            } else if structural {
                Err(Conflict)
            } else {
                Ok(None)
            }
        }
        Node::Fields(fields) => {
            let Some(Value::Object(map)) = current else { return Ok(None) };
            let kept = check_fields(fields, map)?;
            Ok((!kept.is_empty()).then_some(Node::Fields(kept)))
        }
        Node::Items { items, order } => {
            let Some(Value::Array(list)) = current else { return Ok(None) };
            let by_id: HashMap<&str, &Value> = list.iter().filter_map(|v| Some((id_of(v)?, v))).collect();
            let mut kept = BTreeMap::new();
            for (id, n) in items {
                let is_change = matches!(n, Node::Change { .. });
                if let Some(n) = check_node(n, by_id.get(id.as_str()).copied(), is_change)? {
                    kept.insert(id.clone(), n);
                }
            }
            let order = order.clone();
            Ok((!kept.is_empty() || order.is_some()).then_some(Node::Items { items: kept, order }))
        }
    }
}

fn check_fields(fields: &BTreeMap<String, Node>, map: &Map<String, Value>) -> Result<BTreeMap<String, Node>, Conflict> {
    let mut kept = BTreeMap::new();
    for (key, node) in fields {
        if let Some(n) = check_node(node, map.get(key), false)? {
            kept.insert(key.clone(), n);
        }
    }
    Ok(kept)
}

impl Patch {
    /// What it takes to get from `before` to `after`.
    pub fn between(before: &ProjectModel, after: &ProjectModel) -> Patch {
        Patch(diff_objects(&to_json(before), &to_json(after)))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The patch that takes this one back.
    pub fn reversed(&self) -> Patch {
        Patch(reverse_fields(&self.0))
    }

    /// `m` with this patch applied. Exact when `m` is what the patch was made from.
    pub fn apply(&self, m: &ProjectModel) -> Result<ProjectModel, PatchError> {
        let mut map = to_json(m);
        apply_fields(&mut map, &self.0);
        from_json(map)
    }

    /// The part of this patch that still holds against `current`: values somebody else has
    /// changed since are dropped, so their edit stands. `Err` means an item this patch would add
    /// or remove has moved on, and none of it should be applied. With nothing else going on,
    /// that is the whole patch.
    pub fn applicable_to(&self, current: &ProjectModel) -> Result<Patch, Conflict> {
        Ok(Patch(check_fields(&self.0, &to_json(current))?))
    }

    /// This patch without what belongs to one person rather than the project: track mute and
    /// solo, the loop brace and the punch range. See [`Personal`].
    pub fn without_personal(&self) -> Patch {
        let mut fields = self.0.clone();
        fields.remove("loopRegion");
        fields.remove("punchRegion");
        if let Some(Node::Items { items, order }) = fields.get_mut("tracks") {
            items.retain(|_, node| {
                if let Node::Fields(track) = node {
                    track.remove("muted");
                    track.remove("soloed");
                    !track.is_empty()
                } else {
                    true
                }
            });
            if items.is_empty() && order.is_none() {
                fields.remove("tracks");
            }
        }
        Patch(fields)
    }

    /// Whether this patch changes `field` of the model (its JSON name), or anything inside it.
    pub fn touches(&self, field: &str) -> bool {
        self.0.contains_key(field)
    }

    /// Whether any track's `field` (its JSON name) changes, or a track comes or goes.
    pub fn touches_track_field(&self, field: &str) -> bool {
        match self.0.get("tracks") {
            Some(Node::Items { items, .. }) => items.values().any(|n| match n {
                Node::Fields(f) => f.contains_key(field),
                _ => true,
            }),
            Some(_) => true,
            None => false,
        }
    }
}

/// What belongs to one person in a shared session rather than to the project.
///
/// A session keeps these fields in each person's own copy of the model, which is why playback,
/// export and saving need no idea that a session exists: they read the model as they always have.
/// [`Patch::without_personal`] keeps them from leaving the machine, and rebuilding a guest's view
/// lays them back on top.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Personal {
    pub muted: HashSet<uuid::Uuid>,
    pub soloed: HashSet<uuid::Uuid>,
    pub loop_region: Option<(i64, i64)>,
    pub punch_region: Option<(i64, i64)>,
}

impl Personal {
    pub fn of(m: &ProjectModel) -> Personal {
        Personal {
            muted: m.tracks.iter().filter(|t| t.muted).map(|t| t.id).collect(),
            soloed: m.tracks.iter().filter(|t| t.soloed).map(|t| t.id).collect(),
            loop_region: m.loop_region,
            punch_region: m.punch_region,
        }
    }

    /// Lay this over `m`, holding the ranges inside the song, which may have shrunk.
    pub fn overlay(&self, m: &mut ProjectModel) {
        for t in &mut m.tracks {
            t.muted = self.muted.contains(&t.id);
            t.soloed = self.soloed.contains(&t.id);
        }
        let end = m.song_length_ticks;
        let hold = |r: Option<(i64, i64)>| r.and_then(|(s, e)| crate::model::clamp_loop(s, e, end));
        m.loop_region = hold(self.loop_region);
        m.punch_region = hold(self.punch_region);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NoteEvent;

    fn model() -> ProjectModel {
        let mut m = ProjectModel::empty();
        m.patterns[0].notes = (0..4).map(|i| NoteEvent::new(i as i64 * 240, 240, 60 + i)).collect();
        m
    }

    #[test]
    fn a_patch_gets_from_one_model_to_the_other_and_back() {
        let a = model();
        let mut b = a.clone();
        b.tempo = 97.5;
        b.patterns[0].notes[1].pitch = 72;
        b.patterns[0].notes.remove(0);
        b.patterns[0].notes.push(NoteEvent::new(960, 120, 50));
        b.patterns[0].notes.swap(0, 1);
        let p = Patch::between(&a, &b);
        assert_eq!(p.apply(&a).unwrap(), b);
        assert_eq!(p.reversed().apply(&b).unwrap(), a);
    }

    #[test]
    fn one_changed_field_is_one_small_change() {
        let a = model();
        let mut b = a.clone();
        b.patterns[0].notes[2].pitch = 72;
        let json = serde_json::to_string(&Patch::between(&a, &b)).unwrap();
        assert!(json.len() < 300, "{json}");
    }

    #[test]
    fn adding_a_note_does_not_ship_the_list() {
        let a = model();
        let mut b = a.clone();
        b.patterns[0].notes.insert(2, NoteEvent::new(300, 120, 50));
        let p = Patch::between(&a, &b);
        assert_eq!(p.apply(&a).unwrap(), b);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.matches(&a.patterns[0].notes[0].id.to_string()).count() == 0, "{json}");
    }

    #[test]
    fn undo_leaves_a_field_somebody_else_changed() {
        let a = model();
        let mut mine = a.clone();
        mine.patterns[0].notes[0].pitch = 70;
        mine.patterns[0].notes[1].pitch = 71;
        let edit = Patch::between(&a, &mine);
        let mut now = mine.clone();
        now.patterns[0].notes[1].pitch = 40;
        let undo = edit.reversed().applicable_to(&now).unwrap();
        let after = undo.apply(&now).unwrap();
        assert_eq!(after.patterns[0].notes[0].pitch, a.patterns[0].notes[0].pitch);
        assert_eq!(after.patterns[0].notes[1].pitch, 40);
    }

    #[test]
    fn undoing_an_add_is_refused_once_somebody_built_on_it() {
        let a = model();
        let mut mine = a.clone();
        mine.patterns[0].notes.push(NoteEvent::new(960, 120, 50));
        let edit = Patch::between(&a, &mine);
        let mut now = mine.clone();
        now.patterns[0].notes[4].pitch = 41;
        assert_eq!(edit.reversed().applicable_to(&now), Err(Conflict));
    }

    #[test]
    fn personal_fields_stay_home() {
        let a = model();
        let mut b = a.clone();
        b.tracks[0].muted = true;
        b.tracks[0].soloed = true;
        b.loop_region = Some((0, 960));
        b.punch_region = Some((0, 960));
        assert!(Patch::between(&a, &b).without_personal().is_empty());
        b.tracks[0].volume = 0.25;
        let shared = Patch::between(&a, &b).without_personal();
        let out = shared.apply(&a).unwrap();
        assert_eq!(out.tracks[0].volume, 0.25);
        assert!(!out.tracks[0].muted);
        assert_eq!(out.loop_region, None);
    }

    #[test]
    fn a_patch_survives_the_wire() {
        let a = model();
        let mut b = a.clone();
        b.tempo = 1.0 / 3.0 * 300.0;
        b.swing = 66.66666666666667;
        let p = Patch::between(&a, &b);
        let wire: Patch = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(wire.apply(&a).unwrap(), b);
    }
}

#[cfg(test)]
mod speed {
    use super::*;
    use crate::model::NoteEvent;
    use std::time::Instant;

    /// How long a change costs on a big project. Not a pass or fail, so it is ignored; run it with
    /// `cargo test --release -p dissonant-core patch_speed -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn patch_speed() {
        let mut a = ProjectModel::empty();
        for p in &mut a.patterns {
            p.notes = (0..20_000).map(|i| NoteEvent::new(i * 60, 60, 40 + (i % 40) as i32)).collect();
        }
        let notes: usize = a.patterns.iter().map(|p| p.notes.len()).sum();
        let mut b = a.clone();
        b.patterns[0].notes[100].pitch = 90;
        let runs = 20;
        let t = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(Patch::between(&a, &b));
        }
        let diff = t.elapsed() / runs;
        let p = Patch::between(&a, &b);
        let t = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(p.apply(&a).unwrap());
        }
        println!("{notes} notes: diff {diff:?}, apply {:?}", t.elapsed() / runs);
    }
}
