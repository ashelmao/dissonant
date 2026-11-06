//! `dissonant-session` — several people on one project.
//!
//! One person's app is the **host**: it holds the real project and puts every edit in order.
//! Everyone else is a **guest**. A guest sends a [`Command`]; the host runs it and tells everyone
//! the [`Patch`] it produced, so command logic only ever runs for real in one place. To keep
//! editing instant a guest also runs the command itself straight away, and what it shows is the
//! confirmed project with the commands the host hasn't answered yet replayed on top.
//!
//! This crate is the logic only. It takes messages and returns messages, and `dissonant-net`
//! carries them, which is how `tests/convergence.rs` can run a whole session without a network.

mod base;
mod guest;
mod host;
pub mod protocol;

pub use base::{narrow, Bases};
pub use guest::{GuestEvents, GuestSession};
pub use host::{ConnId, HostEvents, HostSession};
pub use protocol::{OpBody, PeerInfo, ToGuest, ToHost, PROTOCOL_VERSION};
