import { useEffect, useState } from "react";
import { CURSOR_COLORS, saveIdentity, savedIdentity } from "../collab";
import { useStore } from "../store";

function Identity({ name, color, onName, onColor }: { name: string; color: string; onName: (n: string) => void; onColor: (c: string) => void }) {
  return (
    <>
      <label className="sharefield">
        <span>your name</span>
        <input autoFocus type="text" value={name} maxLength={24} placeholder="what people see on your cursor" onChange={(e) => onName(e.target.value)} />
      </label>
      <div className="sharefield">
        <span>your color</span>
        <div className="swatches">
          {CURSOR_COLORS.map((c) => (
            <button key={c} className={c === color ? "on" : ""} style={{ background: c }} aria-label={`cursor color ${c}`} aria-pressed={c === color} onClick={() => onColor(c)} />
          ))}
        </div>
      </div>
    </>
  );
}

function Start() {
  const start = useStore((s) => s.startSharing);
  const join = useStore((s) => s.joinSession);
  const close = useStore((s) => s.openShare);
  const saved = savedIdentity();
  const [tab, setTab] = useState<"share" | "join">("share");
  const [name, setName] = useState(saved.name);
  const [color, setColor] = useState(saved.color);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const ready = name.trim().length > 0 && (tab === "share" || code.trim().length > 0);

  const go = async () => {
    if (!ready || busy) return;
    saveIdentity(name.trim(), color);
    setBusy(true);
    await (tab === "share" ? start(name.trim(), color) : join(code.trim(), name.trim(), color));
    setBusy(false);
  };

  return (
    <>
      <div className="seg sharetabs">
        <button className={tab === "share" ? "on" : ""} onClick={() => setTab("share")}>
          share this project
        </button>
        <button className={tab === "join" ? "on" : ""} onClick={() => setTab("join")}>
          join someone
        </button>
      </div>
      <Identity name={name} color={color} onName={setName} onColor={setColor} />
      {tab === "share" ? (
        <div className="mbody">
          you get an invite to send to someone. they edit this project with you, live, and each of you plays it on your own. it stays on this computer, so the session ends when you leave.
        </div>
      ) : (
        <>
          <label className="sharefield">
            <span>invite</span>
            <textarea className="mono" rows={3} value={code} spellCheck={false} placeholder="paste the invite they sent you" onChange={(e) => setCode(e.target.value)} />
          </label>
          <div className="mbody">joining replaces what's open here with their project.</div>
        </>
      )}
      <div className="mbtns">
        <button className="quiet" onClick={() => close(false)}>
          cancel
        </button>
        <button className="solid" disabled={!ready || busy} onClick={() => void go()}>
          {busy ? (tab === "share" ? "going online…" : "connecting…") : tab === "share" ? "start sharing" : "join"}
        </button>
      </div>
    </>
  );
}

function People() {
  const session = useStore((s) => s.session);
  const remove = useStore((s) => s.removePeer);
  return (
    <ul className="sharepeople">
      {session.peers.map((p) => (
        <li key={p.id}>
          <i style={{ background: p.color }} />
          <span>
            {p.name}
            {p.id === session.you ? " (you)" : p.isHost ? " (host)" : ""}
          </span>
          {session.role === "host" && p.id !== session.you && (
            <button className="quiet" onClick={() => void remove(p.id)}>
              remove
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}

function Hosting() {
  const code = useStore((s) => s.session.code) ?? "";
  const count = useStore((s) => s.session.peers.length);
  const stop = useStore((s) => s.stopSharing);
  const reset = useStore((s) => s.resetInvite);
  const close = useStore((s) => s.openShare);
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(code);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <>
      <div className="mbody">send this to whoever you want in. anyone who has it can edit the project, so keep it to people you trust.</div>
      <textarea className="mono sharecode" rows={4} readOnly value={code} onFocus={(e) => e.currentTarget.select()} />
      <div className="sharerow">
        <button className="solid" onClick={() => void copy()}>
          {copied ? "copied" : "copy invite"}
        </button>
        <button className="quiet" onClick={() => void reset()} title="the old invite stops working. people already here stay.">
          new invite
        </button>
      </div>
      <People />
      <div className="mbtns">
        <button className="danger" onClick={() => void stop()}>
          {count > 1 ? `end session for ${count - 1} ${count === 2 ? "person" : "people"}` : "stop sharing"}
        </button>
        <button className="quiet" onClick={() => close(false)}>
          close
        </button>
      </div>
    </>
  );
}

function Joined() {
  const session = useStore((s) => s.session);
  const leave = useStore((s) => s.leaveSession);
  const close = useStore((s) => s.openShare);
  const status = session.reconnecting
    ? "lost the host. trying to get back, and what you do now will go through when it does."
    : !session.live
      ? "connecting…"
      : `connected${session.path ? ` (${session.path})` : ""}. recording and importing audio are the host's for now.`;
  return (
    <>
      <div className="mbody">{status}</div>
      <People />
      <div className="mbtns">
        <button className="danger" onClick={() => void leave()}>
          leave session
        </button>
        <button className="quiet" onClick={() => close(false)}>
          close
        </button>
      </div>
    </>
  );
}

/** Starting, joining and looking after a session. */
export function ShareDialog() {
  const open = useStore((s) => s.shareOpen);
  const close = useStore((s) => s.openShare);
  const role = useStore((s) => s.session.role);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close(false);
      }
      // Nothing typed here should reach the editors or the transport.
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, close]);

  if (!open) return null;
  return (
    <div className="modal-backdrop" onPointerDown={() => close(false)}>
      <div className="modal sharedlg" role="dialog" aria-modal="true" aria-label="session" onPointerDown={(e) => e.stopPropagation()}>
        <div className="mtitle">{role === "solo" ? "work together" : role === "host" ? "you're sharing this project" : "you're in a session"}</div>
        {role === "solo" ? <Start /> : role === "host" ? <Hosting /> : <Joined />}
      </div>
    </div>
  );
}
