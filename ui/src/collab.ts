// Shared sessions, on the UI's side: who is here, where they are, and telling them where we are.
//
// Positions travel in musical terms (a tick and a pitch, or a tick and a track), never in pixels:
// everyone has their own zoom, scroll and panel sizes. Everything here is kept out of the main
// store on purpose, because cursors change 30 times a second and the editors must not re-render
// at that rate.

import { create } from "zustand";
import type { ProjectModel } from "./types";

export interface PeerInfo {
  id: string;
  name: string;
  color: string;
  isHost: boolean;
}

export interface SessionState {
  role: "solo" | "host" | "guest";
  /** A guest who has the project and can hear the host. A host always is. */
  live: boolean;
  /** A guest who lost the host and is trying to get back. */
  reconnecting: boolean;
  you: string | null;
  /** The invite to pass around, for a host. */
  code: string | null;
  peers: PeerInfo[];
  path: "direct" | "relayed" | null;
}

export const SOLO: SessionState = { role: "solo", live: false, reconnecting: false, you: null, code: null, peers: [], path: null };

/** A cursor in a pattern: `frac` is how far down the pitch's row it sits, 0–1. */
export interface PatternCursor {
  tick: number;
  pitch: number;
  frac: number;
}

/** A cursor in the song: `lane` is how far down the track's row it sits, 0–1. */
export interface SongCursor {
  tick: number;
  trackId: string;
  lane: number;
}

export type PeerLocation =
  | { screen: "song"; cursor?: SongCursor }
  | { screen: "pattern"; patternId: string; cursor?: PatternCursor }
  | { screen: "setup" | "sound" | "rec" };

export interface PeerPresence {
  peer: string;
  location: PeerLocation;
}

/**
 * Cursor colors. Clear of the brand purple, which means "your own selection", and of the hues the
 * three note tiers use, so a cursor over a note can't be read as a statement about the note. A
 * cursor always carries its name too: color is never the only thing telling two people apart.
 */
export const CURSOR_COLORS = ["#6fd3c7", "#f0a6ca", "#9ad0f5", "#e8d48a", "#c3e88d", "#ffb38a", "#a7b8ff", "#d7c4a3"];

/** A cursor not heard from in this long has gone quiet: its owner lost the connection, say. */
export const STALE_MS = 3500;
const SEND_EVERY_MS = 33;
const HEARTBEAT_MS = 1000;

export function sameView(mine: PeerLocation, theirs: PeerLocation): boolean {
  if (mine.screen !== theirs.screen) return false;
  if (mine.screen === "pattern" && theirs.screen === "pattern") return mine.patternId === theirs.patternId;
  return mine.screen === "song";
}

/** Where someone is, in a few words for the presence bar. */
export function whereLabel(location: PeerLocation | undefined, model: ProjectModel | undefined): string {
  if (!location) return "here";
  if (location.screen === "pattern") {
    const pattern = model?.patterns.find((p) => p.id === location.patternId);
    return pattern ? `in ${pattern.name}` : "in a pattern";
  }
  return location.screen === "song" ? "in the song" : `in ${location.screen}`;
}

/** The row to draw a pitch on when this person's roll has it folded away: the nearest one shown. */
export function nearestPitch(pitch: number, visible: number[]): number | null {
  let best: number | null = null;
  for (const p of visible) {
    if (best === null || Math.abs(p - pitch) < Math.abs(best - pitch)) best = p;
  }
  return best;
}

interface PresenceState {
  /** Where everyone else is, by peer id, and when we last heard. */
  locations: Record<string, { location: PeerLocation; at: number }>;
}

export const usePresence = create<PresenceState>(() => ({ locations: {} }));

export function receivePresence(p: PeerPresence) {
  usePresence.setState((s) => ({ locations: { ...s.locations, [p.peer]: { location: p.location, at: performance.now() } } }));
}

export function forgetPresence(keep: string[]) {
  usePresence.setState((s) => ({
    locations: Object.fromEntries(Object.entries(s.locations).filter(([id]) => keep.includes(id))),
  }));
}

// ─── Telling people where we are ─────────────────────────────────────────────────────────────

let send: ((location: PeerLocation) => void) | null = null;
let here: PeerLocation = { screen: "setup" };
let lastSent = 0;
let waiting: ReturnType<typeof setTimeout> | null = null;
let heartbeat: ReturnType<typeof setInterval> | null = null;

function flush() {
  waiting = null;
  lastSent = performance.now();
  send?.(here);
}

function schedule() {
  if (!send || waiting !== null) return;
  waiting = setTimeout(flush, Math.max(0, SEND_EVERY_MS - (performance.now() - lastSent)));
}

/** Start or stop sending. Datagrams get lost, so where we are is repeated while nothing moves. */
export function setPresenceSender(fn: ((location: PeerLocation) => void) | null) {
  send = fn;
  if (heartbeat !== null) clearInterval(heartbeat);
  heartbeat = fn ? setInterval(flush, HEARTBEAT_MS) : null;
  if (fn) schedule();
}

/** The screen we are on changed. */
export function reportView(location: PeerLocation) {
  if (here.screen === location.screen && sameView(here, location)) return;
  here = location;
  schedule();
}

/** The pointer moved over an editor, or left it (`null`). */
export function reportCursor(cursor: PatternCursor | SongCursor | null) {
  if (here.screen === "pattern") {
    here = { screen: "pattern", patternId: here.patternId, ...(cursor ? { cursor: cursor as PatternCursor } : {}) };
  } else if (here.screen === "song") {
    here = { screen: "song", ...(cursor ? { cursor: cursor as SongCursor } : {}) };
  }
  schedule();
}

/** Where we are right now, for the editors to compare against. */
export function currentView(): PeerLocation {
  return here;
}

// ─── The name people see ─────────────────────────────────────────────────────────────────────

const NAME_KEY = "dissonant.session.name";
const COLOR_KEY = "dissonant.session.color";

export function savedIdentity(): { name: string; color: string } {
  try {
    return { name: localStorage.getItem(NAME_KEY) ?? "", color: localStorage.getItem(COLOR_KEY) ?? CURSOR_COLORS[0] };
  } catch {
    return { name: "", color: CURSOR_COLORS[0] };
  }
}

export function saveIdentity(name: string, color: string) {
  try {
    localStorage.setItem(NAME_KEY, name);
    localStorage.setItem(COLOR_KEY, color);
  } catch {
    // Remembering this is a convenience; the session works without it.
  }
}
