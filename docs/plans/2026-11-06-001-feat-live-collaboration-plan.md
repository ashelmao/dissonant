# Plan: live collaboration

Tracked in beads (epic `dis-nky`, run `bd list --parent dis-nky`). Task N below is `dis-nky.(N+1)`, so
task 0 is `dis-nky.1`. Audio sync is `dis-nky.12`.

## What was built

Implemented on 2026-11-06. The design below held, with these changes. Where they disagree, this
section is right and the rest of the document is the original plan.

- **Personal state is simpler than planned.** There is no overlay in front of the model. Each person's
  own copy of the model simply holds their own mute, solo, loop and punch, and `Patch::without_personal`
  keeps those fields from being sent. Playback, export and saving needed no changes at all. Personal
  edits also stay undoable, on your own stack.
- **The host sends no inverses.** A guest builds its own undo steps from its own prediction, and the
  host checks them when they arrive. `Inverse` and `CommitGesture` are gone from the protocol.
- **Reconnecting always sends the whole project,** so there is no log of past revisions to keep or
  replay. A guest holds its edits until the host's welcome, then resends what the host hasn't seen.
- **A guest's undo is checked per item, as planned, but a conflict on an added or removed item refuses
  the whole step,** because other work may hang off that item.
- **Sharing doesn't need a saved project.** Host autosave covers an untitled one.
- **Removing someone also rotates the invite,** because they still hold it. People already in a session
  stay in through a new invite, even across a dropped connection.
- **`dissonant-net` is its own crate** rather than living in `src-tauri`, so it runs in `cargo test`.

**Not built yet:** selection outlines in other people's colors, `dissonant://` deep links (the invite is
pasted), keep-awake while hosting, per-guest rate limits, running our own relay (n0's public relays are
used), and audio sync. Tracked in beads under `dis-nky`.

## Goal

Several people work on one project at the same time, Google Docs style. Everyone's edits appear
for everyone within a moment. Each person plays the track on their own machine, from their own
playhead. When two people are in the same place, either the song tab or the same pattern, they
see each other's cursors.

## Decisions

Settled with the user on 2026-11-06.

- **The project lives on the host's machine.** One person's app is the authority and the others
  join that live session. The project stays an ordinary `.dissonant` file on the host's disk.
  The session ends when the host leaves, and nobody can work on it without them.
- **Access is by invite link.** Anyone with the link joins and picks a display name and a cursor
  color. There are no accounts. The host can reset the link.
- **Notes first, audio later.** The first release syncs notes, clips, chords, sound and mixing.
  Recorded audio stays on the machine that has it, and everyone else sees a placeholder.
- **Mute, solo, the loop brace and the punch range are personal.** All four are saved in the
  shared project today, so this is a real change to how they work during a session.

### Defaults this plan chose

These follow from the decisions above. Each one is easy to change now and harder later, so they
are worth a look during review.

- **Guests can't add audio in v1.** Recording and importing audio are host-only during a session.
  A guest's take would exist only on the guest's machine, so the host's project would keep a clip
  it can never play after that guest leaves. Guests can still move and trim existing audio clips.
  This is the biggest cost of "notes first": a guest with a guitar can't record until audio sync
  lands.
- **"Mute" means the track M button.** Muting a clip (`0`) and muting a note stay shared, because
  they are edits to the music rather than listening choices.
- **Faders, pan, tempo, key and swing stay shared.** Changing the tempo changes it for everyone,
  including anyone who is playing.
