import { useMemo } from "react";
import { create } from "zustand";
import { getBridge } from "./bridge";
import { detectionSummary } from "./detection";
import type {
  AmpRig,
  AudioImportProgress,
  AudioStatus,
  CatalogEntry,
  ChordEvent,
  Command,
  ExportProgress,
  ExportRequest,
  InputDevice,
  InsertTarget,
  InstallProgress,
  KeyState,
  MidiStatus,
  NoteEvent,
  NoteProp,
  OutputDevice,
  ParamInfo,
  PlayMode,
  PluginDescriptor,
  ProjectModel,
  RecoveryCandidate,
  Settings,
  Snapshot,
  SongPattern,
  SoundPreset,
  TemplateInfo,
  Track,
  ViewMode,
  VibeInfo,
} from "./types";
import {
  barTicks as barTicksOf,
  beatsPerBar as bpbOf,
  chordsInWindow,
  DEFAULT_SETTINGS,
  fileNameOf,
  insertChain,
  keyAt,
  TICKS_PER_BEAT,
  TIER_COLORS,
  ticksToBeats,
} from "./types";
import { applyUiScale } from "./uiScale";
import { nativeEffect } from "./effects";
import { FACTORY_SOUNDS, type Sound } from "./sound";
import type { ArrSnap } from "./clipEditing";

export interface Toast {
  text: string;
  error?: boolean;
}

/** One button of an in-app dialog. `value` is what `ask` resolves with. */
export interface DialogButton {
  label: string;
  value: string;
  kind?: "primary" | "danger" | "quiet";
}

export interface DialogSpec {
  title: string;
  message?: string;
  /** Optional monospace detail line (a path, a count). */
  detail?: string;
  buttons: DialogButton[];
  /** Resolved with when the dialog is dismissed with Esc / the backdrop. */
  cancelValue?: string;
  /** A text field; the typed value is read back through `dialogText`. */
  input?: { placeholder?: string; value?: string };
}

export type BottomPanel = "devices" | "mixer";
/**
 * Which top-level screen is showing. It is not what the engine sequences: `PlayMode` is a
 * Rust enum with two variants, and SETUP, SONG, SOUND and the rec page all play the song,
 * so `playModeOf` folds this down for the bridge.
 */
export type Screen = ViewMode | "rec";
export type LiveKeyboard = "off" | "tier" | "chromatic";

/** A pattern being dragged out of the tree, in client coordinates. */
export interface PatternDrag {
  patternId: string;
  clientX: number;
  clientY: number;
}

/** What the song view does when a dragged pattern is released over the window. */
export type PatternDropHandler = (patternId: string, clientX: number, clientY: number) => void;

interface State {
  snapshot: Snapshot | null;
  /** Invariant: the selected pattern (if any) belongs to the selected track. */
  selectedTrackId: string | null;
  selectedPatternId: string | null;
  /** The song tick the edited pattern's bar 1 sits on: its chords and section key come from there. */
  chordOffsetTicks: number;
  /** Clips selected in the song view. */
  selectedClipIds: string[];
  selectedNoteIds: string[];
  /** Notes copied out of a roll or a drum grid; one clipboard, so hits carry between them. */
  noteClipboard: NoteEvent[];
  /** Grid / default note length, in ticks. */
  noteLength: number;
  zoom: number;
  arrZoom: number; // px per bar in the arrangement
  /** The grid clip gestures land on in the song view ("off" = whole ticks). */
  arrSnap: ArrSnap;
  showLandscape: boolean;
  highlightRows: boolean;
  hearChords: boolean;
  bottomPanel: BottomPanel;
  /** Transport position in ticks (fractional between blocks). */
  playhead: number;
  playing: boolean;
  /** Beats of count-in left (0 = none). */
  countIn: number;
  masterPeak: [number, number];
  trackPeaks: number[];
  audio: AudioStatus | null;
  toast: Toast | null;
  dialog: DialogSpec | null;
  recent: string[];
  settings: Settings;
  settingsOpen: boolean;
  outputDevices: OutputDevice[];
  dialogText: string;
  exportOpen: boolean;
  exporting: ExportProgress | null;
  templates: TemplateInfo[];
  /** The new-project screen (vibes, starter, templates, recent files). */
  newProjectOpen: boolean;
  /** It was opened at launch, over the fresh starter the app starts with. */
  newProjectAtLaunch: boolean;
  vibes: VibeInfo[];
  looping: boolean;
  /** Loop start/end in ticks. */
  loopRegion: [number, number] | null;
  armed: boolean;
  /** The grid recorded notes snap to, in ticks; `null` records the take as it was played. */
  recordQuantize: number | null;
  midi: MidiStatus | null;
  midiActivityAt: number;
  liveKeyboard: LiveKeyboard;
  magnet: boolean;
  stamp: boolean;
  /** Brush: left-drag on empty space paints notes. */
  brush: boolean;
  /** Keep the playhead in view while playing. */
  follow: boolean;
  /** Fold: only rows with notes (or chord tones nearby) are shown. */
  fold: boolean;
  /** Another pattern (any track) whose notes are drawn as ghosts behind the roll (heard too while `hearGhost` is on). */
  ghostPatternId: string | null;
  /** Play the ghost pattern along in pattern mode (it is always drawn). */
  hearGhost: boolean;
  /** How loud the ghost plays: 0…1.5, 1 = its track's own level. */
  ghostLevel: number;
  /** A pattern being dragged from the tree; the song view previews the drop from it. */
  patternDrag: PatternDrag | null;
  /** Registered by the song view while mounted; the tree calls it on release. */
  patternDropHandler: PatternDropHandler | null;
  /** Which per-note property the lane under the roll (and the drum graph editor) edits. */
  laneProp: NoteProp;
  /** Something outside the roll (inspector, menu) asked for this selection. */
  selectionRequest: { ids: string[]; nonce: number } | null;
  /** Plugins found on this computer, in scan order; empty until `refreshPlugins`. */
  plugins: PluginDescriptor[];
  /** The curated free-plugin catalog; empty until `refreshCatalog`. */
  catalog: CatalogEntry[];
  /** The latest install event per catalog entry, so a row can show its own progress. */
  installing: Record<string, InstallProgress>;
  /** Which top-level screen is showing. */
  screen: Screen;
  /** Peak of the live input (0 when nothing is captured). */
  inputPeak: number;
  /** True while a take is being captured. */
  recordingAudio: boolean;
  inputDevices: InputDevice[];
  /** The audio track that hears the live input. */
  armedAudioTrackId: string | null;
  /** Hear the input through the rig, with or without the transport running. */
  monitor: boolean;
  selectedAudioClipId: string | null;
  /** Non-null while a file decodes. */
  importing: AudioImportProgress | null;
  /** The audio clip whose DI is being transcribed, while that runs. One at a time. */
  detectingClipId: string | null;
  /** The tuner strip is open. The engine is silent while it is. */
  tunerOpen: boolean;
  /** What the tuner last heard, in Hz; null when it hears nothing with a pitch. */
  tunerHz: number | null;
  /** How periodic that reading was, 0…1. */
  tunerClarity: number;

