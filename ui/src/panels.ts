// Panel sizes the user drags (the track tree, the inspector, the bottom panel, …).
//
// They are layout, not project data and not a setting anyone would look for in a settings tab,
// so they live here: one module-level record, persisted per machine in `localStorage`, read
// through `useSyncExternalStore` so a drag on the splitter re-renders both the panel and
// whatever else measures against it (the arrangement's head column is a width every row uses).

import { useSyncExternalStore } from "react";
import { onZoomChange, pageZoom } from "./uiScale";

export type PanelId = "tree" | "inspector" | "bottom" | "songHead" | "rec" | "soundBrowser";

interface Spec {
  /** The size in CSS px before anyone drags it: it scales with the UI, like the text in it. */
  size: number;
  /** The floor in *screen* px — how small the panel may be dragged, whatever the UI scale. */
  min: number;
  /** The ceiling in CSS px: a sanity cap, since `keep` does the real work on a small window. */
  max: number;
  /** Which window dimension the panel eats into, for the "leave room for the rest" clamp. */
  axis: "x" | "y";
  /** Screen px of the other panes that must survive at any window size. */
  keep: number;
}

// `min` and `keep` are screen pixels rather than CSS pixels on purpose: the UI scale is a page
// zoom, so a CSS-px floor of 150 would really be 225 px of screen at 150% and 300 at 200% —
// turning the scale up would quietly take away the user's room to shrink a panel. Dividing by
// the zoom keeps "as small as this may go" the same amount of screen at every scale.
const SPECS: Record<PanelId, Spec> = {
  tree: { size: 248, min: 150, max: 560, axis: "x", keep: 360 },
  inspector: { size: 264, min: 180, max: 600, axis: "x", keep: 360 },
  bottom: { size: 300, min: 120, max: 760, axis: "y", keep: 220 },
  songHead: { size: 220, min: 120, max: 460, axis: "x", keep: 320 },
  rec: { size: 300, min: 200, max: 620, axis: "x", keep: 320 },
  soundBrowser: { size: 228, min: 160, max: 520, axis: "x", keep: 420 },
};

const KEY = "dissonant.panels";

const IDS = Object.keys(SPECS) as PanelId[];
const sizes = Object.fromEntries(IDS.map((id) => [id, SPECS[id].size])) as Record<PanelId, number>;
const listeners = new Set<() => void>();

/** The default size of a panel, for a splitter's double-click reset. */
export function panelDefault(id: PanelId): number {
  return SPECS[id].size;
}

/**
 * A size made usable: at or above the floor, inside the cap, and never so large that the rest
 * of the window has less than `keep` left. The window and zoom clamps are applied on every
 * read rather than on the stored value, so shrinking the window (or scaling the UI up) and
 * going back gives the size the user set.
 */
export function clampPanel(id: PanelId, px: number): number {
  const spec = SPECS[id];
  const zoom = pageZoom();
  const min = spec.min / zoom;
  const window_ = typeof window === "undefined" ? null : window;
  const room = window_ ? (spec.axis === "x" ? window_.innerWidth : window_.innerHeight) - spec.keep / zoom : Infinity;
  const max = Math.max(min, Math.min(spec.max, room));
  return Math.round(Math.max(min, Math.min(max, px)));
}

export function panelSize(id: PanelId): number {
  return clampPanel(id, sizes[id]);
}

export function setPanelSize(id: PanelId, px: number) {
  const next = clampPanel(id, px);
  if (next === sizes[id]) return;
  sizes[id] = next;
  listeners.forEach((l) => l());
  try {
    localStorage.setItem(KEY, JSON.stringify(sizes));
  } catch {
    /* storage off: the size just won't outlive the session */
  }
}

/** Load the remembered sizes. Unknown ids and nonsense values are ignored, not thrown over. */
function load() {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return;
    const saved = JSON.parse(raw) as Partial<Record<PanelId, unknown>>;
    for (const id of IDS) {
      const v = saved[id];
      if (typeof v === "number" && Number.isFinite(v)) sizes[id] = clampPanel(id, v);
    }
  } catch {
    /* unreadable: the defaults stand */
  }
}
load();

let stopWatching: (() => void) | null = null;

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  // A narrower window, or a bigger UI scale, can push a panel past its "leave room" clamp;
  // re-reading on both keeps the rendered size honest without touching what was stored.
  if (listeners.size === 1 && typeof window !== "undefined") {
    window.addEventListener("resize", notifyAll);
    const stopZoom = onZoomChange(notifyAll);
    stopWatching = () => {
      window.removeEventListener("resize", notifyAll);
      stopZoom();
    };
  }
  return () => {
    listeners.delete(fn);
    if (listeners.size === 0) {
      stopWatching?.();
      stopWatching = null;
    }
  };
}

function notifyAll() {
  listeners.forEach((l) => l());
}

/** The panel's current size in px, re-rendering the caller while it is dragged. */
export function usePanelSize(id: PanelId): number {
  return useSyncExternalStore(
    subscribe,
    () => panelSize(id),
    () => SPECS[id].size,
  );
}
