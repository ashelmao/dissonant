---
title: Host-authoritative sessions over iroh, with patches rather than a CRDT
status: accepted
date: 2026-11-06
---

# Decision: live collaboration runs through the host's app

## Context

Two or more people want to work on one project at once, each playing it on their own machine.
Every edit already goes through one `Command` bus. The project is small, and audio stays on disk.

## Options considered

| Option | Verdict |
|---|---|
| **Host's app is the authority; guests predict; the host broadcasts patches** | **Chosen.** One machine orders everything, so commands run for real in one place and ripple edits never have to be reproduced. No server to run or pay for. The cost: nobody can work without the host. |
| Cloud room on a server we run | A real Google Docs model, but a server to keep up, and accounts soon after. Still reachable from here: the same session logic could run on a server. |
| CRDT (Automerge, Yjs, Loro) | Offline merge, at the price of rebuilding `ProjectModel` storage and undo around it. Two offline edits can merge cleanly and still sound wrong. |

## Why patches and not commands

A guest could replay the host's commands, but then every command would have to behave identically
everywhere, forever, including every clip split and chord trim. A patch is the *result* of a command, so
guests need one small generic function. The same `Patch` also became the undo step, which is what lets
undo leave other people's work alone.

## Why iroh

MIT or Apache-2.0, which matters here for the same reason CLAP did. It dials by public key, so a host
behind a home router needs no port forwarding, it falls back to a relay when a direct path fails, and the
relay can't read the traffic. QUIC datagrams suit cursors, where a late update is worth nothing.

## Consequences

- `Command` changes are protocol changes: everyone in a session needs the same build, and
  `PROTOCOL_VERSION` has to be bumped.
- n0's public relays are fine for friends and rate-limited in ways they don't publish. Anything bigger
  should run its own `iroh-relay`.
