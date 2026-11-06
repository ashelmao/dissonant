import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

interface Preset {
  id: string;
  name: string;
  blurb: string;
}

/**
 * A popover list of presets (kits, loops) by group: a group row, a search field, ▶ to hear one
 * (↑ ↓ step through and play each), a click or Enter to pick, and the focused preset's blurb
 * underneath. Styled like the sound picker.
 */
export function PresetBrowser<T extends Preset>({
  items,
  groups,
  groupOf,
  value,
  playing,
  onPick,
  onPlay,
  onClose,
  meta,
  matches,
  noun,
  hint,
  footer,
}: {
  items: T[];
  /** Group names in display order. */
  groups: string[];
  groupOf: (item: T) => string;
  /** The current preset, marked in the list; empty when none matches. */
  value: string;
  /** The preset being heard; its ▶ shows ■ and stops it. */
  playing: string | null;
  onPick: (item: T) => void;
  onPlay: (item: T) => void;
  onClose: () => void;
  meta: (item: T) => ReactNode;
  /** Search beyond the name and group (`q` is lowercase). */
  matches: (item: T, q: string) => boolean;
  noun: string;
  /** Under the list when nothing is focused. */
  hint: string;
  /** Extra controls under the blurb. */
  footer?: ReactNode;
}) {
  const [group, setGroup] = useState("all");
  const [query, setQuery] = useState("");
  const [hi, setHi] = useState(-1);
  const listRef = useRef<HTMLDivElement>(null);

  const q = query.trim().toLowerCase();
  const shown = groups
    .filter((g) => q || group === "all" || g === group)
    .map((g) => ({ group: g, items: items.filter((it) => groupOf(it) === g && (!q || it.name.toLowerCase().includes(q) || g.includes(q) || matches(it, q))) }))
    .filter((g) => g.items.length);
  const flat = shown.flatMap((g) => g.items);
  const focus = flat[hi] ?? flat.find((it) => it.id === value);

  useLayoutEffect(() => {
    listRef.current?.querySelector(".sp-row.on")?.scrollIntoView({ block: "nearest" });
  }, []);
  useEffect(() => {
    listRef.current?.querySelector(".sp-row.hi")?.scrollIntoView({ block: "nearest" });
  }, [hi]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const d = e.key === "ArrowDown" ? 1 : -1;
      const next = Math.max(0, Math.min(flat.length - 1, (hi < 0 ? flat.findIndex((it) => it.id === value) : hi) + d));
      setHi(next);
      if (flat[next] && next !== hi) onPlay(flat[next]);
    } else if (e.key === "Enter") {
      const it = flat[hi >= 0 ? hi : 0];
      if (it) onPick(it);
    }
  };

  let n = 0;
  return (
    <div className="sp kp" onKeyDown={onKey}>
      <div className="sp-cats">
        <button className={`chip tiny${group === "all" && !q ? " on" : ""}`} onClick={() => (setGroup("all"), setQuery(""), setHi(-1))}>all</button>
        {groups.map((g) => (
          <button key={g} className={`chip tiny${group === g && !q ? " on" : ""}`} onClick={() => (setGroup(g), setQuery(""), setHi(-1))}>
            {g}
          </button>
        ))}
      </div>
      <input
        type="text"
        className="sp-search"
        placeholder={`search ${items.length} ${noun}`}
        value={query}
        autoFocus
        spellCheck={false}
        onChange={(e) => (setQuery(e.target.value), setHi(-1))}
      />
      <div className="sp-list" ref={listRef}>
        {shown.map((g) => (
          <div key={g.group} className="sp-group">
            {(group === "all" || q) && <div className="sp-head">{g.group}</div>}
            {g.items.map((it) => {
              const i = n++;
              const heard = it.id === playing;
              return (
                <div key={it.id} className={`sp-row${it.id === value ? " on" : ""}${i === hi ? " hi" : ""}`} title={it.blurb} onClick={() => onPick(it)} onPointerEnter={() => setHi(i)}>
                  <button
                    className="sp-play"
                    title={heard ? "stop" : `hear ${it.name}`}
                    aria-label={heard ? "stop" : `hear ${it.name}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      onPlay(it);
                    }}
                  >
                    {heard ? "■" : "▶"}
                  </button>
                  <span className="sp-name">{it.name}</span>
                  <span className="kp-meta">{meta(it)}</span>
                  {it.id === value && <span className="sp-meta">●</span>}
                </div>
              );
            })}
          </div>
        ))}
        {flat.length === 0 && <div className="help">no {noun} match “{query}”</div>}
      </div>
      <div className="kp-blurb">{focus ? focus.blurb : hint}</div>
      {footer}
    </div>
  );
}
