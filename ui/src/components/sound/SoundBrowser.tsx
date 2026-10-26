import { useMemo, useState } from "react";
import { soundChoices, useStore } from "../../store";
import type { Sound } from "../../sound";

export type SoundSource = "factory" | "library" | "project";

/** Sounds grouped by category, in first-seen order; uncategorized ones under "other". */
function byCategory(sounds: Sound[]): [string, Sound[]][] {
  const groups = new Map<string, Sound[]>();
  for (const s of sounds) {
    const c = s.category || "other";
    groups.set(c, [...(groups.get(c) ?? []), s]);
  }
  return [...groups];
}

/**
 * The left column: synth / drum, a search box, and every sound of that kind — factory by
 * category, then the user's library, then sounds only this project carries. Picking one loads
 * it as the draft.
 */
export function SoundBrowser({
  kind,
  setKind,
  currentId,
  dirty,
  onPick,
  onNew,
}: {
  kind: "synth" | "drum";
  setKind: (k: "synth" | "drum") => void;
  currentId: string;
  dirty: boolean;
  onPick: (sound: Sound, source: SoundSource) => void;
  onNew: () => void;
}) {
  const library = useStore((s) => s.library);
  const snapshot = useStore((s) => s.snapshot);
  const [query, setQuery] = useState("");
  const [folded, setFolded] = useState<Record<string, boolean>>({});
  const choices = useMemo(() => soundChoices({ library, snapshot }, kind), [library, snapshot, kind]);
  const q = query.trim().toLowerCase();
  const match = (s: Sound) => !q || s.name.toLowerCase().includes(q) || s.category.toLowerCase().includes(q);

  const groups: { key: SoundSource; title: string; sounds: Sound[]; empty: string }[] = [
    { key: "factory", title: "factory", sounds: choices.factory.filter(match), empty: "nothing matches" },
    { key: "library", title: "my sounds", sounds: choices.library.filter(match), empty: q ? "nothing matches" : "save a sound and it lands here, in every project" },
    { key: "project", title: "in this project", sounds: choices.project.filter(match), empty: "" },
  ];

  return (
    <aside className="sp-browser">
      <div className="sp-bhead">
        <div className="seg sm" role="radiogroup" aria-label="sound kind">
          <div className={kind === "synth" ? "on" : ""} role="radio" aria-checked={kind === "synth"} onClick={() => setKind("synth")} title="melodic sounds for synth tracks">
            synth
          </div>
          <div className={kind === "drum" ? "on" : ""} role="radio" aria-checked={kind === "drum"} onClick={() => setKind("drum")} title="one-shot hits for drum pads">
            drum
          </div>
        </div>
        <span className="spacer" />
        <button className="quiet" onClick={onNew} title={`start from a blank ${kind} patch`}>
          + init
        </button>
      </div>
      <input type="text" className="sp-search" placeholder={`search ${kind} sounds`} value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setQuery("")} aria-label="search sounds" />
      <div className="sp-blist">
        {groups.map((g) => {
          if (g.key === "project" && g.sounds.length === 0) return null;
          const isFolded = folded[g.key] && !q;
          return (
            <div key={g.key} className="sp-bgroup">
              <div className="sp-btitle" onClick={() => setFolded({ ...folded, [g.key]: !folded[g.key] })} title={g.key === "project" ? "embedded in this song but not in your library (made on another machine, or deleted from it)" : undefined}>
                <span className="fold">{isFolded ? "▸" : "▾"}</span>
                <span className="cap">{g.title}</span>
                <span className="n">{g.sounds.length}</span>
              </div>
              {!isFolded &&
                (g.sounds.length === 0 ? (
                  <div className="sp-bempty">{g.empty}</div>
                ) : (
                  byCategory(g.sounds).map(([cat, list]) => (
                    <div key={cat}>
                      <div className="sp-bcat">{cat}</div>
                      {list.map((s) => (
                        <div
                          key={s.id}
                          className={`sp-bitem${s.id === currentId ? " on" : ""}`}
                          onClick={() => onPick(s, g.key)}
                          role="button"
                          tabIndex={0}
                          onKeyDown={(e) => e.key === "Enter" && onPick(s, g.key)}
                          title={g.key === "factory" ? `${s.name} · factory: edits save as a new sound of yours` : s.name}
                        >
                          <span className="nm">{s.name}</span>
                          {s.id === currentId && dirty && <span className="edited">edited</span>}
                        </div>
                      ))}
                    </div>
                  ))
                ))}
            </div>
          );
        })}
      </div>
    </aside>
  );
}