  init(): Promise<void>;
  /** `label` names the edit in the Edit menu ("move notes"); defaults to the command's name. */
  dispatch(
    command: Command,
    transient?: boolean,
    label?: string,
  ): Promise<void>;
  commitGesture(): Promise<void>;
  /** Voice, tone and reverb from a named preset, as one undo step. */
  applySoundPreset(trackId: string, preset: SoundPreset): Promise<void>;
  /** The global user sound library (loaded at init). */
  library: Sound[];
  loadLibrary(): Promise<void>;
  /** Save to the global library; also refreshes the open project's embedded copy if it has one. */
  saveLibrarySound(sound: Sound): Promise<void>;
  deleteLibrarySound(id: string): Promise<void>;
  undo(): Promise<void>;
  redo(): Promise<void>;
  /** Switch screens. PATTERN loops the selected pattern; SETUP, SONG, SOUND and the rec
   *  page all play the song. The only writer of `screen`. */
  setScreen(screen: Screen): void;
  /** Also selects that track's first pattern (or keeps the selected one if it's on that track), or none. */
  selectTrack(id: string): void;
  /** Also selects its track. */
  selectPattern(id: string): void;
  /** Select, set `chordOffsetTicks` to `atTick` (a clip's start; 0 = song bar 1) and switch to PATTERN. */
  openPattern(patternId: string, atTick?: number): void;
  /** Replace the clip selection, or add to it with `additive`. */
  selectClips(ids: string[], additive?: boolean): void;
  setSelectedNoteIds(ids: string[]): void;
  setNoteClipboard(notes: NoteEvent[]): void;
  setNoteLength(ticks: number): void;
  setZoom(zoom: number): void;
  setArrZoom(px: number): void;
  setArrSnap(snap: ArrSnap): void;
  toggleLandscape(): void;
  toggleHighlight(): void;
  toggleHearChords(): void;
  setBottomPanel(p: BottomPanel): void;
  play(): void;
  stop(): void;
  toggleMetronome(): void;
  togglePlay(): void;
  rewind(): void;
  seek(tick: number): void;
  tapTempo(): void;
  /** Preview a pitch. `lengthTicks` defaults to the current grid, so a preview never outlasts the note. */
  audition(pitch: number, velocity?: number, lengthTicks?: number): void;
  /** The roll's loop brace, in the selected pattern's own ticks. Dropped on a pattern switch. */
  setLoopRegion(region: [number, number] | null): void;
  /** The song's loop brace, in song ticks: an undoable edit, saved with the project. */
  setSongLoop(region: [number, number] | null): Promise<void>;
  /** The song's punch range, in song ticks: an undoable edit, saved with the project. Arming
   *  one blocks looping, so this says so rather than letting the two fight. */
  setPunch(region: [number, number] | null): Promise<void>;
  toggleLooping(): void;
  toggleRecord(): void;
  /** The grid a take snaps to; `null` records it as played. */
  setRecordQuantize(ticks: number | null): void;
  openMidi(name?: string): Promise<void>;
  closeMidi(): Promise<void>;
  refreshMidi(): Promise<void>;
  setLiveKeyboard(mode: LiveKeyboard): void;
  noteOn(pitch: number, velocity?: number): void;
  noteOff(pitch: number): void;
  toggleMagnet(): void;
  toggleStamp(): void;
  toggleBrush(): void;
  toggleFollow(): void;
  toggleFold(): void;
  setGhostPattern(id: string | null): void;
  toggleHearGhost(): void;
  setGhostLevel(level: number): void;
  setPatternDrag(drag: PatternDrag | null): void;
  setPatternDropHandler(fn: PatternDropHandler | null): void;
  setLaneProp(prop: NoteProp): void;
  requestSelection(ids: string[]): void;
  /** Apply a pure edit to the selected pattern's notes (ids = current selection). */
  transformSelection(
    fn: (notes: NoteEvent[], ids: Set<string>) => NoteEvent[],
    label: string,
  ): void;
  newProject(): Promise<void>;
  openProject(): Promise<void>;
  /** Open a known path (recent file, dropped file), asking about unsaved work first. */
  openPath(path: string): Promise<void>;
  /** Save; resolves true when the document ended up saved (false on cancel / error). */
  saveProject(saveAs?: boolean): Promise<boolean>;
  /** Open the export dialog (⌘E). */
  exportWav(): Promise<void>;
  openExport(open?: boolean): void;
  /** Pick a file and run the export described by `request` (path is filled in here). */
  runExport(request: Omit<ExportRequest, "path">): Promise<void>;
  exportMidi(): Promise<void>;
  importMidi(): Promise<void>;
  setDialogText(text: string): void;
  /** A dialog with one text field; resolves with the text, or null on cancel. */
  askInput(
    title: string,
    message: string,
    placeholder?: string,
    okLabel?: string,
    initial?: string,
  ): Promise<string | null>;
  openNewProject(open?: boolean): void;
  newFromVibe(id: string): Promise<void>;
  refreshTemplates(): Promise<void>;
  saveAsTemplate(): Promise<void>;
  deleteTemplate(name: string): Promise<void>;
  newFromTemplate(name: string | "starter" | "empty"): Promise<void>;
  restartAudio(): Promise<void>;
  showToast(text: string, error?: boolean): void;
  /** Show an in-app dialog and resolve with the chosen button's value. */
  ask(spec: DialogSpec): Promise<string>;
  /** Yes/no convenience over `ask`. */
  confirm(
    title: string,
    message?: string,
    okLabel?: string,
    danger?: boolean,
  ): Promise<boolean>;
  /** Called by a dialog button; resolves the pending `ask`. */
  answerDialog(value: string): void;
  /** Unsaved changes stand in the way of `what`; returns true when it is OK to proceed. */
  confirmDiscard(what: string): Promise<boolean>;
  /** The OS close button: save / discard / cancel, then quit. */
  requestClose(): Promise<void>;
  refreshRecent(): Promise<void>;
  clearRecent(): Promise<void>;
  openSettings(open?: boolean): void;
  /** Merge a partial patch into one settings section, persist, and apply it. */
  updateSettings<K extends keyof Settings>(
    section: K,
    patch: Partial<Settings[K]>,
  ): Promise<void>;
  refreshOutputDevices(): Promise<void>;
  testTone(): void;
  /** Acknowledge the latched master clip indicator. */
  clearClip(): void;
  refreshAudio(): Promise<void>;
  /** Plugins found on this computer; refreshed after an install. */
  refreshPlugins(rescan?: boolean): Promise<void>;
  /** The curated free-plugin catalog, loaded when the browser is first shown. */
  refreshCatalog(): Promise<void>;
  /** Put a scanned plugin on a track, or `null` to go back to the built-in voice. */
  setTrackPlugin(trackId: string, uid: string | null): Promise<void>;
  /** A track plugin's parameters with the values it currently holds. */
  pluginParams(trackId: string): Promise<ParamInfo[]>;
  /** Write one plugin parameter: `commit` false while dragging, true on release — only the
   *  committed one becomes an undo step. */
  setPluginParam(
    trackId: string,
    paramId: number,
    value: number,
    commit?: boolean,
  ): Promise<void>;
  /** Install one catalog entry and rescan; progress shows up in `installing`. */
  installPlugin(entryId: string): Promise<void>;
  /** Hand an https link to the user's own browser; anything else is ignored. */
  openUrl(url: string): Promise<void>;
  /** Open (or raise) the separate window holding one track's plugin controls. */
  openPluginWindow(trackId: string): Promise<void>;

  // ── Effect inserts ──────────────────────────────────────────────────────────────────────
  /** Put a scanned effect in a chain, appended unless `index` says otherwise. */
  addInsert(
    target: InsertTarget,
    uid: string,
    index?: number | null,
  ): Promise<void>;
  removeInsert(target: InsertTarget, insertId: string): Promise<void>;
  /** Move an insert within its chain; out-of-range indices land on the nearest end. */
  moveInsert(target: InsertTarget, insertId: string, toIndex: number): Promise<void>;
  setInsertEnabled(
    target: InsertTarget,
    insertId: string,
    on: boolean,
  ): Promise<void>;
  /** One insert's parameters with the values its effect currently holds. */
  insertParams(insertId: string): Promise<ParamInfo[]>;
  /** Write one insert parameter: `commit` false while dragging, true on release — only the
   *  committed one becomes an undo step. */
  setInsertParam(
    insertId: string,
    paramId: number,
    value: number,
    commit?: boolean,
  ): Promise<void>;
  /** The free-plugin browser is an overlay, like settings and export: finding a sound is
   *  something you do from wherever you are, not a reason to change what the window shows. */
  pluginBrowserOpen: boolean;
  openPluginBrowser(open?: boolean): void;

  // ── Guitar ──────────────────────────────────────────────────────────────────────────────
  addAudioTrack(): Promise<void>;
  /** Point the live input at one audio track (null disarms). */
  setArm(trackId: string | null): Promise<void>;
  toggleMonitor(): Promise<void>;
  /** Arm the recorder, then start the transport — so a count-in is never recorded. */
  startRecording(): Promise<void>;
  stopRecording(): Promise<void>;
  /** Pick a file and import it onto `trackId` (or a new audio track). */
  importAudioFile(trackId?: string | null): Promise<void>;
  /** Import a known path (dropped file). */
  importAudioPath(path: string, trackId?: string | null): Promise<void>;
  /** Transcribe one audio clip's raw DI: notes onto a new synth track and chord candidates
   *  into the detected lane, one undo step. Says what it wrote and what it refused. */
  detectNotes(clipId: string): Promise<void>;
  selectAudioClip(id: string | null): void;
  /** Merge a patch into the selected audio track's rig. `transient` while a knob is dragged. */
  setAmp(
    trackId: string,
    patch: Partial<AmpRig>,
    transient?: boolean,
  ): Promise<void>;
  refreshInputDevices(): Promise<void>;
  /** Open or close the tuner. Opening silences the engine; closing lets it speak again. */
  toggleTuner(): Promise<void>;
  /** Poll one reading from the engine. The strip drives this while it is open. */
  pollTuner(): Promise<void>;
  /** Acknowledge the input meter's peak hold and clip latch. */
  clearInputPeak(): Promise<void>;
  /** Set the manual latency trim, in ms, for one input device. */
  setInputTrim(device: string, ms: number): Promise<void>;
}

