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

/// mimalloc's arenas reserved 32 MiB at a time instead of its default 1 GiB.
/// Every CLI call is a process of its own, and some machines' agents spend
/// CPU on a process that maps 1 GiB: on GitHub's Linux runner, 11.9 ms of
/// another daemon's CPU per call, taken from the browser, made a click 9.4 ms
/// instead of 6.3. The large-page snapshot is as fast with 32 MiB (121 vs 125
/// ms); with no arena at all it is not (142). ci's musl job checks that no
/// mapping reaches 512 MiB.
#[cfg(target_env = "musl")]
mod arena {
    use std::ffi::c_long;

    /// `mi_option_arena_reserve` in mimalloc.h (v2 and v3), in KiB.
    const MI_OPTION_ARENA_RESERVE: i32 = 23;
    const RESERVE_KIB: c_long = 32 * 1024;

    extern "C" {
        fn mi_option_set(option: i32, value: c_long);
    }

    extern "C" fn set() {
        // SAFETY: mi_option_set writes one entry of mimalloc's static option
        // table; it allocates nothing and is valid before mimalloc's own
        // initialisation. It runs before the first allocation (below), so
        // the first arena is reserved at this size.
        unsafe { mi_option_set(MI_OPTION_ARENA_RESERVE, RESERVE_KIB) }
    }

    // From .init_array: before main and before Rust's runtime allocates.
    #[used]
    #[link_section = ".init_array"]
    static SET: extern "C" fn() = set;
}

fn main() -> ExitCode {
    navigera::cli::main(std::env::args())
}