- **Undo only undoes your own edits.** See [Undo](#undo).
- **When the host leaves, guests are offered a local copy** of the project as they last saw it, so
  nobody loses sight of the work. The copy is a new, separate project.
- **Export renders what you hear,** so it follows your personal mute and solo. It already respects
  mute and solo today. A guest's export warns about any audio that isn't on their machine.
- **A session holds up to 8 people,** host included.

## What the code gives us

| Already in place | Why it helps |
|---|---|
| Every edit is a `Command` through one bus ([document.rs](../../crates/dissonant-core/src/document.rs), 67 variants) | Edits can be sent over a network as they are, and the JSON is pinned by [ipc_contract.rs](../../crates/dissonant-core/tests/ipc_contract.rs) |
| Transport lives in `PlaybackContext` and the engine, not the model | Each person already has their own playhead. "Play for yourself" needs no new work |
| Pattern loop, ghost, chord bed and metronome are local | Same as above |
| Notes, clips, chords, sections, tempo points and breakpoints all have stable ids | Concurrent edits can be merged per item |
| The UI already takes pushed updates through the `document` event ([store.ts:778](../../ui/src/store.ts)) | Remote edits can arrive with no new plumbing |
| `reconcileSelection` repairs a selection when its pattern disappears ([store.ts:594](../../ui/src/store.ts)) | Someone deleting what you're looking at is mostly handled |
| Non-factory sounds and plugin state are embedded in the model | They travel with the project. A missing plugin already keeps its name and says so |

| Has to change | Why |
|---|---|
| `SetNotes` and `SetChords` send whole arrays ([document.rs:179](../../crates/dissonant-core/src/document.rs)) | Two people in one pattern would erase each other's notes |
| Undo restores whole-model snapshots ([document.rs:577](../../crates/dissonant-core/src/document.rs)) | In a session, undo would also revert everyone else's work |
| `apply_to` mints random UUIDs in 10 places, plus model constructors | A guest's prediction and the host's result would disagree on ids |
| About 15 call sites in [lib.rs](../../src-tauri/src/lib.rs) call the document directly, and the recorder takes `&mut Document` | Every edit has to pass through one gateway |
| Mute, solo, loop and punch are model fields | They have to become personal without rewriting the UI |
| The `audio`, `sync_plugins` and `sync_inserts` follow-ups are decided per Tauri command | Remote edits need the same follow-ups |

## Approach

### Shape of the system

```
 guest app                              host app                         guest app
┌─────────────────────┐               ┌───────────────────────┐        ┌───────────┐
│ UI ─ dispatch       │    command    │ Session host          │ patch  │           │
│  │                  │ ───────────►  │  runs apply_to        │ ─────► │   same    │
│ AppState::submit    │               │  diffs before/after   │        │    as     │
│  │                  │ ◄───────────  │  assigns a revision   │        │   left    │
│ Guest: confirmed    │     patch     │  saves, autosaves     │        │           │
│  + pending = view   │               │                       │        │           │
│  │                  │ ◄──────────►  │ fans out presence     │ ◄────► │           │
│ local engine        │   presence    │ local engine          │        │           │
└─────────────────────┘               └───────────────────────┘        └───────────┘
```

It is a star: guests talk only to the host. No CRDT is needed, because one machine puts every edit
in order.

### The host runs commands, everyone else receives patches

A guest sends a `Command`. The host runs it through the existing `apply_to`, diffs the model before
and after, and broadcasts the result as a **patch** with a new revision number.

Commands only ever run for real on the host, and that is the main simplification here. `InsertTime`,
clip splitting, chord trimming and every other piece of ripple logic stays exactly as it is, and
guests never need to reproduce it. Guests only need to apply patches, which is one small generic
function.

A patch is a list of changes keyed by id: upsert an item, remove an item, set a field, set the order
of a list. It lives in a new `crates/dissonant-core/src/patch.rs`:

- `diff(before, after) -> Patch` and `apply(model, patch)`, with the rule
  `apply(a, diff(a, b)) == b` tested heavily.
- Top-level collections are compared with Rust `PartialEq`, and only items that changed are
  serialized. Inside a changed item the diff works on JSON, treating arrays of objects with an `id`
  as keyed. New model fields then work without touching the diff.
- `serde_json` needs its `float_roundtrip` feature (it's off today), or a last-digit float change
  would make `is_dirty` and the undo guards see edits that didn't happen.

### Guests predict, so editing feels instant

A guest keeps two things: the **confirmed** model (everything the host has sent) and a list of
**pending** commands the host hasn't answered yet. What the UI shows is the **view**: confirmed plus
pending, replayed.

- A local edit runs through `apply_to` on the view straight away and returns a snapshot, so
  `dispatch` behaves as it does today.
- When a patch arrives, the guest applies it to confirmed, drops its own command if the patch
  answers it, and replays what's left.
- If the host refuses a command (the clip was deleted a moment ago), the guest drops it, the replay
  removes the edit, and a toast says what happened.

Ids have to match between the prediction and the real result, or a newly added track would lose its
selection when confirmed. Each op carries an `op_id`, and a thread-local id source in core derives
ids from it with `Uuid::new_v5(op_id, counter)`. Outside an op it falls back to `new_v4`, so solo
behaviour doesn't change. This is the same deterministic-id trick the ghost repeats already use in
[sequence.rs](../../crates/dissonant-core/src/sequence.rs).

### Concurrent edits

The rule is simple: **edits to different items merge, and edits to the same item go to whoever
reached the host last.**

The hard case is the roll, which commits whole arrays. It builds `next` from the notes it saw when
the drag began. If someone else added a note in the meantime, comparing `next` with the current model
would look as though you had deleted their note.

The fix is a three-way comparison, and the UI's many `setNotes` calls don't have to change:

1. `Snapshot` gains `revision`.
2. The store records the revision when an editor's pointer goes down (`beginEdit`), and `dispatch`
   sends it as `base_revision`. That is one hook per editor rather than one per call site. Outside a
   gesture it sends the current revision.
3. `AppState` keeps a short ring of recent models. The gateway compares the base with `next`, and
   turns `SetNotes` into `PatchNotes { pattern_id, upsert, remove }`. `SetChords` becomes
   `PatchChords` the same way.

`PatchNotes` has to keep what `SetNotes` does today: sanitize each note, keep the list sorted, and run
`tidy_ropes`, which is what cuts a rope when its source note is removed.

Whole-value commands (`SetMaster`, `SetTrackAmp`, `SetDrumPads`) stay last-writer-wins for v1. Two
people turning different master controls in the same moment could lose one change. If that matters
in practice, they can move to field-level patches later.

### Undo

Undo moves from whole-model snapshots to patches, **for solo use as well**. There is then one undo
system, and the 4,349 lines of [document_behavior.rs](../../crates/dissonant-core/tests/document_behavior.rs)
prove it still behaves. It also removes up to 200 retained model clones.

An undo step stores, for each item it touched, the value before and the value after. Undoing restores
the "before" value **only if the item still holds the "after" value**. If someone else has changed it
since, their edit stays. In solo use that guard can never fire, so nothing changes there.

- Each person has their own stack, kept on their own machine. The host sends a step's inverse to the
  person who made the edit, and undo goes back to the host as an ordinary op.
- A gesture (a knob drag, a freehand automation stroke) stays one step, as it is today: keep the first
  "before" and the last "after" for each item.
- If nothing could be undone because other people changed all of it, a toast says so.
- Undoing a big structural edit such as `InsertTime` after other people have edited inside that range
  can restore only part of it. This is the weakest spot in the design (see [Risks](#risks)).

### Personal listening

The shared model keeps its `muted`, `soloed`, `loop_region` and `punch_region` fields. They become the
project's saved defaults, which are what a new joiner starts with. A small `PersonalView` sits on top:

```rust
struct PersonalView { muted: HashSet<Uuid>, soloed: HashSet<Uuid>,
                      loop_region: Option<(i64, i64)>, punch_region: Option<(i64, i64)> }
```

- `effective_model()` is the shared model with the personal view laid over it. `snapshot()`,
  `build_sequence`, export and `open_render_plugins` all read it. The UI keeps reading
  `track.muted` and `model.loopRegion`, so **no UI component changes**, and `syncLoop` keeps working.
- During a session, the gateway catches `SetTrackMuted`, `SetTrackSoloed`, `SetLoopRegion` and
  `SetPunchRegion` and never sends them. It runs them through `apply_to` on a scratch copy of the
  effective model and reads the result back, so the existing clamping rules still apply with no new
  logic.
- `stop_audio_record` reads `punch_region` straight off the document
  ([lib.rs:2386](../../src-tauri/src/lib.rs)). That has to read the effective model too.
- Time edits have to move personal ranges. `InsertTime` and `DeleteTime` shift the loop and punch
  ([ripple.rs:258](../../crates/dissonant-core/src/ripple.rs)), but guests only receive patches, so
  the host adds a `time_edit` note to the patch. Each person runs the same shifting code on their own
  ranges, once the `shift` and `pull` closures are pulled out into public functions. Ranges are
  clamped to the song length after every patch.
- These changes are not on the undo stack during a session, because they are view state. Outside a
  session nothing changes, and they stay undoable.
- When the host saves, the file gets the host's personal view baked in, so reopening the song alone
  gives the host what they had. The same happens when the session ends. Personal changes count as
  unsaved changes on the host.

### Presence and cursors

Positions are sent in **musical coordinates, never pixels**, because everyone has different zoom,
scroll and panel sizes.

```ts
type Location =
  | { screen: "song"; cursor?: { tick: number; trackId: string; laneFraction: number } }
  | { screen: "pattern"; patternId: string; cursor?: { tick: number; pitch: number } }
  | { screen: "setup" | "sound" | "rec" };
```

- A cursor is drawn only when the other person has the same `screen`, and for patterns the same
  `patternId`. The drum grid uses the same pattern location, since a row is a pitch.
- `cursor` is absent when their pointer isn't over the grid, so the cursor disappears rather than
  freezing in place.
- The roll can hide pitches per person (`visiblePitches`), so a cursor on a hidden row snaps to the
  nearest row shown. A cursor outside your scrolled view is pinned to the edge of the view with its
  name.
- Presence also carries the selection (note and clip ids), drawn as an outline in that person's color.
  Other people's drags are not shown until release in v1.
- Updates go out at most 30 times a second, unreliably, and the newest one wins. They never go through
  the edit path, never touch `snapshot` and never dirty the document.

**Rendering.** Cursors get their own small store outside `useStore`, and a `<PeerCursors>` overlay
reads it with `requestAnimationFrame`. The roll's main canvas and the arrangement must not re-render
at cursor rate. The overlay takes a `toPixels(position)` function from each editor. Motion is smoothed
between updates, and with reduced motion on, cursors simply jump.

**Look.** Following [DESIGN.md](../../DESIGN.md), a cursor is never told apart by color alone: each one
carries its name label. The color choices have to stay clear of the note-tier palette and the brand
color, which is reserved for your own selection.

A **presence bar** in the top bar shows everyone in the session and where they are ("in bass 2",
"in the song", "in sound"). Clicking someone takes you to their location.

### Network and invite link

Use [iroh](https://github.com/n0-computer/iroh): Rust, MIT or Apache-2.0 (which suits the licensing
position the project took on plugin hosting), now at 1.x. It dials by public key, tries a direct
encrypted QUIC connection, and falls back to a relay when a direct path fails. So hosts don't need
port forwarding, and relays can't read session traffic.

- **Relays.** n0's public relays are free but are for development: they are rate-limited, with
  unpublished limits and no uptime guarantee. Cursor traffic through a rate-limited relay would stutter.
  Use them during development, then run `iroh-relay` on a small VPS for release. It holds no project
  data and needs only a public IP and a DNS name. This is the one piece of infrastructure the feature
  needs.
- **Invite code.** The host's address plus a random room secret, encoded as text. **Pasting it into a
  Join dialog is the main path**, and it always works. `dissonant://join/<code>` links are an addition
  (`tauri-plugin-deep-link`, plus `tauri-plugin-single-instance` on Windows and Linux), since chat apps
  may not make custom-scheme links clickable. A static `https` redirect page can solve that later with
  no backend.
- **Reset link** rotates the secret. People already in the session stay in, and the host can remove
  anyone from the presence bar.
- All networking lives behind a `Transport` trait, so the session logic has no iroh in it and tests run
  in memory.

### Session lifecycle

| Event | What happens |
|---|---|
| Host starts sharing | Project must be saved first, so the session has a real file. Undo history carries over |
| Guest joins | Unsaved work prompts first, as File → Open does today. Handshake checks protocol version, `SCHEMA_VERSION` and the room secret, then the guest receives the full model |
| Version mismatch | Refused with a plain message naming both versions. Everyone in a session has to run the same protocol version |
| Guest's connection drops | Keeps editing into pending for up to 60 seconds and shows "reconnecting". On return it resumes from its last revision, and pending ops replay safely because the host ignores any `(client, seq)` it has already applied. After 60 seconds, see "Host goes away" |
| Host's project changes wholesale | File → New and File → Open are blocked while hosting, with a prompt to end the session first |
| Host saves | Normal save. Autosave and `.bak` keep working on the host alone |
| Guest presses ⌘S | Save a copy, which makes a separate local project |
| Host closes the app | Prompts "end the session for N people?" |
| Host goes away (ends, crashes, or sleeps) | Guests get "session ended" and the offer to keep a local copy. The host's autosave covers the host. Hosting keeps the machine from idle-sleeping |
| Guest leaves | Their cursor goes. Their edits stay |

### Audio in v1

- The `Welcome` message lists which audio sources the host has. `Snapshot` gains `missingAudio`.
- `audio_peaks` already returns silence for a missing source and `sync_audio_pool` already logs and
  carries on, so nothing breaks. The audio clip draws a hatched placeholder labelled "not on this
  machine" in place of a flat waveform.
- `AddAudioTake`, `PunchAudioTake`, `import_audio`, and transcription of a clip that isn't local are
  refused for guests. The rec page says why rather than failing on the button press.

### Protocol

JSON, because `Command` uses tagged enums and the wire format is already pinned. The full model is
compressed in `Welcome`. Messages are size-limited, and ops are rate-limited per guest.

```
guest → host   Hello { protocol, schema, secret, name, color, resume_from? }
               Op { op_id, seq, gesture?, transient, label?, body: Command | Undo(step) }
               CommitGesture { gesture }
               Presence { location, selection }          (unreliable)
               Bye
host → guest   Welcome { client_id, revision, model, peers, audio_sources }
               Applied { revision, origin, seq, patch, time_edit? }
               Inverse { seq, step }                     (to the op's origin only)
               Rejected { seq, reason }
               PresenceBatch { peers }                   (unreliable)
               Peers { peers }
               Ended { reason }
```

### Where the code goes

- `crates/dissonant-core`: `patch.rs`, `ids.rs`, `personal.rs`, `PatchNotes` and `PatchChords`,
  patch-based undo in `Document`.
- `crates/dissonant-session` (new): protocol types, the `Host` and `Guest` state machines, and the
  `Transport` trait with an in-memory version. No async, no iroh, no Tauri.
- `src-tauri/src/collab.rs` (new): the iroh transport and the session pump, plus the Tauri commands
  `share_start`, `share_stop`, `share_reset_link`, `join_session`, `leave_session`, `remove_peer` and
  `send_presence`.
- **Session pump.** One thread owns all session changes to the document. Network tasks hand it messages
  over a channel, so no network code holds `doc` directly. It coalesces remote updates and runs the
  follow-ups (`sync_engine`, `sync_inserts`, `sync_plugins`, `sync_audio_pool`, emitting `document`) at
  most 20 times a second, picking the follow-ups from what the patch touched.
- `ui/src`: `collab.ts` (presence store, position mapping), `components/ShareDialog.tsx`,
  `components/PresenceBar.tsx`, `components/PeerCursors.tsx`.
- [bridge.ts](../../ui/src/bridge.ts): the mock bridge gains a scripted fake peer that moves a cursor
  and makes edits, so the UI can be built and tested in a browser.

## Tasks

Sizes follow the roadmap's S / M / L. Tasks 1 to 4 ship nothing visible and change no behaviour, so they
can merge to `main` as they finish.

0. **Network spike** (S, timeboxed). Connect two real machines on different networks through iroh inside
   the Tauri app. Measure round-trip time, direct versus relayed rates, cursor smoothness through a relay,
   and the binary size cost. Check iroh's minimum Rust version against the workspace's `rust-version =
   1.80`, and that its tokio sits happily beside Tauri's. *Done when:* go or no-go on iroh, with numbers.
   If it's no-go, only the transport changes.
1. **Deterministic ids** (S). The thread-local id source in core, used by the 10 mints in `document.rs`
   and `ripple.rs` and the model constructors they call. *Done when:* running the same command twice with
   the same `op_id` on the same model gives equal models, for every variant.
2. **Patches** (M). `patch.rs` and the `float_roundtrip` feature. Add `proptest` as a dev-dependency.
   *Done when:* `apply(a, diff(a, b)) == b` holds across generated models and random command sequences, and
   a model survives the JSON round trip exactly, floats included.
3. **Patch-based undo** (L). Replace `HistoryEntry` with patch steps, keeping labels, gestures and
   `MAX_UNDO`. Add the guard. Depends on 2. *Done when:* `document_behavior.rs` passes with no test edits.
4. **The gateway** (M). `AppState::submit` becomes the one way in, with all the direct `apply` and
   `apply_labeled` call sites routed through it, and the recorder returning a command. The audio, chain
   and plugin follow-ups move into it. Add `PatchNotes`, `PatchChords`, `Snapshot.revision`, the model
   ring, `base_revision` and `beginEdit`, with the new variants in `ipc_contract.rs`. *Done when:* solo
   behaviour is unchanged and a three-way test keeps a note added mid-gesture.
5. **Session core** (L). `dissonant-session`: protocol, `Host`, `Guest`, in-memory transport, per-person
   undo, resume after a drop. Depends on 1 to 4. *Done when:* the simulation test (see
   [Testing](#testing)) converges over thousands of seeded runs.
6. **Personal view** (M). `PersonalView`, `effective_model()`, and the catches in the gateway. Time-edit
   notes on patches, baking into the file on save, and export. Depends on 4. *Done when:* in a simulated
   session, a guest's mute, solo and loop never reach the host, the loop rides along with `InsertTime`,
   and reopening the host's file gives the host's own state.
7. **Connect it** (L). `collab.rs`, the session pump, the Tauri commands, Share and Join dialogs, the
   session rows in the lifecycle table, and a `DISSONANT_DATA_DIR` override so two copies can run on one
   machine ([lib.rs:3226](../../src-tauri/src/lib.rs) fixes the folder today). Depends on 0, 5 and 6.
   *Done when:* two people on different networks edit one pattern and each plays it separately. This is
   the first usable build.
8. **Cursors and presence** (M). Presence messages, the cursor store, `<PeerCursors>` in the roll, drum
   grid and song view, selections, and the presence bar. Can start against the mock bridge's fake peer
   once 4 is merged. *Done when:* cursors appear only in a shared location, and profiling shows the
   editors don't re-render at cursor rate.
9. **Audio placeholders and guest limits** (S). `missingAudio`, the placeholder drawing, the refusals and
   their explanations, and the export warning. Depends on 7.
10. **Ship it** (M). Run `iroh-relay` and point release builds at it. Add deep links, the keep-awake while
    hosting, message limits and rate limits. A soak test, then the README and the `docs/decisions` entry.
    Depends on 7 to 9.

**Later, in its own plan: audio sync.** Host and guests exchange missing sources on demand over the same
connection (`iroh-blobs` looks like a natural fit), and guests get recording. Since the audio lives on
machines rather than a server, a take is only available while someone who has it is connected.

## Testing

- **Convergence simulation** is the most important test. A seeded test runs a host and several guests on
  the in-memory transport with random delays, reorders of presence, dropped connections and random commands
  from every variant. When everything drains, every client's model must equal the host's. It runs in plain
  `cargo test` with no network, and a failure reproduces from its seed.
- **Undo properties.** In any interleaving, undo never changes an item someone else edited later.
  Undo then redo returns to the same model when nobody else acted.
- **Existing suites stay green unchanged:** `document_behavior.rs`, `ipc_contract.rs`, `persistence.rs` and
  `bridge.test.ts`.
- **UI.** Unit tests for position mapping (zoom, scroll, hidden rows). Visual checks in the browser
  preview against the fake peer.
- **Real-world pass** before shipping: two copies on one machine, then two networks, then one relayed-only
  connection. Include a host crash, a laptop sleep and a mid-drag disconnect.
- **Soak.** Four clients for an hour, with a scripted editor on each and the transport playing, watching
  for audio dropouts and memory growth.

## Risks

| Risk | Plan |
|---|---|
| **Patch-based undo is the riskiest change, and it touches solo use.** | The existing suite pins behaviour, and it merges alone in task 3. If it goes badly, solo can keep snapshots and only sessions use patches, at the cost of two undo systems |
| **Undo of a big edit after others worked inside it** restores only part of it and could leave the song inconsistent | Accept for v1, with a toast. If testing shows it's confusing, refuse the undo instead when the guard trips on a structural step |
| **Full snapshots per update.** The whole model is sent to the UI after every edit, and three people multiply that. Plugin state blobs make it worse | The pump caps updates at 20 a second. If that's not enough, send patches to the UI as well. Measure during the soak test |
| **Engine rebuilds during playback.** Each remote edit rebuilds the sequence and sends `SetSequence` | Same path as local edits while playing, capped by the pump. The soak test listens for dropouts |
| **Slow plugin loads block the pump** when someone changes a plugin | Run `sync_plugins` off the pump thread |
| **Hosts on bad connections.** Everything depends on one home upload link and a laptop staying awake | Prediction hides latency, reconnection covers short drops, and the local copy covers the rest. This is the known cost of choosing host's machine |
| **Invite code leaks.** Whoever has it can edit the project, and plugin state from a guest reaches native plugin code on every machine, the same trust as opening someone's project file | Say so in the Share dialog. Removal and link reset exist, and the host's `.bak` and autosave remain |
| **Relay rate limits** if the release leans on n0's public relays | Run our own relay before release (task 10) |
| **`lib.rs` is 3,400 lines and has uncommitted work** | The gateway change in task 4 touches many lines, so land it after the current branch merges |

## Out of scope for v1

- Audio sync, and audio recording by guests.
- Handing the host role to someone else, or carrying on without the host.
- Offline editing that merges later.
- Accounts, roles, view-only links, comments, chat and voice.
- Seeing other people's playheads, or playing in sync with each other. Both could ride on presence later.
- Showing other people's drags live.
- Sharing plugin binaries. A guest without a plugin sees it named and missing, as today.

## Sources

- [iroh on GitHub](https://github.com/n0-computer/iroh) and [iroh on docs.rs](https://docs.rs/crate/iroh/latest)
- [Relays](https://iroh.computer/docs/concepts/relay), [rate limiting](https://docs.iroh.computer/relays/rate-limiting), [shared relays](https://docs.iroh.computer/iroh-services/relays/shared), [self-hosted relays](https://docs.iroh.computer/iroh-services/relays/self-hosted)
