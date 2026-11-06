import { useEffect, useRef, type RefObject } from "react";
import { STALE_MS, usePresence, type PeerLocation } from "../collab";
import { useStore } from "../store";

interface Point {
  x: number;
  y: number;
}

interface Props<T> {
  /** The cursor in `location`, if that person is here and over this editor. */
  pick: (location: PeerLocation) => T | undefined;
  /** Where a cursor goes, in this overlay's own coordinates. Null when it has nowhere to go. */
  place: (cursor: T) => Point | null;
  /** What scrolls, so a cursor outside the view can wait at its edge rather than vanish. */
  scroller: RefObject<HTMLElement | null>;
  /** Keep cursors out of a sticky gutter or ruler. */
  inset?: { left?: number; top?: number };
}

/**
 * Other people's cursors over an editor. Cursors move far more often than anything else on
 * screen, so they are moved straight on their elements from an animation frame: this component
 * only re-renders when somebody joins or leaves, and the editor under it not at all.
 */
export function PeerCursors<T>({ pick, place, scroller, inset }: Props<T>) {
  const peers = useStore((s) => s.session.peers);
  const you = useStore((s) => s.session.you);
  const others = peers.filter((p) => p.id !== you);
  const root = useRef<HTMLDivElement>(null);
  const elements = useRef(new Map<string, HTMLDivElement>());
  const latest = useRef({ pick, place, inset });
  latest.current = { pick, place, inset };
  const anyone = others.length > 0;

  useEffect(() => {
    if (!anyone) return;
    const still = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const shown = new Map<string, Point>();
    let frame = requestAnimationFrame(function draw() {
      frame = requestAnimationFrame(draw);
      const box = root.current?.getBoundingClientRect();
      const view = scroller.current?.getBoundingClientRect();
      if (!box || !view) return;
      const now = performance.now();
      const { locations } = usePresence.getState();
      const { pick, place, inset } = latest.current;
      const left = view.left - box.left + (inset?.left ?? 0);
      const top = view.top - box.top + (inset?.top ?? 0);
      const right = view.right - box.left - 90;
      const bottom = view.bottom - box.top - 24;
      for (const [id, el] of elements.current) {
        const seen = locations[id];
        const cursor = seen && now - seen.at < STALE_MS ? pick(seen.location) : undefined;
        const at = cursor === undefined ? null : place(cursor);
        if (!at) {
          el.style.display = "none";
          shown.delete(id);
          continue;
        }
        const target = { x: Math.max(left, Math.min(right, at.x)), y: Math.max(top, Math.min(bottom, at.y)) };
        const was = shown.get(id);
        // Updates arrive about 30 times a second and frames faster, so easing toward the newest
        // one hides the gaps. A jump across the screen shouldn't take a tour of it.
        const far = was && Math.hypot(target.x - was.x, target.y - was.y) > 400;
        const next = was && !still && !far ? { x: was.x + (target.x - was.x) * 0.4, y: was.y + (target.y - was.y) * 0.4 } : target;
        shown.set(id, next);
        el.style.display = "block";
        el.style.transform = `translate(${next.x}px, ${next.y}px)`;
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [anyone, scroller]);

  if (!anyone) return null;
  return (
    <div className="peercursors" ref={root} aria-hidden="true">
      {others.map((peer) => (
        <div
          key={peer.id}
          className="peercursor"
          style={{ display: "none", color: peer.color }}
          ref={(el) => {
            if (el) elements.current.set(peer.id, el);
            else elements.current.delete(peer.id);
          }}
        >
          <svg width="12" height="14" viewBox="0 0 12 14">
            <path d="M0 0 L0 12 L3.4 9 L6 14 L8 13 L5.5 8.4 L10 8 Z" fill="currentColor" stroke="#0b0b0d" strokeWidth="1" />
          </svg>
          <span style={{ background: peer.color }}>{peer.name}</span>
        </div>
      ))}
    </div>
  );
}