/** Push the appearance section into CSS variables, the live tier palette and the webview zoom. */
export function applyAppearance(a: Settings["appearance"]) {
  const root = document.documentElement;
  TIER_COLORS.chordTone = a.tierColors.chordTone;
  TIER_COLORS.tension = a.tierColors.tension;
  TIER_COLORS.dissonance = a.tierColors.dissonance;
  root.style.setProperty("--tier-solid", a.tierColors.chordTone);
  root.style.setProperty("--tier-tension", a.tierColors.tension);
  root.style.setProperty("--tier-dissonance", a.tierColors.dissonance);
  root.style.setProperty("--ok", a.tierColors.chordTone);
  root.style.setProperty("--accent", a.accent);
  const [r, g, b] = [1, 3, 5].map((i) =>
    parseInt(a.accent.slice(i, i + 2), 16),
  );
  root.style.setProperty("--accent-rgb", `${r}, ${g}, ${b}`);
  root.style.setProperty("--accent-bg", `rgba(${r}, ${g}, ${b}, 0.16)`);
  root.style.setProperty("--accent-border", `rgba(${r}, ${g}, ${b}, 0.45)`);
  void applyUiScale(a.uiScale);
  root.classList.toggle("reduced-motion", a.reducedMotion);
}

let toastTimer: ReturnType<typeof setTimeout> | null = null;
let taps: number[] = [];
let dialogResolve: ((value: string) => void) | null = null;
let lastTitle = "";

// ─── Zoom memory ───────────────────────────────────────────────────────────────────────────
//
// A zoom is how close the writer wants to stand to the music, not project data, so — like the
// panel sizes in `panels.ts` — it is remembered per machine in `localStorage` instead of being
// thrown away on every navigation and relaunch. `setup` is nullable because the song page's
// "fit" is a real choice, not a number.

export type ZoomId = "pattern" | "song" | "setup";

const ZOOM_RANGE: Record<ZoomId, [number, number]> = {
  pattern: [0.25, 3], // × the base beat width
  song: [12, 160], // px per bar
  setup: [4, 160], // px per beat
};
const ZOOM_KEY = "dissonant.zoom";
const zooms: Record<ZoomId, number | null> = { pattern: 1, song: 30, setup: null };

export function clampZoom(id: ZoomId, value: number): number {
  const [min, max] = ZOOM_RANGE[id];
  return Math.min(max, Math.max(min, value));
}

/** The remembered zoom, or null where the view's default is "fit". */
export function storedZoom(id: ZoomId): number | null {
  const v = zooms[id];
  return v === null ? null : clampZoom(id, v);
}

export function setStoredZoom(id: ZoomId, value: number | null) {
  const next = value === null ? null : clampZoom(id, value);
  if (next === zooms[id]) return;
  zooms[id] = next;
  try {
    localStorage.setItem(ZOOM_KEY, JSON.stringify(zooms));
  } catch {
    /* storage off: the zoom just won't outlive the session */
  }
}

// Nonsense in the file is ignored rather than thrown over: a bad zoom would blank the editor.
try {
  const saved = JSON.parse(localStorage.getItem(ZOOM_KEY) ?? "{}") as Partial<Record<ZoomId, unknown>>;
  for (const id of Object.keys(ZOOM_RANGE) as ZoomId[]) {
    const v = saved[id];
    if (typeof v === "number" && Number.isFinite(v)) zooms[id] = clampZoom(id, v);
    else if (v === null) zooms[id] = null;
  }
} catch {
  /* unreadable: the defaults stand */
}

/**
 * Every sound a track could pick, of one kind: factory, then the user library, then any sound
 * embedded in the project that isn't in the library (opened from another machine).
 */
export function soundChoices(st: { library: Sound[]; snapshot: Snapshot | null }, kind: "synth" | "drum"): { factory: Sound[]; library: Sound[]; project: Sound[] } {
  const of = (list: Sound[]) => list.filter((s) => s.patch.kind === kind);
  const library = of(st.library);
  const project = of(st.snapshot?.model.sounds ?? []).filter((s) => !library.some((l) => l.id === s.id));
  return { factory: of(FACTORY_SOUNDS), library, project };
}

/** The engine's playback mode for a screen: only PATTERN loops a pattern. */
export const playModeOf = (screen: Screen): PlayMode => (screen === "pattern" ? "pattern" : "song");

/** A track's patterns, in model order. */
export function patternsOf(m: ProjectModel, trackId: string): SongPattern[] {
  return m.patterns.filter((p) => p.trackId === trackId);
}

/**
 * The selection invariant over a (new) model: a surviving pattern brings its own track; a lost
 * pattern falls back to the selected track's first pattern; a lost track to the first track.
 */
export function reconcileSelection(m: ProjectModel, trackId: string | null, patternId: string | null): { selectedTrackId: string | null; selectedPatternId: string | null } {
  const pattern = m.patterns.find((p) => p.id === patternId);
  if (pattern) return { selectedTrackId: pattern.trackId, selectedPatternId: pattern.id };
  const track = m.tracks.find((t) => t.id === trackId) ?? m.tracks[0];
  if (!track) return { selectedTrackId: null, selectedPatternId: null };
  return { selectedTrackId: track.id, selectedPatternId: patternsOf(m, track.id)[0]?.id ?? null };
}

