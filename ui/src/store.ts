import { useMemo } from "react";
import { create } from "zustand";
import { getBridge } from "./bridge";
import type { AudioStatus, ChordEvent, Command, ExportProgress, ExportRequest, KeyState, MidiStatus, NoteEvent, OutputDevice, PlayMode, ProjectModel, RecoveryCandidate, Settings, Snapshot, SongPattern, SoundPreset, TemplateInfo, Track, VibeInfo, ViewMode } from "./types";
import { beatsPerBar as bpbOf, chordsInWindow, DEFAULT_SETTINGS, fileNameOf, keyAt, TIER_COLORS } from "./types";
import { applyUiScale } from "./uiScale";
import { FACTORY_SOUNDS, type Sound } from "./sound";

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
  /** SETUP and SONG both play the song; PATTERN loops the selected pattern. */
  mode: ViewMode;
  /** Invariant: the selected pattern (if any) belongs to the selected track. */
  selectedTrackId: string | null;
  selectedPatternId: string | null;
  /** The song beat the edited pattern's bar 1 sits on: its chords and section key come from there. */
  chordOffsetBeats: number;
  /** Clips selected in the song view. */
  selectedClipIds: string[];
  selectedNoteIds: string[];
  noteLength: number;
  zoom: number;
  arrZoom: number; // px per bar in the arrangement
  showLandscape: boolean;
  highlightRows: boolean;
  hearChords: boolean;
  bottomPanel: BottomPanel;
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
  loopRegion: [number, number] | null;
  armed: boolean;
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
  /** Something outside the roll (inspector, menu) asked for this selection. */
  selectionRequest: { ids: string[]; nonce: number } | null;

  init(): Promise<void>;
  /** `label` names the edit in the Edit menu ("move notes"); defaults to the command's name. */
  dispatch(command: Command, transient?: boolean, label?: string): Promise<void>;
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
  /** SETUP and SONG put the engine in song playback; PATTERN in pattern playback. */
  setMode(mode: ViewMode): void;
  /** Also selects that track's first pattern (or keeps the selected one if it's on that track), or none. */
  selectTrack(id: string): void;
  /** Also selects its track. */
  selectPattern(id: string): void;
  /** Select, set `chordOffsetBeats` to `atBeat` (a clip's start; 0 = song bar 1) and switch to PATTERN. */
  openPattern(patternId: string, atBeat?: number): void;
  /** Replace the clip selection, or add to it with `additive`. */
  selectClips(ids: string[], additive?: boolean): void;
  setSelectedNoteIds(ids: string[]): void;
  setNoteLength(len: number): void;
  setZoom(zoom: number): void;
  setArrZoom(px: number): void;
  toggleLandscape(): void;
  toggleHighlight(): void;
  toggleHearChords(): void;
  setBottomPanel(p: BottomPanel): void;
  play(): void;
  stop(): void;
  toggleMetronome(): void;
  togglePlay(): void;
  rewind(): void;
  seek(beat: number): void;
  tapTempo(): void;
  /** Preview a pitch. `beats` defaults to the current grid, so a preview never outlasts the note. */
  audition(pitch: number, velocity?: number, beats?: number): void;
  setLoopRegion(region: [number, number] | null): void;
  toggleLooping(): void;
  toggleRecord(): void;
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
  requestSelection(ids: string[]): void;
  /** Apply a pure edit to the selected pattern's notes (ids = current selection). */
  transformSelection(fn: (notes: NoteEvent[], ids: Set<string>) => NoteEvent[], label: string): void;
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
  askInput(title: string, message: string, placeholder?: string, okLabel?: string, initial?: string): Promise<string | null>;
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
  confirm(title: string, message?: string, okLabel?: string, danger?: boolean): Promise<boolean>;
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
  updateSettings<K extends keyof Settings>(section: K, patch: Partial<Settings[K]>): Promise<void>;
  refreshOutputDevices(): Promise<void>;
  testTone(): void;
  refreshAudio(): Promise<void>;
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
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(a.accent.slice(i, i + 2), 16));
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

