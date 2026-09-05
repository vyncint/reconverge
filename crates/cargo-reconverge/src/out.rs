//! Writing to stdout, and what a reader that closes early means.
//!
//! Rust's runtime sets `SIGPIPE` to `SIG_IGN`, so a `println!` into a closed
//! pipe panics and the process exits 101 — a code outside the set the CLI
//! documents (`0` clean, `1` findings, `2` tool error). `check --strict |
//! head -40` is the most ordinary thing anyone does to a long report, and it
//! reported a rustc panic notice as though the analyzer had crashed on the
//! code under test.
//!
//! So every write to stdout goes through a locked handle here and its error
//! is classified rather than unwrapped. A reader that goes away is not a
//! failure: the verdict is fully computed before anything is rendered, and it
//! is still true when the rendering is cut short — so the run keeps its exit
//! code. Mapping a broken pipe to `0` instead would turn a cosmetic bug into
//! the silent pass this analyzer exists to prevent.

use std::io;

/// Interpret the outcome of writing a report to stdout.
///
/// `Ok(())` for a complete write **and** for a reader that closed early;
/// `Err` — which the caller turns into the documented exit code 2 — only for
/// an io error that is a real failure.
pub fn finish(result: io::Result<()>) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        // `| head -1`, `| grep -m1`: the reader got what it wanted and hung
        // up. Stop writing and keep the verdict.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(format!("cannot write to stdout: {e}")),
    }
}
