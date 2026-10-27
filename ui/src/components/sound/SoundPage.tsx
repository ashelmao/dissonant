import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import "./sound.css";
import { getBridge } from "../../bridge";
import { pitchForKey } from "../../liveKeyboard";
import { selectedTrack, useStore } from "../../store";
import { addPad } from "../../drumKit";
import { FACTORY_SOUNDS, factorySound, filledPatch, isFactory, MAX_DRUM_PADS, userSoundId, type DrumPad, type Sound } from "../../sound";
import { historyOf, record, redo, seal, throttle, undo, type History } from "../../soundDesign";
import { midiName } from "../../theory";
import { keys } from "../../platform";
import { DrumPadButton, DRUM_PITCH, Keyboard, useAudition } from "./AuditionKeys";
import type { EditPatch } from "./Controls";
import { DrumEditor } from "./DrumEditor";
import { Scope } from "./Scope";
import { SoundBrowser, type SoundSource } from "./SoundBrowser";
import { SynthEditor } from "./SynthEditor";
import { archetypeFor, DRUM_KINDS, mutatePatch, rollPatch } from "./soundParams";

/** What the history tracks: everything but the id (which changes on save-as, not on undo). */
type Body = Omit<Sound, "id">;

interface Session {
  id: string;
  history: History<Body>;
  /** Where the draft was loaded from; null for a new sound that was never saved. */
  origin: SoundSource | null;
  /** The body as last loaded or saved (JSON), to tell whether there are unsaved edits. */
  saved: string | null;
}

/** The draft outlives the page: leave for the piano roll and come back to the same sound. */
let kept: Session | null = null;

const bodyOf = (s: Sound): Body => ({ name: s.name, category: s.category, patch: s.patch });
const sessionFor = (s: Sound, origin: SoundSource): Session => ({ id: s.id, history: historyOf(bodyOf(s)), origin, saved: JSON.stringify(bodyOf(s)) });
const VELOCITIES = [40, 70, 100, 127];
/** The "use on" pad choice that adds the sound to the kit instead of replacing a pad. */
const NEW_PAD = -1;

/** Start on the selected track's sound (its first pad for a drum track). */
function initialSession(): Session {
  const st = useStore.getState();
  const track = selectedTrack(st);
  const id = track ? (track.isDrum ? track.drumPads[0]?.sound : track.voice) : undefined;
  const lib = st.library.find((s) => s.id === id);
  if (lib) return sessionFor(lib, "library");
  const embedded = st.snapshot?.model.sounds?.find((s) => s.id === id);
  if (embedded) return sessionFor(embedded, "project");
  return sessionFor(factorySound(id ?? "") ?? FACTORY_SOUNDS[0], "factory");
}

function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

/**
 * SOUND: design synth and drum sounds. A browser of every sound on the left; the draft's
 * editor in the middle; audition keys, a live scope and the save / use-on-track actions
 * around it. The draft has its own undo (⌘Z here never touches the song).
 */