export const useStore = create<State>((set, get) => {
  /**
   * Tell the engine which loop brace is live. The two are in different time bases — the roll's
   * is in pattern ticks, the song's in song ticks — so only one can ever be loaded, and this is
   * the single place that picks. PATTERN plays a pattern; every other screen plays the song.
   */
  const syncLoop = async () => {
    const { screen, loopRegion, snapshot } = get();
    const region = screen === "pattern" ? loopRegion : (snapshot?.model.loopRegion ?? null);
    const b = await getBridge();
    if (region) await b.setLoop(region[0], region[1]);
    else await b.clearLoop();
  };

  /**
   * Tell the engine where the punch range is. It lives on the model like the song's brace, so
   * opening a project, undoing and dragging it all arrive through here. One time base only:
   * the punch is a place in the *song*, and PATTERN has nothing to punch into.
   */
  const syncPunch = async () => {
    const region = get().snapshot?.model.punchRegion ?? null;
    const b = await getBridge();
    if (region) await b.setPunch(region[0], region[1]);
    else await b.clearPunch();
  };

  const syncPlayback = async () => {
    const b = await getBridge();
    const { screen, selectedPatternId, chordOffsetTicks, ghostPatternId, hearGhost, ghostLevel } = get();
    const ghost = hearGhost && ghostPatternId !== selectedPatternId ? ghostPatternId : null;
    await b.setPlaybackContext(playModeOf(screen), selectedPatternId, chordOffsetTicks, ghost, ghostLevel);
    // The brace belongs to the sequence that was just loaded, so it travels with it: every
    // screen or project change goes through here.
    await syncLoop();
  };

  /**
   * Move the selection to `trackId` / `patternId` (already reconciled). A different pattern
   * drops the note selection, the loop region and the chord offset; a different track
   * becomes the live (MIDI / keyboard) track.
   */
  const moveSelection = (next: { selectedTrackId: string | null; selectedPatternId: string | null }, chordOffsetTicks?: number) => {
    const { selectedTrackId, selectedPatternId, chordOffsetTicks: before } = get();
    const patternChanged = next.selectedPatternId !== selectedPatternId;
    if (patternChanged && get().loopRegion) get().setLoopRegion(null);
    const offset = chordOffsetTicks ?? (patternChanged ? 0 : get().chordOffsetTicks);
    set({ ...next, chordOffsetTicks: offset, ...(patternChanged || next.selectedTrackId !== selectedTrackId ? { selectedNoteIds: [] } : {}) });
    if (next.selectedTrackId !== selectedTrackId) void getBridge().then((b) => b.setLiveTrack(next.selectedTrackId));
    return patternChanged || offset !== before;
  };

  /** What the user is seeing this insert called, for an undo label. Falls back to the plain
   *  word when the slot has already gone — an undo label is never worth an error. */
  const insertName = (target: InsertTarget, insertId: string) => {
    const m = get().snapshot?.model;
    const found = m && insertChain(m, target).find((i) => i.id === insertId);
    return found ? found.name : "insert";
  };

  const applySnapshot = (snapshot: Snapshot) => {
    const m = snapshot.model;
    const { selectedTrackId, selectedPatternId, selectedClipIds, ghostPatternId } = get();
    // Opening, undoing and the brace drag itself all arrive as snapshots, so the engine is
    // re-told from here rather than from each of them.
    const loopWas = get().snapshot?.model.loopRegion ?? null;
    const loopNow = m.loopRegion ?? null;
    const loopChanged = loopWas?.[0] !== loopNow?.[0] || loopWas?.[1] !== loopNow?.[1];
    const punchWas = get().snapshot?.model.punchRegion ?? null;
    const punchNow = m.punchRegion ?? null;
    const punchChanged = punchWas?.[0] !== punchNow?.[0] || punchWas?.[1] !== punchNow?.[1];
    const next = reconcileSelection(m, selectedTrackId, selectedPatternId);
    const patternLost = next.selectedPatternId !== selectedPatternId;
    const clipIds = new Set(m.clips.map((c) => c.id));
    const keptClips = selectedClipIds.filter((id) => clipIds.has(id));
    set({
      snapshot,
      ...(keptClips.length !== selectedClipIds.length ? { selectedClipIds: keptClips } : {}),
      ...(ghostPatternId && !m.patterns.some((p) => p.id === ghostPatternId) ? { ghostPatternId: null } : {}),
    });
    const ghostLost = ghostPatternId !== null && get().ghostPatternId === null;
    moveSelection(next);
    if (patternLost || ghostLost) void syncPlayback();
    if (loopChanged) void syncLoop();
    if (punchChanged) void syncPunch();
    // OS window title: "song.dissonant • — dissonant"
    const title = `${fileNameOf(snapshot.path)}${snapshot.dirty ? " •" : ""} — dissonant`;
    if (title !== lastTitle) {
      lastTitle = title;
      void getBridge().then((b) => b.setTitle(title));
    }
  };

  /**
   * A fresh, untitled starter or vibe also turns the chord bed on, so the first press of space
   * plays the progression with everything else. A no-op for a project without chords.
   */
  const hearBedForFresh = (snapshot: Snapshot) => {
    if (snapshot.path !== null || snapshot.dirty || get().hearChords) return;
    if (snapshot.model.chords.length === 0) return;
    set({ hearChords: true });
    void getBridge().then((b) => b.setHearChords(true));
  };

  /** A fresh project (new / starter / vibe / template) opens on SETUP. */
  const openFresh = async (snapshot: Snapshot) => {
    set({ screen: "setup", selectedClipIds: [] });
    applySnapshot(snapshot);
    await syncPlayback();
    hearBedForFresh(snapshot);
  };

  /** Offer to restore autosaves left by an earlier run, newest first. */
  const offerRecovery = async (candidates: RecoveryCandidate[]) => {
    const b = await getBridge();
    for (const c of candidates) {
      const when = new Date(c.savedAtMs).toLocaleString();
      const answer = await get().ask({
        title: "Recover unsaved work?",
        message: `An autosave of ${c.projectPath ? `“${fileNameOf(c.projectPath)}”` : "an untitled project"} from ${when} was found. It was never saved.`,
        detail: c.projectPath ?? undefined,
        buttons: [
          { label: "Restore", value: "restore", kind: "primary" },
          { label: "Delete autosave", value: "discard", kind: "danger" },
          { label: "Later", value: "later", kind: "quiet" },
        ],
        cancelValue: "later",
      });
      if (answer === "restore") {
        try {
          applySnapshot(await b.restoreAutosave(c.autosavePath));
          await syncPlayback();
          get().showToast("restored from autosave — save to keep it");
        } catch (e) {
          get().showToast(String(e), true);
        }
        return; // one document at a time; the rest stay on disk for next launch
      }
      if (answer === "discard") await b.discardAutosave(c.autosavePath);
    }
  };

  return {
    snapshot: null,
    library: [],
    selectedTrackId: null,
    selectedPatternId: null,
    chordOffsetTicks: 0,
    selectedClipIds: [],
    selectedNoteIds: [],
    noteClipboard: [],
    noteLength: 240,
    zoom: storedZoom("pattern") ?? 1,
    arrZoom: storedZoom("song") ?? 30,
    arrSnap: "bar",
    showLandscape: false,
    highlightRows: true,
    hearChords: false,
    bottomPanel: "devices",
    playhead: 0,
    playing: false,
    countIn: 0,
    masterPeak: [0, 0],
    trackPeaks: [],
    audio: null,
    toast: null,
    dialog: null,
    recent: [],
    settings: DEFAULT_SETTINGS,
    settingsOpen: false,
    pluginBrowserOpen: false,
    outputDevices: [],
    dialogText: "",
    exportOpen: false,
    exporting: null,
    templates: [],
    newProjectOpen: false,
    newProjectAtLaunch: false,
    vibes: [],
    looping: true,
    loopRegion: null,
    armed: false,
    recordQuantize: TICKS_PER_BEAT / 4,
    midi: null,
    midiActivityAt: 0,
    liveKeyboard: "off",
    magnet: false,
    stamp: false,
    brush: false,
    follow: true,
    fold: false,
    ghostPatternId: null,
    hearGhost: true,
    ghostLevel: 1,
    patternDrag: null,
    patternDropHandler: null,
    laneProp: "velocity",
    selectionRequest: null,
    plugins: [],
    catalog: [],
    installing: {},
    screen: "pattern",
    inputPeak: 0,
    recordingAudio: false,
    inputDevices: [],
    armedAudioTrackId: null,
    monitor: false,
    selectedAudioClipId: null,
    importing: null,
    detectingClipId: null,
    tunerOpen: false,
    tunerHz: null,
    tunerClarity: 0,

    async init() {
      const b = await getBridge();
      try {
        const settings = await b.getSettings();
        set({
          settings,
          noteLength: settings.editing.defaultGrid,
        });
        applyAppearance(settings.appearance);
      } catch (e) {
        console.warn("settings unavailable", e);
      }
      const first = await b.getState();
      // The project the app launches with is a fresh one unless it came from a file.
      if (first.path === null && !first.dirty) set({ screen: "setup" });
      applySnapshot(first);
      await syncPlayback();
      hearBedForFresh(first);
      await b.setLiveTrack(get().selectedTrackId);
      b.onPlayhead((e) =>
        set({
          playhead: e.tick,
          playing: e.playing,
          masterPeak: e.masterPeak,
          trackPeaks: e.trackPeaks,
          countIn: e.countIn ?? 0,
          inputPeak: e.inputPeak ?? 0,
          recordingAudio: e.recording ?? false,
        }),
      );
      b.onDocument((snap) => applySnapshot(snap));
      b.onMidiActivity(() => set({ midiActivityAt: performance.now() }));
      // Something the player needs to know about and did not ask for: a note the recorder
      // could not place, say. Shown as a warning, because it is one.
      b.onNotice((message) => get().showToast(message, true));
      b.onCloseRequested(() => void get().requestClose());
      b.onFileDropped((path) => void get().openPath(path));
      b.onAudioDropped((path) => void get().importAudioPath(path));
      b.onAudioImport((p) => set({ importing: p.fraction >= 1 ? null : p }));
      b.onAudioStatus((e) => {
        set({ audio: e.status });
        get().showToast(e.message, !e.status.running);
      });
      b.onMidiStatus((midi) => {
        const before = get().midi?.open ?? null;
        set({ midi });
        if (midi.open && midi.open !== before)
          get().showToast(`MIDI in: ${midi.open}`);
        else if (!midi.open && before)
          get().showToast(`MIDI in: ${before} disconnected`, true);
      });
      b.onExportProgress((p) =>
        set({ exporting: p.done >= p.total ? null : p }),
      );
      b.onPluginInstall((p) =>
        set((s) => ({ installing: { ...s.installing, [p.entryId]: p } })),
      );
      // Engine load / xruns in the status bar.
      setInterval(() => void get().refreshAudio(), 2000);
      const audio = await b.audioStatus();
      set({ audio });
      if (!audio.running && b.isTauri)
        get().showToast(`audio: ${audio.error ?? "not running"}`, true);
      void get().refreshMidi();
      void get().refreshRecent();
      void get().refreshPlugins();
      void get().refreshInputDevices();
      void get().loadLibrary();
      try {
        set({ vibes: await b.listVibes() });
      } catch {
        /* no vibes backend */
      }
      let recovering = false;
      try {
        const candidates = await b.recoveryCandidates();
        recovering = candidates.length > 0;
        if (recovering) void offerRecovery(candidates);
      } catch {
        /* no recovery backend */
      }
      // The new-project screen greets a fresh launch; a recovery offer takes its place.
      if (!recovering && get().settings.editing.showWelcome) {
        get().openNewProject(true);
        set({ newProjectAtLaunch: true });
      }
    },

    async dispatch(command, transient = false, label) {
      const b = await getBridge();
      try {
        applySnapshot(await b.apply(command, transient, label));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    async commitGesture() {
      const b = await getBridge();
      applySnapshot(await b.commitGesture());
    },

    async loadLibrary() {
      const b = await getBridge();
      try {
        set({ library: await b.librarySounds() });
      } catch (e) {
        console.warn("sound library unavailable", e);
      }
    },
    async saveLibrarySound(sound) {
      const b = await getBridge();
      set({ library: await b.saveLibrarySound(sound) });
      if (get().snapshot?.model.sounds?.some((s) => s.id === sound.id)) await get().dispatch({ type: "upsertSound", sound });
    },
    async deleteLibrarySound(id) {
      const b = await getBridge();
      // The project keeps its embedded copy, so songs using the sound still play.
      set({ library: await b.deleteLibrarySound(id) });
    },

    async applySoundPreset(trackId, preset) {
      // Three transient edits folded into one undo step by commitGesture, like a knob drag.
      const label = `sound: ${preset.name}`;
      await get().dispatch({ type: "setTrackVoice", id: trackId, voice: preset.voice }, true, label);
      await get().dispatch({ type: "setTrackParam", id: trackId, param: "tone", value: preset.tone }, true, label);
      await get().dispatch({ type: "setTrackParam", id: trackId, param: "reverbSend", value: preset.reverbSend }, true, label);
      await get().commitGesture();
    },

    async undo() {
      const b = await getBridge();
      applySnapshot(await b.undo());
    },

    async redo() {
      const b = await getBridge();
      applySnapshot(await b.redo());
    },

    setScreen(screen) {
      // SETUP, SONG, SOUND and the REC screen all play the song timeline; only PATTERN loops
      // a pattern, so the engine's mode only has to follow that one step.
      const before = playModeOf(get().screen);
      // The tuner lives on REC and mutes the engine while it is open; leaving the page has to
      // give the sound back, or the app is silent for reasons the user can no longer see.
      if (get().tunerOpen && screen !== "rec") void get().toggleTuner();
      set({ screen });
      if (playModeOf(screen) !== before) void syncPlayback();
    },
    selectTrack(id) {
      const m = get().snapshot?.model;
      if (!m || !m.tracks.some((t) => t.id === id)) return;
      const current = m.patterns.find((p) => p.id === get().selectedPatternId);
      const patternId = current?.trackId === id ? current.id : (patternsOf(m, id)[0]?.id ?? null);
      if (moveSelection({ selectedTrackId: id, selectedPatternId: patternId })) void syncPlayback();
    },
    selectPattern(id) {
      const p = get().snapshot?.model.patterns.find((x) => x.id === id);
      if (!p) return;
      if (moveSelection({ selectedTrackId: p.trackId, selectedPatternId: p.id })) void syncPlayback();
    },
    openPattern(patternId, atTick) {
      const p = get().snapshot?.model.patterns.find((x) => x.id === patternId);
      if (!p) return;
      const offset = Number.isFinite(atTick) ? Math.max(0, atTick!) : 0;
      if (offset !== get().chordOffsetTicks && get().loopRegion) get().setLoopRegion(null);
      moveSelection({ selectedTrackId: p.trackId, selectedPatternId: p.id }, offset);
      set({ screen: "pattern" });
      void syncPlayback();
    },
    selectClips(ids, additive = false) {
      set((s) => ({ selectedClipIds: additive ? [...new Set([...s.selectedClipIds, ...ids])] : [...new Set(ids)] }));
    },
    setSelectedNoteIds(ids) {
      set({ selectedNoteIds: ids });
    },
    setNoteClipboard(notes) {
      set({ noteClipboard: notes });
    },
    setNoteLength(ticks) {
      set({ noteLength: ticks });
    },
    setZoom(zoom) {
      const next = clampZoom("pattern", zoom);
      setStoredZoom("pattern", next);
      set({ zoom: next });
    },
    setArrZoom(px) {
      const next = clampZoom("song", px);
      setStoredZoom("song", next);
      set({ arrZoom: next });
    },
    setArrSnap(snap) {
      set({ arrSnap: snap });
    },
    toggleLandscape() {
      set((s) => ({ showLandscape: !s.showLandscape }));
    },
    toggleHighlight() {
      set((s) => ({ highlightRows: !s.highlightRows }));
    },
    toggleHearChords() {
      const on = !get().hearChords;
      set({ hearChords: on });
      void getBridge().then((b) => b.setHearChords(on));
    },
    setBottomPanel(p) {
      set({ bottomPanel: p });
    },

    play() {
      const { armed, settings, playhead, snapshot } = get();
      const m = settings.metronome;
      // An open tuner silences the engine, so pressing play with it open would look like a
      // dead transport. Tuning is a stopped activity; starting one ends the other.
      if (get().tunerOpen) void get().toggleTuner();
      void getBridge().then(async (b) => {
        if (armed && m.preRollBars > 0) {
          const bar = barTicksOf(snapshot?.model.timeSignature);
          await b.seek(Math.max(0, playhead - m.preRollBars * bar));
        }
        await b.play(armed ? m.countInBars : 0);
      });
    },
    stop() {
      void getBridge().then((b) => b.stop());
    },
    toggleMetronome() {
      const on = !get().settings.metronome.on;
      void get().updateSettings("metronome", { on });
    },
    togglePlay() {
      if (get().playing) get().stop();
      else get().play();
    },
    rewind() {
      void getBridge().then((b) => b.seek(0));
    },
    seek(tick) {
      void getBridge().then((b) => b.seek(tick));
    },

    tapTempo() {
      const now = performance.now();
      // A pause of 3 s starts a new run; only the last 8 taps count.
      taps = taps.filter((t) => now - t < 3000).slice(-7);
      taps.push(now);
      if (taps.length >= 2) {
        const intervals = taps.slice(1).map((t, i) => t - taps[i]);
        const avg = intervals.reduce((a, b) => a + b, 0) / intervals.length;
        const bpm = Math.round((60000 / avg) * 10) / 10;
        if (bpm >= 20 && bpm <= 300) {
          void get().dispatch({ type: "setTempo", bpm }, false, "tap tempo");
          get().showToast(`tap · ${bpm.toFixed(1)} bpm (${taps.length} taps)`);
        }
      } else {
        get().showToast("tap · keep tapping…");
      }
    },

    audition(pitch, velocity = 100, lengthTicks) {
      const { selectedTrackId: id, noteLength, snapshot } = get();
      if (!id) return;
      const bpm = snapshot?.model.tempo ?? 120;
      const seconds = (ticksToBeats(lengthTicks ?? noteLength) * 60) / bpm;
      void getBridge().then((b) => b.audition(id, pitch, velocity, seconds));
    },

    setLoopRegion(region) {
      set({ loopRegion: region });
      void syncLoop();
    },
    async setSongLoop(region) {
      const was = get().snapshot?.model.loopRegion ?? null;
      if (region?.[0] === was?.[0] && region?.[1] === was?.[1]) return;
      if (region && get().snapshot?.model.punchRegion) {
        get().showToast("loop is blocked while punch is armed — clear the punch first", true);
        return;
      }
      await get().dispatch({ type: "setLoopRegion", region });
    },
    async setPunch(region) {
      const was = get().snapshot?.model.punchRegion ?? null;
      if (region?.[0] === was?.[0] && region?.[1] === was?.[1]) return;
      await get().dispatch({ type: "setPunchRegion", region });
      // Say it once, where the user is looking, rather than quietly ignoring the loop.
      if (region && get().looping) get().showToast("punch armed — looping is off until you clear it");
    },
    toggleLooping() {
      const on = !get().looping;
      if (on && get().snapshot?.model.punchRegion) {
        get().showToast("loop is blocked while punch is armed — clear the punch first", true);
        return;
      }
      set({ looping: on });
      void getBridge().then((b) => b.setLooping(on));
    },
    toggleRecord() {
      const armed = !get().armed;
      set({ armed });
      void getBridge().then(async (b) =>
        set({ midi: await b.setRecord(armed, get().recordQuantize) }),
      );
    },
    setRecordQuantize(ticks) {
      set({ recordQuantize: ticks });
      if (get().armed) void getBridge().then((b) => b.setRecord(true, ticks));
    },
    async openMidi(name) {
      const b = await getBridge();
      try {
        set({ midi: await b.openMidiInput(name) });
        get().showToast(`MIDI in: ${get().midi?.open ?? "—"}`);
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async closeMidi() {
      const b = await getBridge();
      set({ midi: await b.closeMidiInput() });
    },
    async refreshMidi() {
      const b = await getBridge();
      try {
        set({ midi: await b.midiStatus() });
      } catch {
        /* no midi backend */
      }
    },
    setLiveKeyboard(mode) {
      set({ liveKeyboard: mode });
    },
    noteOn(pitch, velocity = 100) {
      void getBridge().then((b) => b.noteOn(pitch, velocity));
    },
    noteOff(pitch) {
      void getBridge().then((b) => b.noteOff(pitch));
    },
    toggleMagnet() {
      set((s) => ({ magnet: !s.magnet }));
    },
    toggleStamp() {
      set((s) => ({ stamp: !s.stamp }));
    },
    toggleBrush() {
      set((s) => ({ brush: !s.brush }));
    },
    toggleFollow() {
      set((s) => ({ follow: !s.follow }));
    },
    toggleFold() {
      set((s) => ({ fold: !s.fold }));
    },
    setGhostPattern(id) {
      set({ ghostPatternId: id });
      void syncPlayback();
    },
    toggleHearGhost() {
      set((s) => ({ hearGhost: !s.hearGhost }));
      void syncPlayback();
    },
    setGhostLevel(level) {
      if (!Number.isFinite(level)) return;
      set({ ghostLevel: Math.min(1.5, Math.max(0, level)) });
      void syncPlayback();
    },
    setPatternDrag(drag) {
      set({ patternDrag: drag });
    },
    setPatternDropHandler(fn) {
      set({ patternDropHandler: fn });
    },
    setLaneProp(prop) {
      set({ laneProp: prop });
    },
    requestSelection(ids) {
      set((s) => ({
        selectionRequest: { ids, nonce: (s.selectionRequest?.nonce ?? 0) + 1 },
      }));
    },
    transformSelection(fn, label) {
      const st = get();
      const pattern = selectedPattern(st);
      if (!pattern) return;
      const notes = pattern.notes;
      const next = fn(notes, new Set(st.selectedNoteIds));
      if (next === notes) return;
      void st.dispatch(
        { type: "setNotes", patternId: pattern.id, notes: next },
        false,
        label,
      );
    },

    async newProject() {
      const b = await getBridge();
      if (!(await get().confirmDiscard("creating a new project"))) return;
      // No explicit choice: the backend follows the "new project opens" setting.
      await openFresh(await b.newProject());
    },

    async openProject() {
      const b = await getBridge();
      if (!(await get().confirmDiscard("opening another project"))) return;
      const path = await b.pickOpenPath();
      if (!path) return;
      try {
        applySnapshot(await b.openProject(path));
        await syncPlayback();
        get().showToast(`opened ${fileNameOf(path)}`);
        void get().refreshRecent();
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    async openPath(path) {
      const b = await getBridge();
      if (path === get().snapshot?.path && !get().snapshot?.dirty) return;
      if (!(await get().confirmDiscard(`opening “${fileNameOf(path)}”`)))
        return;
      try {
        applySnapshot(await b.openProject(path));
        await syncPlayback();
        get().showToast(`opened ${fileNameOf(path)}`);
      } catch (e) {
        get().showToast(String(e), true);
      }
      void get().refreshRecent();
    },

    async saveProject(saveAs = false) {
      const b = await getBridge();
      let path: string | undefined;
      if (saveAs || !get().snapshot?.path) {
        const picked = await b.pickSavePath(
          fileNameOf(get().snapshot?.path),
          "dissonant",
        );
        if (!picked) return false;
        path = picked;
      }
      try {
        applySnapshot(await b.saveProject(path));
        get().showToast("saved");
        void get().refreshRecent();
        return true;
      } catch (e) {
        get().showToast(String(e), true);
        return false;
      }
    },

    async exportWav() {
      set({ exportOpen: true });
    },
    openExport(open = true) {
      set({ exportOpen: open });
    },
    async runExport(request) {
      const b = await getBridge();
      const base = fileNameOf(get().snapshot?.path).replace(/\.dissonant$/, "");
      const path = await b.pickSavePath(
        `${request.scope === "pattern" ? `${base} - ${selectedPattern(get())?.name ?? "pattern"}` : base}.wav`,
        "wav",
      );
      if (!path) return;
      set({
        exportOpen: false,
        exporting: {
          done: 0,
          total: request.stems ? 2 : 1,
          current: "rendering…",
        },
      });
      try {
        const out = await b.exportWav({ ...request, path });
        const what =
          out.files.length > 1
            ? `exported ${out.files.length} files next to ${fileNameOf(path)}`
            : `exported ${fileNameOf(path)}`;
        // A plugin that would not open is not a failed export, but the mix is not the one
        // being heard — say so rather than letting a wrong file look finished.
        const missing = out.missingPlugins;
        get().showToast(
          missing.length ? `${what} — without: ${missing.join(", ")}` : what,
          missing.length > 0,
        );
      } catch (e) {
        get().showToast(String(e), true);
      }
      set({ exporting: null });
    },
    async exportMidi() {
      const b = await getBridge();
      const st = get();
      const scope =
        st.screen !== "pattern" && st.snapshot?.model.clips.length
          ? "song"
          : "pattern";
      const base = fileNameOf(st.snapshot?.path).replace(/\.dissonant$/, "");
      const path = await b.pickSavePath(
        scope === "pattern"
          ? `${base} - ${selectedPattern(st)?.name ?? "pattern"}.mid`
          : `${base}.mid`,
        "mid",
      );
      if (!path) return;
      try {
        await b.exportMidi(path, scope);
        get().showToast(`exported ${fileNameOf(path)}`);
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async importMidi() {
      const b = await getBridge();
      const path = await b.pickOpenPath(["mid", "midi"], "MIDI file");
      if (!path) return;
      try {
        applySnapshot(await b.importMidi(path));
        get().showToast(`imported ${fileNameOf(path)} as new tracks`);
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    setDialogText(text) {
      set({ dialogText: text });
    },
    async askInput(title, message, placeholder, okLabel = "OK", initial = "") {
      set({ dialogText: initial });
      const v = await get().ask({
        title,
        message,
        input: { placeholder, value: initial },
        buttons: [
          { label: okLabel, value: "ok", kind: "primary" },
          { label: "Cancel", value: "cancel", kind: "quiet" },
        ],
        cancelValue: "cancel",
      });
      return v === "ok" ? get().dialogText.trim() || null : null;
    },
    openNewProject(open = true) {
      set({ newProjectOpen: open, newProjectAtLaunch: false });
      if (open) {
        void get().refreshTemplates();
        void get().refreshRecent();
      }
    },
    async newFromVibe(id) {
      const b = await getBridge();
      if (!(await get().confirmDiscard("creating a new project"))) return;
      set({ newProjectOpen: false, newProjectAtLaunch: false });
      try {
        await openFresh(await b.newFromVibe(id));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async refreshTemplates() {
      const b = await getBridge();
      try {
        set({ templates: await b.listTemplates() });
      } catch {
        /* no backend */
      }
    },
    async saveAsTemplate() {
      const b = await getBridge();
      const suggested = fileNameOf(get().snapshot?.path).replace(
        /\.dissonant$/,
        "",
      );
      const name = await get().askInput(
        "Save as template",
        "New projects can start from this one (tracks, chords, notes and mix included).",
        "template name",
        "Save",
        suggested === "untitled" ? "" : suggested,
      );
      if (!name) return;
      try {
        await b.saveTemplate(name);
        get().showToast(`template “${name}” saved`);
        void get().refreshTemplates();
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async deleteTemplate(name) {
      const b = await getBridge();
      if (
        !(await get().confirm(
          "Delete template?",
          `“${name}” will be removed.`,
          "Delete",
          true,
        ))
      )
        return;
      await b.deleteTemplate(name);
      void get().refreshTemplates();
    },
    async newFromTemplate(name) {
      const b = await getBridge();
      // At launch the open project already is a fresh starter: picking it just gets to work.
      const snap0 = get().snapshot;
      if (name === "starter" && get().newProjectAtLaunch && snap0 && snap0.path === null && !snap0.dirty && !snap0.canUndo) {
        set({ newProjectOpen: false, newProjectAtLaunch: false });
        get().setScreen("setup");
        return;
      }
      if (!(await get().confirmDiscard("creating a new project"))) return;
      set({ newProjectOpen: false, newProjectAtLaunch: false });
      try {
        if (name === "starter" || name === "empty")
          await openFresh(await b.newProject(name === "starter"));
        else await openFresh(await b.newFromTemplate(name));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    async restartAudio() {
      const b = await getBridge();
      const audio = await b.restartAudio();
      set({ audio });
      get().showToast(
        audio.running
          ? `audio running @ ${audio.sampleRate} Hz`
          : `audio: ${audio.error}`,
        !audio.running,
      );
    },

    showToast(text, error = false) {
      set({ toast: { text, error } });
      if (toastTimer) clearTimeout(toastTimer);
      toastTimer = setTimeout(() => set({ toast: null }), error ? 6000 : 2500);
    },

    ask(spec) {
      // One dialog at a time: a new one cancels the previous.
      dialogResolve?.(spec.cancelValue ?? "cancel");
      return new Promise<string>((resolve) => {
        dialogResolve = resolve;
        set({ dialog: spec });
      });
    },
    answerDialog(value) {
      const resolve = dialogResolve;
      dialogResolve = null;
      set({ dialog: null });
      resolve?.(value);
    },
    async confirm(title, message, okLabel = "OK", danger = false) {
      if (danger && !get().settings.editing.confirmDestructive) return true;
      const v = await get().ask({
        title,
        message,
        buttons: [
          { label: okLabel, value: "ok", kind: danger ? "danger" : "primary" },
          { label: "Cancel", value: "cancel", kind: "quiet" },
        ],
        cancelValue: "cancel",
      });
      return v === "ok";
    },
    async confirmDiscard(what) {
      const snap = get().snapshot;
      if (!snap?.dirty) return true;
      const v = await get().ask({
        title: "Unsaved changes",
        message: `“${fileNameOf(snap.path)}” has changes that will be lost by ${what}.`,
        buttons: [
          { label: "Save", value: "save", kind: "primary" },
          { label: "Discard", value: "discard", kind: "danger" },
          { label: "Cancel", value: "cancel", kind: "quiet" },
        ],
        cancelValue: "cancel",
      });
      if (v === "save") return get().saveProject(false);
      return v === "discard";
    },
    async requestClose() {
      const b = await getBridge();
      const snap = get().snapshot;
      if (!snap?.dirty) return b.quit();
      const v = await get().ask({
        title: "Save before closing?",
        message: `“${fileNameOf(snap.path)}” has unsaved changes.`,
        buttons: [
          { label: "Save", value: "save", kind: "primary" },
          { label: "Don’t save", value: "discard", kind: "danger" },
          { label: "Cancel", value: "cancel", kind: "quiet" },
        ],
        cancelValue: "cancel",
      });
      if (v === "cancel") return;
      if (v === "save" && !(await get().saveProject(false))) return;
      await b.quit();
    },
    async refreshRecent() {
      const b = await getBridge();
      try {
        set({ recent: await b.recentFiles() });
      } catch {
        /* no backend */
      }
    },
    async clearRecent() {
      const b = await getBridge();
      await b.clearRecent();
      set({ recent: [] });
    },

    openSettings(open = true) {
      set({ settingsOpen: open });
      if (open) {
        void get().refreshOutputDevices();
        void get().refreshMidi();
      }
    },
    async updateSettings(section, patch) {
      const b = await getBridge();
      const next: Settings = {
        ...get().settings,
        [section]: { ...get().settings[section], ...patch },
      };
      set({ settings: next });
      if (section === "appearance") applyAppearance(next.appearance);
      if (section === "metronome")
        void b.setMetronome(next.metronome.on, next.metronome.volume);
      try {
        const audio = await b.setSettings(next);
        if (b.isTauri) set({ audio });
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async refreshOutputDevices() {
      const b = await getBridge();
      try {
        set({ outputDevices: await b.outputDevices() });
      } catch {
        /* no backend */
      }
    },
    testTone() {
      void getBridge().then((b) => b.testTone());
    },
    clearClip() {
      // Clear the local copy at once so the indicator responds to the click; the 2 s status
      // poll would otherwise leave it lit, and the engine re-latches on the next over anyway.
      const audio = get().audio;
      if (audio) set({ audio: { ...audio, masterOver: false } });
      void getBridge().then((b) => b.clearClip());
    },
    async refreshAudio() {
      const b = await getBridge();
      if (!b.isTauri) return;
      try {
        set({ audio: await b.audioStatus() });
      } catch {
        /* ignore */
      }
    },

    async refreshPlugins(rescan = false) {
      const b = await getBridge();
      try {
        set({ plugins: await b.scanPlugins(rescan) });
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async setTrackPlugin(trackId, uid) {
      const b = await getBridge();
      try {
        await b.setTrackPlugin(trackId, uid);
        applySnapshot(await b.getState());
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async refreshCatalog() {
      const b = await getBridge();
      try {
        set({ catalog: await b.pluginCatalog() });
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async pluginParams(trackId) {
      const b = await getBridge();
      try {
        return await b.pluginParams(trackId);
      } catch (e) {
        get().showToast(String(e), true);
        return [];
      }
    },
    async setPluginParam(trackId, paramId, value, commit = false) {
      const b = await getBridge();
      try {
        await b.setPluginParam(trackId, paramId, value, commit);
        // A committed value is a document edit; the dragging ones never touch it.
        if (commit) applySnapshot(await b.getState());
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async installPlugin(entryId) {
      const b = await getBridge();
      try {
        await b.installPlugin(entryId);
        await get().refreshPlugins(true);
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async openUrl(url) {
      // Only https: handing an arbitrary string to the OS opener is a shell-adjacent surface,
      // and the backend refuses anything else too.
      if (!url.startsWith("https://")) return;
      const b = await getBridge();
      try {
        await b.openUrl(url);
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async openPluginWindow(trackId) {
      const b = await getBridge();
      try {
        await b.openPluginWindow(trackId);
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    openPluginBrowser(open = true) {
      set({ pluginBrowserOpen: open });
    },

    // ── Effect inserts ────────────────────────────────────────────────────────────────────
    // Every one of these names the effect in its undo label, so the Edit menu reads
    // "undo bypass Airwindows Consolidated" rather than "undo bypass insert".
    async addInsert(target, uid, index = null) {
      const b = await getBridge();
      const name =
        nativeEffect(uid)?.name ?? get().plugins.find((p) => p.uid === uid)?.name ?? "effect";
      try {
        applySnapshot(await b.addInsert(target, uid, index, `add ${name}`));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async removeInsert(target, insertId) {
      const b = await getBridge();
      const label = `remove ${insertName(target, insertId)}`;
      try {
        applySnapshot(await b.removeInsert(target, insertId, label));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async moveInsert(target, insertId, toIndex) {
      const b = await getBridge();
      const label = `reorder ${insertName(target, insertId)}`;
      try {
        applySnapshot(await b.moveInsert(target, insertId, toIndex, label));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async setInsertEnabled(target, insertId, on) {
      const b = await getBridge();
      const label = `${on ? "enable" : "bypass"} ${insertName(target, insertId)}`;
      try {
        applySnapshot(await b.setInsertEnabled(target, insertId, on, label));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },
    async insertParams(insertId) {
      const b = await getBridge();
      try {
        return await b.insertParams(insertId);
      } catch (e) {
        get().showToast(String(e), true);
        return [];
      }
    },
    async setInsertParam(insertId, paramId, value, commit = false) {
      const b = await getBridge();
      try {
        // A committed value is a document edit and comes back as a snapshot; a dragged one
        // only reaches the effect and answers null.
        const s = await b.setInsertParam(insertId, paramId, value, commit);
        if (s) applySnapshot(s);
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    // ── Guitar ────────────────────────────────────────────────────────────────────────────

    async addAudioTrack() {
      const n =
        (get().snapshot?.model.tracks.filter((t) => t.isAudio).length ?? 0) + 1;
      await get().dispatch({
        type: "addAudioTrack",
        name: n === 1 ? "guitar" : `guitar ${n}`,
      });
      const tracks = get().snapshot?.model.tracks ?? [];
      const added = tracks[tracks.length - 1];
      if (added?.isAudio) {
        set({ selectedTrackId: added.id });
        await get().setArm(added.id);
      }
    },

    async setArm(trackId) {
      const b = await getBridge();
      try {
        const audio = await b.setAudioArm(trackId, get().monitor);
        set({ armedAudioTrackId: trackId, audio });
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    async toggleMonitor() {
      const monitor = !get().monitor;
      set({ monitor });
      const b = await getBridge();
      try {
        set({ audio: await b.setAudioArm(get().armedAudioTrackId, monitor) });
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    async startRecording() {
      const b = await getBridge();
      try {
        await b.startAudioRecord();
      } catch (e) {
        get().showToast(String(e), true);
        return;
      }
      const punch = get().snapshot?.model.punchRegion ?? null;
      if (!punch) {
        // Arming first and playing second is what keeps the count-in out of the take.
        get().play();
        return;
      }
      // A punch drops in at its own point, so the run-up is measured back from there rather
      // than from wherever the playhead was parked — otherwise the take starts after the
      // drop-in and never happens. Pre-roll and count-in are the metronome's own settings;
      // punch does not get a second pair.
      if (get().tunerOpen) void get().toggleTuner();
      const m = get().settings.metronome;
      const bar = barTicksOf(get().snapshot?.model.timeSignature);
      await b.seek(Math.max(0, punch[0] - m.preRollBars * bar));
      await b.play(m.countInBars);
    },

    async stopRecording() {
      const b = await getBridge();
      get().stop();
      try {
        const snap = await b.stopAudioRecord();
        applySnapshot(snap);
        const clips = snap.model.audioClips ?? [];
        if (clips.length)
          set({ selectedAudioClipId: clips[clips.length - 1].id });
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    async importAudioFile(trackId = null) {
      const b = await getBridge();
      const path = await b.pickOpenPath(
        ["wav", "wave", "aif", "aiff", "flac", "mp3", "m4a", "aac"],
        "audio file",
      );
      if (!path) return;
      await get().importAudioPath(path, trackId);
    },

    async importAudioPath(path, trackId = null) {
      const b = await getBridge();
      // Drop onto the selected audio track when there is one, so a drag does the obvious thing.
      const selected = get().snapshot?.model.tracks.find(
        (t) => t.id === get().selectedTrackId,
      );
      const target = trackId ?? (selected?.isAudio ? selected.id : null);
      try {
        applySnapshot(await b.importAudio(path, target, 0));
        set({ screen: "rec", importing: null });
        await syncPlayback();
        get().showToast(`imported ${fileNameOf(path)}`);
      } catch (e) {
        set({ importing: null });
        get().showToast(String(e), true);
      }
    },

    async detectNotes(clipId) {
      if (get().detectingClipId !== null) return;
      const b = await getBridge();
      set({ detectingClipId: clipId });
      try {
        const report = await b.detectAudioClip(clipId);
        applySnapshot(report.snapshot);
        await syncPlayback();
        const track = report.trackId
          ? report.snapshot.model.tracks.find((t) => t.id === report.trackId)
          : undefined;
        get().showToast(detectionSummary(report, track?.name ?? null));
      } catch (e) {
        get().showToast(String(e), true);
      } finally {
        set({ detectingClipId: null });
      }
    },

    selectAudioClip(id) {
      set({ selectedAudioClipId: id });
    },

    async setAmp(trackId, patch, transient = false) {
      const track = get().snapshot?.model.tracks.find((t) => t.id === trackId);
      const amp = track?.amp;
      if (!amp) return;
      await get().dispatch(
        { type: "setTrackAmp", id: trackId, amp: { ...amp, ...patch } },
        transient,
      );
    },

    async refreshInputDevices() {
      const b = await getBridge();
      try {
        set({ inputDevices: await b.inputDevices() });
      } catch {
        set({ inputDevices: [] });
      }
    },

    async toggleTuner() {
      const open = !get().tunerOpen;
      // Clear the reading on both edges: a stale needle under a new string is worse than no
      // needle, and closing should leave nothing behind for the next open to inherit.
      set({ tunerOpen: open, tunerHz: null, tunerClarity: 0 });
      const b = await getBridge();
      try {
        await b.setTuner(open);
      } catch (e) {
        set({ tunerOpen: false });
        get().showToast(String(e), true);
      }
    },

    async pollTuner() {
      if (!get().tunerOpen) return;
      const b = await getBridge();
      try {
        const r = await b.tunerReading();
        set({ tunerHz: r?.hz ?? null, tunerClarity: r?.clarity ?? 0 });
      } catch {
        set({ tunerHz: null, tunerClarity: 0 });
      }
    },

    async clearInputPeak() {
      // Drop the local copy first so the strip answers the click; the status poll would
      // otherwise leave it lit for a second, and the engine re-latches on the next over.
      const audio = get().audio;
      if (audio) set({ audio: { ...audio, inputPeakHold: 0, inputOver: false } });
      const b = await getBridge();
      try {
        set({ audio: await b.clearInputPeak() });
      } catch {
        /* no backend */
      }
    },

    async setInputTrim(device, ms) {
      if (!device) return;
      const trims = { ...get().settings.audio.inputTrims, [device]: ms };
      await get().updateSettings("audio", { inputTrims: trims });
    },
  };
});

// ─── Selectors / helpers ───────────────────────────────────────────────────────────────────

/** The selected pattern, always on the selected track; null when that track has no patterns. */
export function selectedPattern(s: State): SongPattern | null {
  const m = s.snapshot?.model;
  if (!m) return null;
  return m.patterns.find((p) => p.id === s.selectedPatternId) ?? null;
}

/** The project chords the selected pattern is heard and tiered against, from
 *  `chordOffsetTicks` (non-hook). */
export function patternChords(s: State): ChordEvent[] {
  const m = s.snapshot?.model;
  const p = selectedPattern(s);
  if (!m || !p) return [];
  return chordsInWindow(m.chords, s.chordOffsetTicks, p.lengthTicks);
}

/** `patternChords` as a hook, memoized so it is stable between renders (safe in effect deps). */
export function usePatternChords(): ChordEvent[] {
  const chords = useStore((s) => s.snapshot?.model.chords);
  const offset = useStore((s) => s.chordOffsetTicks);
  const length = useStore((s) => selectedPattern(s)?.lengthTicks ?? null);
  return useMemo(
    () => (chords && length !== null ? chordsInWindow(chords, offset, length) : []),
    [chords, offset, length],
  );
}

export function selectedTrack(s: State): Track | null {
  const m = s.snapshot?.model;
  if (!m) return null;
  return (
    m.tracks.find((t) => t.id === s.selectedTrackId) ?? m.tracks[0] ?? null
  );
}

/**
 * The audio track the REC page is showing: the armed one, else the selected one when it is an
 * audio track, else the first.
 *
 * The arm comes first because REC is about what you are about to play into, and the arm is the
 * only thing that decides where a take lands — a page that showed the selected track while
 * recording into another one would be actively misleading. One rule, so the page and the
 * status bar never disagree about it.
 */
export function recTrack(s: State): Track | null {
  const tracks = s.snapshot?.model.tracks.filter((t) => t.isAudio) ?? [];
  return (
    tracks.find((t) => t.id === s.armedAudioTrackId) ??
    tracks.find((t) => t.id === s.selectedTrackId) ??
    tracks[0] ??
    null
  );
}

export function selectedTrackIndex(s: State): number {
  const m = s.snapshot?.model;
  if (!m) return 0;
  const i = m.tracks.findIndex((t) => t.id === s.selectedTrackId);
  return i < 0 ? 0 : i;
}

/**
 * Every snap the writer can pick, smallest first. The tick grid makes the odd ones exact: at
 * 960 PPQ a triplet of a 1/8 is 320 ticks and a dotted 1/16 is 360, so nothing here rounds.
 * A `T` suffix is a triplet (two thirds of the straight value), a `.` is dotted (one and a half).
 */
export const GRID_OPTIONS: [string, number][] = [
  ["1/32", 120],
  ["1/16T", 160],
  ["1/16", 240],
  ["1/8T", 320],
  ["1/16.", 360],
  ["1/8", 480],
  ["1/4T", 640],
  ["1/8.", 720],
  ["1/4", 960],
  ["1/2T", 1280],
  ["1/4.", 1440],
  ["1/2", 1920],
  ["1/2.", 2880],
  ["1", 3840],
];

export function gridLabel(ticks: number): string {
  return GRID_OPTIONS.find(([, v]) => v === ticks)?.[0] ?? `${ticks}`;
}

export function dbText(gain: number): string {
  if (gain <= 0.0005) return "-inf";
  const db = 20 * Math.log10(gain);
  return `${db >= 0 ? "+" : ""}${db.toFixed(1)}`;
}

export function peakDb(peak: number): number {
  return peak <= 0.0001 ? -60 : 20 * Math.log10(peak);
}

/** 0..1 meter position for a peak, on a -48 dB … 0 dB scale. */
export function meterPos(peak: number): number {
  return Math.min(1, Math.max(0, (peakDb(peak) + 48) / 48));
}

export function beatsPerBar(s: State): number {
  return bpbOf(s.snapshot?.model.timeSignature);
}

/** Ticks per bar at the project's time signature. */
export function barTicks(s: State): number {
  return barTicksOf(s.snapshot?.model.timeSignature);
}

/**
 * The key the selected pattern is tiered against: the key of the section at
 * `chordOffsetTicks` (where the pattern was opened), or the project key.
 */
export function effectiveKey(s: State): KeyState {
  const m = s.snapshot?.model;
  if (!m) return { rootPitchClass: null, scale: "major", isLocked: false };
  return keyAt(m, s.chordOffsetTicks);
}