/** The engine's playback mode for a view: only PATTERN loops a pattern. */
export const playModeOf = (mode: ViewMode): PlayMode => (mode === "pattern" ? "pattern" : "song");

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
  const syncPlayback = async () => {
    const b = await getBridge();
    const { mode, selectedPatternId, chordOffsetBeats, ghostPatternId, hearGhost, ghostLevel } = get();
    const ghost = hearGhost && ghostPatternId !== selectedPatternId ? ghostPatternId : null;
    await b.setPlaybackContext(playModeOf(mode), selectedPatternId, chordOffsetBeats, ghost, ghostLevel);
  };

  /**
   * Move the selection to `trackId` / `patternId` (already reconciled). A different pattern
   * drops the note selection, the loop region and the chord offset; a different track
   * becomes the live (MIDI / keyboard) track.
   */
  const moveSelection = (next: { selectedTrackId: string | null; selectedPatternId: string | null }, chordOffsetBeats?: number) => {
    const { selectedTrackId, selectedPatternId, chordOffsetBeats: before } = get();
    const patternChanged = next.selectedPatternId !== selectedPatternId;
    if (patternChanged && get().loopRegion) get().setLoopRegion(null);
    const offset = chordOffsetBeats ?? (patternChanged ? 0 : get().chordOffsetBeats);
    set({ ...next, chordOffsetBeats: offset, ...(patternChanged || next.selectedTrackId !== selectedTrackId ? { selectedNoteIds: [] } : {}) });
    if (next.selectedTrackId !== selectedTrackId) void getBridge().then((b) => b.setLiveTrack(next.selectedTrackId));
    return patternChanged || offset !== before;
  };

  const applySnapshot = (snapshot: Snapshot) => {
    const m = snapshot.model;
    const { selectedTrackId, selectedPatternId, selectedClipIds, ghostPatternId } = get();
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
    set({ mode: "setup", selectedClipIds: [] });
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
    mode: "pattern",
    selectedTrackId: null,
    selectedPatternId: null,
    chordOffsetBeats: 0,
    selectedClipIds: [],
    selectedNoteIds: [],
    noteLength: 0.25,
    zoom: 1,
    arrZoom: 30,
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
    selectionRequest: null,

    async init() {
      const b = await getBridge();
      try {
        const settings = await b.getSettings();
        set({ settings, noteLength: settings.editing.defaultGrid });
        applyAppearance(settings.appearance);
      } catch (e) {
        console.warn("settings unavailable", e);
      }
      const first = await b.getState();
      // The project the app launches with is a fresh one unless it came from a file.
      if (first.path === null && !first.dirty) set({ mode: "setup" });
      applySnapshot(first);
      await syncPlayback();
      hearBedForFresh(first);
      await b.setLiveTrack(get().selectedTrackId);
      b.onPlayhead((e) => set({ playhead: e.beat, playing: e.playing, masterPeak: e.masterPeak, trackPeaks: e.trackPeaks, countIn: e.countIn ?? 0 }));
      b.onDocument((snap) => applySnapshot(snap));
      b.onMidiActivity(() => set({ midiActivityAt: performance.now() }));
      b.onCloseRequested(() => void get().requestClose());
      b.onFileDropped((path) => void get().openPath(path));
      b.onAudioStatus((e) => {
        set({ audio: e.status });
        get().showToast(e.message, !e.status.running);
      });
      b.onMidiStatus((midi) => {
        const before = get().midi?.open ?? null;
        set({ midi });
        if (midi.open && midi.open !== before) get().showToast(`MIDI in: ${midi.open}`);
        else if (!midi.open && before) get().showToast(`MIDI in: ${before} disconnected`, true);
      });
      b.onExportProgress((p) => set({ exporting: p.done >= p.total ? null : p }));
      // Engine load / xruns in the status bar.
      setInterval(() => void get().refreshAudio(), 2000);
      const audio = await b.audioStatus();
      set({ audio });
      if (!audio.running && b.isTauri) get().showToast(`audio: ${audio.error ?? "not running"}`, true);
      void get().refreshMidi();
      void get().refreshRecent();
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

    setMode(mode) {
      const before = playModeOf(get().mode);
      set({ mode });
      if (playModeOf(mode) !== before) void syncPlayback();
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
    openPattern(patternId, atBeat) {
      const p = get().snapshot?.model.patterns.find((x) => x.id === patternId);
      if (!p) return;
      const offset = Number.isFinite(atBeat) ? Math.max(0, atBeat!) : 0;
      if (offset !== get().chordOffsetBeats && get().loopRegion) get().setLoopRegion(null);
      moveSelection({ selectedTrackId: p.trackId, selectedPatternId: p.id }, offset);
      set({ mode: "pattern" });
      void syncPlayback();
    },
    selectClips(ids, additive = false) {
      set((s) => ({ selectedClipIds: additive ? [...new Set([...s.selectedClipIds, ...ids])] : [...new Set(ids)] }));
    },
    setSelectedNoteIds(ids) {
      set({ selectedNoteIds: ids });
    },
    setNoteLength(len) {
      set({ noteLength: len });
      if (get().armed) void getBridge().then((b) => b.setRecord(true, len));
    },
    setZoom(zoom) {
      set({ zoom: Math.min(3, Math.max(0.25, zoom)) });
    },
    setArrZoom(px) {
      set({ arrZoom: Math.min(160, Math.max(12, px)) });
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
      void getBridge().then(async (b) => {
        if (armed && m.preRollBars > 0) {
          const bpb = bpbOf(snapshot?.model.timeSignature);
          await b.seek(Math.max(0, playhead - m.preRollBars * bpb));
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
    seek(beat) {
      void getBridge().then((b) => b.seek(beat));
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

    audition(pitch, velocity = 100, beats) {
      const { selectedTrackId: id, noteLength, snapshot } = get();
      if (!id) return;
      const bpm = snapshot?.model.tempo ?? 120;
      const seconds = ((beats ?? noteLength) * 60) / bpm;
      void getBridge().then((b) => b.audition(id, pitch, velocity, seconds));
    },

    setLoopRegion(region) {
      set({ loopRegion: region });
      void getBridge().then((b) => (region ? b.setLoop(region[0], region[1]) : b.clearLoop()));
    },
    toggleLooping() {
      const on = !get().looping;
      set({ looping: on });
      void getBridge().then((b) => b.setLooping(on));
    },
    toggleRecord() {
      const armed = !get().armed;
      set({ armed });
      void getBridge().then(async (b) => set({ midi: await b.setRecord(armed, get().noteLength) }));
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
    requestSelection(ids) {
      set((s) => ({ selectionRequest: { ids, nonce: (s.selectionRequest?.nonce ?? 0) + 1 } }));
    },
    transformSelection(fn, label) {
      const st = get();
      const pattern = selectedPattern(st);
      if (!pattern) return;
      const notes = pattern.notes;
      const next = fn(notes, new Set(st.selectedNoteIds));
      if (next === notes) return;
      void st.dispatch({ type: "setNotes", patternId: pattern.id, notes: next }, false, label);
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
      if (!(await get().confirmDiscard(`opening “${fileNameOf(path)}”`))) return;
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
        const picked = await b.pickSavePath(fileNameOf(get().snapshot?.path), "dissonant");
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
      const path = await b.pickSavePath(`${request.scope === "pattern" ? `${base} - ${selectedPattern(get())?.name ?? "pattern"}` : base}.wav`, "wav");
      if (!path) return;
      set({ exportOpen: false, exporting: { done: 0, total: request.stems ? 2 : 1, current: "rendering…" } });
      try {
        const out = await b.exportWav({ ...request, path });
        get().showToast(out.files.length > 1 ? `exported ${out.files.length} files next to ${fileNameOf(path)}` : `exported ${fileNameOf(path)}`);
      } catch (e) {
        get().showToast(String(e), true);
      }
      set({ exporting: null });
    },
    async exportMidi() {
      const b = await getBridge();
      const st = get();
      const scope = st.mode !== "pattern" && st.snapshot?.model.clips.length ? "song" : "pattern";
      const base = fileNameOf(st.snapshot?.path).replace(/\.dissonant$/, "");
      const path = await b.pickSavePath(scope === "pattern" ? `${base} - ${selectedPattern(st)?.name ?? "pattern"}.mid` : `${base}.mid`, "mid");
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
      const suggested = fileNameOf(get().snapshot?.path).replace(/\.dissonant$/, "");
      const name = await get().askInput("Save as template", "New projects can start from this one (tracks, chords, notes and mix included).", "template name", "Save", suggested === "untitled" ? "" : suggested);
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
      if (!(await get().confirm("Delete template?", `“${name}” will be removed.`, "Delete", true))) return;
      await b.deleteTemplate(name);
      void get().refreshTemplates();
    },
    async newFromTemplate(name) {
      const b = await getBridge();
      // At launch the open project already is a fresh starter: picking it just gets to work.
      const snap0 = get().snapshot;
      if (name === "starter" && get().newProjectAtLaunch && snap0 && snap0.path === null && !snap0.dirty && !snap0.canUndo) {
        set({ newProjectOpen: false, newProjectAtLaunch: false });
        get().setMode("setup");
        return;
      }
      if (!(await get().confirmDiscard("creating a new project"))) return;
      set({ newProjectOpen: false, newProjectAtLaunch: false });
      try {
        if (name === "starter" || name === "empty") await openFresh(await b.newProject(name === "starter"));
        else await openFresh(await b.newFromTemplate(name));
      } catch (e) {
        get().showToast(String(e), true);
      }
    },

    async restartAudio() {
      const b = await getBridge();
      const audio = await b.restartAudio();
      set({ audio });
      get().showToast(audio.running ? `audio running @ ${audio.sampleRate} Hz` : `audio: ${audio.error}`, !audio.running);
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
      const next: Settings = { ...get().settings, [section]: { ...get().settings[section], ...patch } };
      set({ settings: next });
      if (section === "appearance") applyAppearance(next.appearance);
      if (section === "metronome") void b.setMetronome(next.metronome.on, next.metronome.volume);
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
    async refreshAudio() {
      const b = await getBridge();
      if (!b.isTauri) return;
      try {
        set({ audio: await b.audioStatus() });
      } catch {
        /* ignore */
      }
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

/** The project chords the selected pattern is heard and tiered against, from `chordOffsetBeats` (non-hook). */
export function patternChords(s: State): ChordEvent[] {
  const m = s.snapshot?.model;
  const p = selectedPattern(s);
  if (!m || !p) return [];
  return chordsInWindow(m.chords, s.chordOffsetBeats, p.lengthBeats);
}

/** `patternChords` as a hook, memoized so it is stable between renders (safe in effect deps). */
export function usePatternChords(): ChordEvent[] {
  const chords = useStore((s) => s.snapshot?.model.chords);
  const offset = useStore((s) => s.chordOffsetBeats);
  const length = useStore((s) => selectedPattern(s)?.lengthBeats ?? null);
  return useMemo(() => (chords && length !== null ? chordsInWindow(chords, offset, length) : []), [chords, offset, length]);
}

export function selectedTrack(s: State): Track | null {
  const m = s.snapshot?.model;
  if (!m) return null;
  return m.tracks.find((t) => t.id === s.selectedTrackId) ?? m.tracks[0] ?? null;
}

export function selectedTrackIndex(s: State): number {
  const m = s.snapshot?.model;
  if (!m) return 0;
  const i = m.tracks.findIndex((t) => t.id === s.selectedTrackId);
  return i < 0 ? 0 : i;
}

export const GRID_OPTIONS: [string, number][] = [
  ["1/32", 0.125],
  ["1/16", 0.25],
  ["1/8", 0.5],
  ["1/4", 1],
  ["1/2", 2],
  ["1", 4],
];

export function gridLabel(len: number): string {
  return GRID_OPTIONS.find(([, v]) => v === len)?.[0] ?? `${len}`;
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

/**
 * The key the selected pattern is tiered against: the key of the section at
 * `chordOffsetBeats` (where the pattern was opened), or the project key.
 */
export function effectiveKey(s: State): KeyState {
  const m = s.snapshot?.model;
  if (!m) return { rootPitchClass: null, scale: "major", isLocked: false };
  return keyAt(m, s.chordOffsetBeats);
}
