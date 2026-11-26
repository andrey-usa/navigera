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
#[cfg(all(any(target_env = "musl", feature = "exp-mimalloc"), not(feature = "exp-sysalloc")))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// experiment-36 run 6: mimalloc's arena reservation set in code, before
/// the first allocation, to `NAVIGERA_EXP_ARENA_KIB` KiB at build time
/// (default 32 MiB; mimalloc's own is 1 GiB). Run 5 saw the runner's
/// provjobd spend ~1.1 ms of CPU on every process that reserves 1 GiB.
#[cfg(feature = "exp-arena-reserve")]
mod arena {
    use std::ffi::c_long;

    // mimalloc.h (v2 and v3 alike): `mi_option_arena_reserve` is 23.
    const MI_OPTION_ARENA_RESERVE: i32 = 23;

    const fn kib() -> c_long {
        let s = match option_env!("NAVIGERA_EXP_ARENA_KIB") {
            Some(s) => s.as_bytes(),
            None => b"32768",
        };
        let mut v: c_long = 0;
        let mut i = 0;
        while i < s.len() {
            assert!(s[i].is_ascii_digit(), "NAVIGERA_EXP_ARENA_KIB: digits only");
            v = v * 10 + (s[i] - b'0') as c_long;
            i += 1;
        }
        v
    }

    extern "C" {
        fn mi_option_set(option: i32, value: c_long);
    }

    extern "C" fn set() {
        // SAFETY: mi_option_set writes one entry of mimalloc's static option
        // table; it allocates nothing and is valid before mimalloc's own
        // initialisation (the entry is then marked initialised, so the
        // environment does not override it).
        unsafe { mi_option_set(MI_OPTION_ARENA_RESERVE, kib()) }
    }

    // From .init_array: before main and before Rust's runtime allocates.
    #[used]
    #[link_section = ".init_array"]
    static SET: extern "C" fn() = set;
}

fn main() -> ExitCode {
    navigera::cli::main(std::env::args())
}