export function SoundPage() {
  const [sess, setSess] = useState<Session>(() => kept ?? initialSession());
  useEffect(() => {
    kept = sess;
  }, [sess]);
  const body = sess.history.present;
  const kind = body.patch.kind;
  const sound: Sound = useMemo(() => ({ id: sess.id, ...body }), [sess.id, body]);
  const [browseKind, setBrowseKind] = useState<"synth" | "drum">(kind);

  const library = useStore((s) => s.library);
  const model = useStore((s) => s.snapshot!.model);
  const selected = useStore(selectedTrack);
  const { dispatch, saveLibrarySound, deleteLibrarySound, confirm, askInput, showToast } = useStore.getState();

  const factory = isFactory(sess.id);
  const inLibrary = library.some((s) => s.id === sess.id);
  const bodyJson = useMemo(() => JSON.stringify(body), [body]);
  const dirty = sess.saved !== bodyJson;

  // ─── Draft edits and history ──────────────────────────────────────────────────────────────
  const edit: EditPatch = useCallback((patch, key = null) => setSess((s) => ({ ...s, history: record(s.history, { ...s.history.present, patch }, key) })), []);
  const commit = useCallback(() => setSess((s) => ({ ...s, history: seal(s.history) })), []);
  const setMeta = (k: "name" | "category", v: string) => setSess((s) => ({ ...s, history: record(s.history, { ...s.history.present, [k]: v }, k) }));
  const undoDraft = useCallback(() => setSess((s) => ({ ...s, history: undo(s.history) })), []);
  const redoDraft = useCallback(() => setSess((s) => ({ ...s, history: redo(s.history) })), []);

  // ─── Preview: the engine's preview channel follows the draft, ~30 updates a second ────────
  const preview = useMemo(() => throttle((s: Sound) => void getBridge().then((b) => b.previewSound(s)), 33), []);
  useEffect(() => preview(sound), [sound, preview]);
  useEffect(
    () => () => {
      preview.cancel();
      void getBridge().then((b) => b.previewSound(null));
    },
    [preview],
  );

  // ─── Audition: mouse keys / pad and the computer keyboard ─────────────────────────────────
  const [velocity, setVelocity] = useState(100);
  const [octave, setOctave] = useState(0);
  const audition = useAudition(velocity);
  const live = useRef({ octave, kind });
  live.current = { octave, kind };
  const { on: noteOn, off: noteOff, allOff } = audition;
  useEffect(() => allOff(), [kind, octave, allOff]);
  useEffect(() => {
    const held = new Map<string, number>();
    const onDown = (e: KeyboardEvent) => {
      const st = useStore.getState();
      if (st.dialog || st.settingsOpen || st.newProjectOpen || st.exportOpen || isTyping(e.target)) return;
      const mod = e.metaKey || e.ctrlKey;
      const k = e.key.toLowerCase();
      if (mod && !e.altKey && (k === "z" || k === "y")) {
        e.preventDefault();
        if (k === "y" || e.shiftKey) redoDraft();
        else undoDraft();
        return;
      }
      if (mod || e.altKey) return;
      if (e.repeat) {
        if (held.has(k)) e.preventDefault();
        return;
      }
      if (e.key === "-" || e.key === "=" || e.key === "+") {
        setOctave((o) => Math.max(-2, Math.min(2, o + (e.key === "-" ? -1 : 1))));
        return;
      }
      const p = pitchForKey(k, "chromatic", live.current.octave);
      if (p === null || held.has(k)) return;
      e.preventDefault();
      const pitch = live.current.kind === "drum" ? DRUM_PITCH : p;
      held.set(k, pitch);
      noteOn(pitch);
    };
    const onUp = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      const pitch = held.get(k);
      if (pitch === undefined) return;
      held.delete(k);
      noteOff(pitch);
    };
    const onBlur = () => {
      for (const p of held.values()) noteOff(p);
      held.clear();
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
      onBlur();
    };
  }, [noteOn, noteOff, undoDraft, redoDraft]);

  // ─── Loading, saving, deleting ────────────────────────────────────────────────────────────
  const discardOk = async () => !dirty || (await confirm(`Discard changes to “${body.name}”?`, "The edits you haven't saved will be lost.", "Discard", true));

  const load = async (s: Sound, source: SoundSource) => {
    if (s.id === sess.id && !dirty) return;
    if (!(await discardOk())) return;
    setSess(sessionFor(s, source));
  };

  const newSound = async () => {
    if (!(await discardOk())) return;
    const patch = filledPatch({ kind: browseKind });
    setSess({ id: userSoundId(), history: historyOf({ name: `new ${browseKind}`, category: "", patch }), origin: null, saved: null });
  };

  const saveAs = async (): Promise<Sound | null> => {
    const original = sess.saved ? (JSON.parse(sess.saved) as Body).name : null;
    const suggested = original !== null && body.name === original ? `${body.name} copy` : body.name;
    const name = await askInput("Save as a new sound", `It goes in my sounds${body.category ? ` under “${body.category}”` : ""}, ready in every project.`, "name", "Save", suggested);
    if (!name) return null;
    const s: Sound = { id: userSoundId(), name, category: body.category.trim(), patch: body.patch };
    try {
      await saveLibrarySound(s);
    } catch (e) {
      showToast(String(e), true);
      return null;
    }
    setSess((x) => ({ id: s.id, history: record(x.history, { ...x.history.present, name: s.name, category: s.category }), origin: "library", saved: JSON.stringify(bodyOf(s)) }));
    showToast(`saved “${s.name}” to my sounds`);
    return s;
  };

  /** Save in place; factory and never-saved drafts go through save-as. */
  const save = async (): Promise<Sound | null> => {
    if (factory || sess.origin === null) return saveAs();
    const s: Sound = { ...sound, name: sound.name.trim() || "untitled", category: sound.category.trim() };
    try {
      await saveLibrarySound(s);
    } catch (e) {
      showToast(String(e), true);
      return null;
    }
    setSess((x) => ({ ...x, origin: "library", saved: JSON.stringify(bodyOf(s)) }));
    showToast(`saved “${s.name}”`);
    return s;
  };

  const remove = async () => {
    if (!inLibrary) return;
    if (!(await confirm(`Delete “${body.name}”?`, "It leaves your library. Songs that already use it keep their own copy.", "Delete", true))) return;
    await deleteLibrarySound(sess.id);
    const embedded = !!useStore.getState().snapshot?.model.sounds?.some((s) => s.id === sess.id);
    setSess((x) => ({ ...x, origin: embedded ? "project" : null, saved: embedded ? x.saved : null }));
    showToast(`deleted “${body.name}”`);
  };

  // ─── Use on a track ───────────────────────────────────────────────────────────────────────
  const targets = model.tracks.filter((t) => t.isDrum === (kind === "drum"));
  const [targetId, setTargetId] = useState<string | null>(null);
  const target = targets.find((t) => t.id === targetId) ?? targets.find((t) => t.id === selected?.id) ?? targets[0] ?? null;
  /** The pad the sound replaces, or NEW_PAD to add it to the kit. */
  const [padPitch, setPadPitch] = useState<number | null>(null);
  const pads = target?.isDrum ? target.drumPads : [];
  const kitFull = pads.length >= MAX_DRUM_PADS;
  const newPad = padPitch === NEW_PAD && !kitFull;
  const pad = pads.find((p) => p.pitch === padPitch) ?? pads.find((p) => p.sound === sess.id) ?? pads[0] ?? null;
  const padName = (p: DrumPad) => {
    const s = (model.sounds ?? []).find((x) => x.id === p.sound) ?? library.find((x) => x.id === p.sound) ?? factorySound(p.sound);
    return `${midiName(p.pitch)} · ${s?.name ?? p.sound}`;
  };

  const useOnTrack = async () => {
    if (!target) return;
    const needsSave = factory ? dirty : dirty || (!inLibrary && sess.origin !== "project");
    const s = needsSave ? await save() : sound;
    if (!s) return;
    if (s.patch.kind === "synth") {
      await dispatch({ type: "setTrackSound", id: target.id, sound: s }, false, `sound: ${s.name}`);
      showToast(`“${s.name}” is on ${target.name}`);
      return;
    }
    const st = useStore.getState();
    const track = st.snapshot?.model.tracks.find((t) => t.id === target.id);
    if (!track || (!pad && !newPad)) return;
    // Every non-factory sound the kit now points at travels with the command.
    const pool = [s, ...st.library, ...(st.snapshot?.model.sounds ?? [])];
    const next = newPad ? addPad(track.drumPads, s, (id) => pool.find((x) => x.id === id) ?? factorySound(id)) : track.drumPads.map((p) => (p.pitch === pad!.pitch ? { ...p, sound: s.id } : p));
    if (!next) return showToast(`a kit holds up to ${MAX_DRUM_PADS} pads`, true);
    const ids = [...new Set(next.map((p) => p.sound).filter((id) => !isFactory(id)))];
    const sounds = ids.map((id) => pool.find((x) => x.id === id)).filter((x): x is Sound => !!x);
    await dispatch({ type: "setDrumPads", id: track.id, pads: next, sounds }, false, newPad ? `add pad: ${s.name}` : `pad sound: ${s.name}`);
    const pitch = newPad ? next[0].pitch : pad!.pitch;
    // The new pad is now the one this sound is on; another "use" replaces it, not adds again.
    if (newPad) setPadPitch(pitch);
    showToast(newPad ? `“${s.name}” is on a new pad (${midiName(pitch)}) of ${track.name}` : `“${s.name}” is on pad ${midiName(pitch)} of ${track.name}`);
  };

  // ─── Randomize / mutate ───────────────────────────────────────────────────────────────────
  const [rollKind, setRollKind] = useState("same");
  const roll = () => {
    const drumKind = rollKind === "same" ? archetypeFor(body.category) : rollKind;
    edit(rollPatch(body.patch, Math.random, drumKind));
  };

  const categories = useMemo(() => {
    const all = [...FACTORY_SOUNDS, ...library].filter((s) => s.patch.kind === kind).map((s) => s.category);
    return [...new Set(all.filter(Boolean))];
  }, [library, kind]);

  const status = factory ? "factory" : inLibrary ? "my sound" : sess.origin === "project" ? "project only" : "new";
  const statusHint = factory
    ? "a factory sound: edit freely, then save it as a sound of yours"
    : inLibrary
      ? "in my sounds: saved for every project"
      : sess.origin === "project"
        ? "only in this song (not in your library): save to keep it"
        : "not saved yet";
  const editorProps = { patch: body.patch, onEdit: edit, onCommit: commit };
  const canUndo = sess.history.past.length > 0;
  const canRedo = sess.history.future.length > 0;

  return (
    <div className="sound">
      <SoundBrowser kind={browseKind} setKind={setBrowseKind} currentId={sess.id} dirty={dirty} onPick={(s, src) => void load(s, src)} onNew={() => void newSound()} />
      <div className="sp-main">
        <div className="sp-head">
          <span className={`sp-kind ${kind}`}>{kind}</span>
          <input type="text" className="sp-name" value={body.name} onChange={(e) => setMeta("name", e.target.value)} onBlur={commit} aria-label="sound name" spellCheck={false} />
          <input type="text" className="sp-cat" value={body.category} placeholder="category" list="sp-categories" onChange={(e) => setMeta("category", e.target.value)} onBlur={commit} aria-label="category" spellCheck={false} />
          <datalist id="sp-categories">
            {categories.map((c) => (
              <option key={c} value={c} />
            ))}
          </datalist>
          <span className="sp-status" title={statusHint}>
            {status}
          </span>
          {dirty && (
            <span className="sp-dirty" title="edits not saved yet">
              ● unsaved
            </span>
          )}
          <span className="spacer" />
          <button className={dirty && !factory && sess.origin !== null ? "solid" : ""} onClick={() => void save()} disabled={!dirty && sess.origin !== null && !factory} title={factory || sess.origin === null ? "save as a new sound of yours" : "save over this sound in my sounds"}>
            save
          </button>
          <button onClick={() => void saveAs()} title="save a copy under a new name">
            save as…
          </button>
          <button className="danger" onClick={() => void remove()} disabled={!inLibrary} title={inLibrary ? "delete from my sounds" : "only your own saved sounds can be deleted"}>
            delete
          </button>
        </div>
        <div className="sp-tools">
          <button className="quiet" onClick={undoDraft} disabled={!canUndo} title={`undo the last edit to this sound (${keys("⌘Z")})`}>
            ↶ undo
          </button>
          <button className="quiet" onClick={redoDraft} disabled={!canRedo} title={`redo (${keys("⇧⌘Z")})`}>
            ↷ redo
          </button>
          <div className="vdiv" />
          <button onClick={roll} title="roll a new patch: random, but inside ranges that tend to sound good">
            ⚄ randomize
          </button>
          {kind === "drum" && (
            <select value={rollKind} onChange={(e) => setRollKind(e.target.value)} aria-label="what kind of drum to roll" title="what kind of drum a roll makes">
              <option value="same">like this one</option>
              {DRUM_KINDS.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          )}
          <button onClick={() => edit(mutatePatch(body.patch, Math.random))} title="nudge a handful of the knobs a little: happy accidents, close to home">
            ~ mutate
          </button>
          <span className="spacer" />
          <span className="cap">use on</span>
          {targets.length === 0 ? (
            <span className="sp-none">no {kind} tracks in this song</span>
          ) : (
            <>
              <select value={target?.id ?? ""} onChange={(e) => setTargetId(e.target.value)} aria-label="track to use the sound on">
                {targets.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name}
                  </option>
                ))}
              </select>
              {kind === "drum" && pad && (
                <select value={newPad ? NEW_PAD : pad.pitch} onChange={(e) => setPadPitch(Number(e.target.value))} aria-label="pad to replace" title="the pad this sound replaces, or a new pad for it">
                  {pads.map((p) => (
                    <option key={p.pitch} value={p.pitch}>
                      {padName(p)}
                    </option>
                  ))}
                  <option value={NEW_PAD} disabled={kitFull}>
                    {kitFull ? `kit full (${MAX_DRUM_PADS} pads)` : "+ new pad"}
                  </option>
                </select>
              )}
              <button className="solid" onClick={() => void useOnTrack()} disabled={!target} title={dirty || (!factory && !inLibrary && sess.origin !== "project") ? "saves the sound first, then puts it on the track" : "put this sound on the track"}>
                {kind === "drum" ? (newPad ? "add to kit" : "use on pad") : `use on ${target?.name ?? "track"}`}
              </button>
            </>
          )}
        </div>
        <div className="sp-body">{kind === "synth" ? <SynthEditor {...editorProps} /> : <DrumEditor {...editorProps} />}</div>
        <div className="sp-audition">
          <div className="sp-scopebox">
            <span className="cap">scope</span>
            <Scope width={220} height={78} />
          </div>
          {kind === "synth" ? <Keyboard octave={octave} audition={audition} /> : <DrumPadButton audition={audition} />}
          <div className="sp-actl">
            <span className="cap">velocity</span>
            <div className="seg sm" role="radiogroup" aria-label="audition velocity">
              {VELOCITIES.map((v) => (
                <div key={v} className={velocity === v ? "on" : ""} role="radio" aria-checked={velocity === v} onClick={() => setVelocity(v)}>
                  {v}
                </div>
              ))}
            </div>
            {kind === "synth" && (
              <>
                <span className="cap">octave</span>
                <div className="row">
                  <button className="quiet" onClick={() => setOctave((o) => Math.max(-2, o - 1))} disabled={octave <= -2} title="octave down (-)" aria-label="octave down">
                    −
                  </button>
                  <span className="mono sp-oct">{octave > 0 ? `+${octave}` : octave}</span>
                  <button className="quiet" onClick={() => setOctave((o) => Math.min(2, o + 1))} disabled={octave >= 2} title="octave up (=)" aria-label="octave up">
                    +
                  </button>
                </div>
              </>
            )}
            <span className="sp-hint">{kind === "synth" ? "play Z–/ and Q–P on your keyboard" : "any note key hits the pad"}</span>
          </div>
        </div>
      </div>
    </div>
  );
}
