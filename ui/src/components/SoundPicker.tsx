import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { getBridge } from "../bridge";
import { soundChoices, useStore } from "../store";
import { lookupSound, type Sound } from "../sound";
import { DRUM_PITCH } from "./sound/AuditionKeys";

interface Group {
  key: string;
  label: string;
  sounds: Sound[];
}

/** Factory sounds by category (file order), then "my sounds", then sounds only this project has. */
function useSoundGroups(kind: "synth" | "drum"): Group[] {
  const library = useStore((s) => s.library);
  const snapshot = useStore((s) => s.snapshot);
  return useMemo(() => {
    const { factory, library: mine, project } = soundChoices({ library, snapshot }, kind);
    const groups: Group[] = [];
    for (const s of factory) {
      const cat = s.category || "other";
      let g = groups.find((x) => x.key === `f:${cat}`);
      if (!g) groups.push((g = { key: `f:${cat}`, label: cat, sounds: [] }));
      g.sounds.push(s);
    }
    if (mine.length) groups.push({ key: "mine", label: "my sounds", sounds: mine });
    if (project.length) groups.push({ key: "project", label: "in this project", sounds: project });
    return groups;
  }, [library, snapshot, kind]);
}

/** A sound's display name: the project's copy, then the factory, then the library; else the id. */
export function useSoundName(id: string): string {
  const sounds = useStore((s) => s.snapshot?.model.sounds);
  const library = useStore((s) => s.library);
  return lookupSound(sounds ?? [], id)?.name ?? library.find((s) => s.id === id)?.name ?? id;
}

/**
 * Hear a sound before picking it, on the engine's preview channel (a drum on any pitch, a synth
 * on middle C). Releasing the channel lets the last hit ring out.
 */
function usePreview() {
  const timer = useRef(0);
  useEffect(
    () => () => {
      window.clearTimeout(timer.current);
      void getBridge().then((b) => b.previewSound(null));
    },
    [],
  );
  return useCallback((s: Sound) => {
    window.clearTimeout(timer.current);
    void getBridge().then(async (b) => {
      await b.previewSound(s);
      await b.previewNote(DRUM_PITCH, 100, true);
    });
    timer.current = window.setTimeout(() => void getBridge().then((b) => b.previewNote(DRUM_PITCH, 0, false)), 400);
  }, []);
}

/**
 * Browse and pick a sound of one kind: a category row, a search field and a scrolling list.
 * `inline` fills its container (the devices box); otherwise a trigger opens it as a popover.
 * The picked Sound is handed over whole, so the caller can embed a user sound in the project.
 * With `preview`, each row has a ▶ to hear it, and stepping through with ↑ ↓ plays each one.
 */
export function SoundPicker({
  kind,
  value,
  onPick,
  inline = false,
  children,
  title,
  triggerClass,
  preview = false,
  disabled = false,
}: {
  kind: "synth" | "drum";
  /** The current sound, marked in the list; empty when picking a new one. */
  value: string;
  onPick: (sound: Sound) => void;
  inline?: boolean;
  /** Trigger contents for the popover form; defaults to the current sound's name and a caret. */
  children?: ReactNode;
  title?: string;
  /** Replaces the trigger's own look (e.g. a chip button). */
  triggerClass?: string;
  preview?: boolean;
  disabled?: boolean;
}) {
  const name = useSoundName(value);
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);

  if (inline) return <SoundBrowser kind={kind} value={value} onPick={onPick} preview={preview} />;
  return (
    <>
      <button
        ref={trigger}
        className={`${triggerClass ?? `sp-trigger${children ? " bare" : ""}`}${open ? " open" : ""}`}
        title={title ?? "pick a sound"}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        {children ?? (
          <>
            <span className="sp-name">{name}</span>
            <span className="sp-caret">▾</span>
          </>
        )}
      </button>
      {open && trigger.current && (
        <Popover anchor={trigger.current} onClose={() => setOpen(false)}>
          <SoundBrowser
            kind={kind}
            value={value}
            autoFocus
            preview={preview}
            onClose={() => setOpen(false)}
            onPick={(s) => {
              setOpen(false);
              onPick(s);
            }}
          />
        </Popover>
      )}
    </>
  );
}

const POP_W = 250;
const POP_H = 320;

