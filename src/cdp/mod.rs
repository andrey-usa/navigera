//! From-scratch minimal CDP engine: no external browser-automation crate.
//!
//! Design goals (this is what makes it faster than the alternatives):
//! * `Runtime.evaluate` with `returnByValue: true` — object results come back
//!   in ONE round trip (no evaluate + callFunctionOn + releaseObject dance).
//! * A tiny tokio runtime (2 workers) owned by the engine — no framework
//!   machinery, no background tasks beyond the single socket pump.
//! * Boring, direct teardown: SIGKILL the browser's process group, delete
//!   the throwaway profile (on tmpfs where possible, see `profile`), reap
//!   in the background. No close lifecycles, no nested runtimes.
//! * Synchronous public facade: the CLI stays simple, `block_on` per call.
//!
//! Layout: `transport` launches Chrome and finds the DevTools endpoint,
//! `client` multiplexes JSON-RPC over the websocket, `browser` owns the
//! browser-level connection, `page` implements the tab-level operations.

pub mod attach;
pub mod ax;
mod browser;
mod client;
mod events;
mod page;
mod procjob;
mod profile;
mod transport;

pub use browser::{Browser, LaunchOptions};
pub use events::DialogPolicy;
pub use page::{AxOptions, Page, Target, DEFAULT_ELEMENT_WAIT};
