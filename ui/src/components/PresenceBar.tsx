import { usePresence, whereLabel } from "../collab";
import { useStore } from "../store";

function Person({ id, name, color, isHost }: { id: string; name: string; color: string; isHost: boolean }) {
  const model = useStore((s) => s.snapshot?.model);
  // A string, so this person's chip re-renders when they go somewhere, not when they move.
  const where = usePresence((s) => whereLabel(s.locations[id]?.location, model));
  const goTo = () => {
    const location = usePresence.getState().locations[id]?.location;
    if (!location) return;
    const s = useStore.getState();
    if (location.screen === "pattern") s.openPattern(location.patternId);
    else s.setScreen(location.screen);
  };
  return (
    <button className="person" onClick={goTo} title={`${name}${isHost ? " (host)" : ""}, ${where}. click to go there`}>
      <i style={{ background: color }}>{name.slice(0, 1).toUpperCase()}</i>
      <span>{where}</span>
    </button>
  );
}

/** Who is in the session and where, with the way into the session panel. */
export function PresenceBar() {
  const session = useStore((s) => s.session);
  const open = useStore((s) => s.openShare);
  if (session.role === "solo") {
    return (
      <button className="sharebtn" onClick={() => open(true)} title="work on this project with someone else">
        share
      </button>
    );
  }
  const others = session.peers.filter((p) => p.id !== session.you);
  return (
    <div className="presence">
      {session.reconnecting && <span className="reconnecting">reconnecting…</span>}
      {session.role === "guest" && !session.live && !session.reconnecting && <span className="reconnecting">joining…</span>}
      {others.map((p) => (
        <Person key={p.id} {...p} />
      ))}
      <button className="sharebtn on" onClick={() => open(true)} title="session details">
        {session.role === "host" ? "sharing" : "in session"}
      </button>
    </div>
  );
}