/** Fixed to the window so the devices panel's scrolling can't clip it; opens above when low. */
function Popover({ anchor, onClose, children }: { anchor: HTMLElement; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!ref.current?.contains(t) && !anchor.contains(t)) closeRef.current();
    };
    // Scrolling what the anchor sits in would leave the popover floating loose.
    const onScroll = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) closeRef.current();
    };
    const close = () => closeRef.current();
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [anchor]);
  const r = anchor.getBoundingClientRect();
  const below = window.innerHeight - r.bottom - 8;
  const above = r.top - 8;
  const left = Math.max(8, Math.min(r.left, window.innerWidth - POP_W - 8));
  const style: React.CSSProperties =
    below >= Math.min(POP_H, above) ? { left, top: r.bottom + 2, maxHeight: Math.min(POP_H, below) } : { left, bottom: window.innerHeight - r.top + 2, maxHeight: Math.min(POP_H, above) };
  return createPortal(
    <div className="sp-pop" ref={ref} style={{ ...style, width: POP_W }}>
      {children}
    </div>,
    document.body,
  );
}

function SoundBrowser({
  kind,
  value,
  onPick,
  autoFocus = false,
  preview = false,
  onClose,
}: {
  kind: "synth" | "drum";
  value: string;
  onPick: (sound: Sound) => void;
  autoFocus?: boolean;
  preview?: boolean;
  onClose?: () => void;
}) {
  const groups = useSoundGroups(kind);
  const play = usePreview();
  const [cat, setCat] = useState<string>("all");
  const [query, setQuery] = useState("");
  const [hi, setHi] = useState(-1);
  const listRef = useRef<HTMLDivElement>(null);

  const q = query.trim().toLowerCase();
  const shown = groups
    .filter((g) => q || cat === "all" || g.key === cat)
    .map((g) => ({ ...g, sounds: q ? g.sounds.filter((s) => s.name.toLowerCase().includes(q) || g.label.toLowerCase().includes(q) || s.id.includes(q)) : g.sounds }))
    .filter((g) => g.sounds.length);
  const flat = shown.flatMap((g) => g.sounds);

  // Start with the current sound in view.
  useLayoutEffect(() => {
    listRef.current?.querySelector(".sp-row.on")?.scrollIntoView({ block: "nearest" });
  }, []);
  useEffect(() => {
    listRef.current?.querySelector(".sp-row.hi")?.scrollIntoView({ block: "nearest" });
  }, [hi]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose?.();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const d = e.key === "ArrowDown" ? 1 : -1;
      const next = Math.max(0, Math.min(flat.length - 1, (hi < 0 ? flat.findIndex((s) => s.id === value) : hi) + d));
      setHi(next);
      if (preview && flat[next] && next !== hi) play(flat[next]);
    } else if (e.key === "Enter") {
      const s = flat[hi >= 0 ? hi : 0];
      if (s) onPick(s);
    }
  };

  let n = 0;
  return (
    <div className="sp" onKeyDown={onKey}>
      <div className="sp-cats">
        <button className={`chip tiny${cat === "all" && !q ? " on" : ""}`} onClick={() => (setCat("all"), setQuery(""))}>all</button>
        {groups.map((g) => (
          <button key={g.key} className={`chip tiny${cat === g.key && !q ? " on" : ""}${g.key.startsWith("f:") ? "" : " own"}`} onClick={() => (setCat(g.key), setQuery(""), setHi(-1))}>
            {g.label}
          </button>
        ))}
      </div>
      <input
        type="text"
        className="sp-search"
        placeholder={`search ${groups.reduce((a, g) => a + g.sounds.length, 0)} ${kind} sounds`}
        value={query}
        autoFocus={autoFocus}
        spellCheck={false}
        onChange={(e) => (setQuery(e.target.value), setHi(-1))}
      />
      <div className="sp-list" ref={listRef}>
        {shown.map((g) => (
          <div key={g.key} className="sp-group">
            {(cat === "all" || q) && <div className="sp-head">{g.label}</div>}
            {g.sounds.map((s) => {
              const i = n++;
              return (
                <div key={s.id} className={`sp-row${s.id === value ? " on" : ""}${i === hi ? " hi" : ""}`} onClick={() => onPick(s)} onPointerEnter={() => setHi(i)}>
                  {preview && (
                    <button
                      className="sp-play"
                      title={`hear ${s.name}`}
                      aria-label={`hear ${s.name}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        play(s);
                      }}
                    >
                      ▶
                    </button>
                  )}
                  <span className="sp-name">{s.name}</span>
                  {s.id === value && <span className="sp-meta">●</span>}
                </div>
              );
            })}
          </div>
        ))}
        {flat.length === 0 && <div className="help">no {kind} sounds match “{query}”</div>}
      </div>
    </div>
  );
}
