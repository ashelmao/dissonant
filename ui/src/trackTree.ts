// Pure helpers behind the track › pattern tree (`components/TrackTree.tsx`): which rows are
// visible, keyboard stepping, where a dragged track lands, and the remembered open state.

import type { ProjectModel } from "./types";

/** One visible row of the tree. `key` is unique across both kinds ("t:<id>" / "p:<id>"). */
export type TreeRow = { kind: "track"; key: string; id: string; trackId: string } | { kind: "pattern"; key: string; id: string; trackId: string };

export const trackKey = (id: string) => `t:${id}`;
export const patternKey = (id: string) => `p:${id}`;

/** Every row the tree shows, top to bottom: each track, then its patterns unless collapsed. */
export function treeRows(m: Pick<ProjectModel, "tracks" | "patterns">, collapsed: ReadonlySet<string>): TreeRow[] {
  const rows: TreeRow[] = [];
  for (const t of m.tracks) {
    rows.push({ kind: "track", key: trackKey(t.id), id: t.id, trackId: t.id });
    if (collapsed.has(t.id)) continue;
    for (const p of m.patterns) if (p.trackId === t.id) rows.push({ kind: "pattern", key: patternKey(p.id), id: p.id, trackId: t.id });
  }
  return rows;
}

/** The row `delta` steps from `key`, clamped to the ends; the first row when `key` isn't visible. */
export function stepRow(rows: TreeRow[], key: string | null, delta: number): TreeRow | null {
  if (rows.length === 0) return null;
  const i = rows.findIndex((r) => r.key === key);
  if (i < 0) return rows[0];
  return rows[Math.max(0, Math.min(rows.length - 1, i + delta))];
}

/** The id in `after` that isn't in `before` (what an add command just made), or null. */
export function addedId(before: readonly { id: string }[], after: readonly { id: string }[]): string | null {
  const had = new Set(before.map((x) => x.id));
  return after.find((x) => !had.has(x.id))?.id ?? null;
}

/**
 * Which gap a dragged track sits over: 0 = above the first track, `mids.length` = below the
 * last. `mids` are the vertical midpoints of each track's branch, top to bottom.
 */
export function dropGap(mids: readonly number[], y: number): number {
  let gap = 0;
  while (gap < mids.length && y > mids[gap]) gap++;
  return gap;
}

/** The `moveTrack` steps that take the track at `from` into `gap`: none when it wouldn't move. */
export function reorderMoves(from: number, gap: number): { up: boolean; count: number } {
  const to = gap > from ? gap - 1 : gap;
  return { up: to < from, count: Math.abs(to - from) };
}

const COLLAPSED_KEY = "dissonant.trackTree.collapsed";

/** Track ids whose branches were collapsed last time. Empty when storage is unavailable. */
export function loadCollapsed(): Set<string> {
  try {
    const raw = localStorage.getItem(COLLAPSED_KEY);
    const ids: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(ids) ? ids.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

/** Remember the collapsed branches. Only ids still in the project are kept, so it can't grow forever. */
export function saveCollapsed(collapsed: ReadonlySet<string>, trackIds: readonly string[]): void {
  try {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify(trackIds.filter((id) => collapsed.has(id))));
  } catch {
    /* private window or blocked storage: the tree just forgets */
  }
}
