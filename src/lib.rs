//! navigera: from-scratch CDP CLI for AI-agentic browser exploration.
//!
//! Native Rust Chrome DevTools Protocol engine (no WebDriver, no external
//! driver crates): direct CDP JSON-RPC over WebSocket, two-worker Tokio
//! runtime, JSON-lines protocol with `serve` mode and one-shot subcommands.

pub mod annotate;
pub mod browser;
pub mod cdp;
pub mod cli;
pub mod clock;
pub mod decide;
pub mod domains;
pub mod emulate;
mod help_examples;
#[cfg(feature = "http")]
pub mod http;
pub mod install;
pub mod mcp;
pub mod passkey;
pub mod proc;
pub mod protocol;
pub mod screencast;
pub mod session;
pub mod shared;
pub mod timing;
pub mod totp;
pub mod vitals;
