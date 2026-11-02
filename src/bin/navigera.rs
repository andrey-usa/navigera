//! `navigera` — thin CLI wrapper over the [`navigera`] library.
//!
//! All parsing, protocol and session logic lives in `navigera::
//! navigera` so it can be reused (and unit-tested) without spawning a
//! process; this binary only wires argv/stdin/stdout to that library.
//!
//! ```text
//! navigera eval --expression "() => document.title" --pretty   # one-shot
//! navigera serve                                               # warm session
//! ```

use std::process::ExitCode;

/// The static Linux build's allocator: musl's own is several times slower
/// on the small allocations of a large page's snapshot.
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    navigera::cli::main(std::env::args())
}
