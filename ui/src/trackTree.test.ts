import { afterEach, describe, expect, it, vi } from "vitest";
import { addedId, dropGap, loadCollapsed, reorderMoves, saveCollapsed, stepRow, treeRows } from "./trackTree";

const m = {
  tracks: [{ id: "a" }, { id: "b" }, { id: "c" }],
  patterns: [
    { id: "a1", trackId: "a" },
    { id: "b1", trackId: "b" },
    { id: "a2", trackId: "a" },
  ],
} as unknown as Parameters<typeof treeRows>[0];

const keys = (collapsed: string[] = []) => treeRows(m, new Set(collapsed)).map((r) => r.key);

describe("treeRows", () => {
  it("lists each track followed by its own patterns, in model order", () => {
    expect(keys()).toEqual(["t:a", "p:a1", "p:a2", "t:b", "p:b1", "t:c"]);
  });

  it("hides the patterns of collapsed tracks", () => {
    expect(keys(["a"])).toEqual(["t:a", "t:b", "p:b1", "t:c"]);
  });
});

describe("stepRow", () => {
  const rows = treeRows(m, new Set());

  it("moves by delta and clamps at both ends", () => {
    expect(stepRow(rows, "p:a2", 1)?.key).toBe("t:b");
    expect(stepRow(rows, "t:a", -1)?.key).toBe("t:a");
    expect(stepRow(rows, "t:c", 1)?.key).toBe("t:c");
    expect(stepRow(rows, "t:a", Infinity)?.key).toBe("t:c");
  });

  it("starts at the top when the current row isn't visible", () => {
    expect(stepRow(rows, "p:gone", 1)?.key).toBe("t:a");
    expect(stepRow([], "t:a", 1)).toBeNull();
  });
});

describe("addedId", () => {
  it("finds the one new id", () => {
    expect(addedId([{ id: "x" }], [{ id: "x" }, { id: "y" }])).toBe("y");
    expect(addedId([{ id: "x" }], [{ id: "x" }])).toBeNull();
  });
});

describe("track drag-reorder", () => {
  it("picks the gap under the pointer from branch midpoints", () => {
    const mids = [10, 30, 50];
    expect(dropGap(mids, 0)).toBe(0);
    expect(dropGap(mids, 20)).toBe(1);
    expect(dropGap(mids, 99)).toBe(3);
  });

  it("turns a gap into moveTrack steps", () => {
    expect(reorderMoves(0, 3)).toEqual({ up: false, count: 2 });
    expect(reorderMoves(2, 0)).toEqual({ up: true, count: 2 });
    // The gaps on either side of the dragged track leave it where it is.
    expect(reorderMoves(1, 1).count).toBe(0);
    expect(reorderMoves(1, 2).count).toBe(0);
  });
});

describe("collapsed state", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("round-trips through storage, keeping only ids still in the project", () => {
    const store = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) });
    saveCollapsed(new Set(["a", "gone"]), ["a", "b"]);
    expect([...loadCollapsed()]).toEqual(["a"]);
  });

  it("forgets quietly when storage throws", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    });
    expect(() => saveCollapsed(new Set(["a"]), ["a"])).not.toThrow();
    expect(loadCollapsed().size).toBe(0);
  });
});
