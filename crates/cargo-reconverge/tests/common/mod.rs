//! Shared helpers for the integration suites.

/// The variables a child `cargo` needs when the test clears the
/// environment, and only those that are actually set: an absent
/// `CARGO_HOME` must stay absent rather than become the empty string, or
/// cargo resolves a different home in one run than in another.
///
/// Every PTY suite here clears the environment, because a child that
/// inherits the caller's produces a different cargo fingerprint and then a
/// `Checking` line that changes what is on the terminal. The list that
/// replaces it was written on Linux and was wrong on Windows in a way that
/// reads like something else entirely:
///
/// ```text
/// [6] Could not resolve hostname (Could not resolve host: index.crates.io)
/// error: could not compile `proc-macro2` (build script)
/// ```
///
/// Neither is a network problem or a compiler problem. Without `SystemRoot`
/// the platform's own networking DLLs cannot initialise, and without `TEMP`
/// and the rest a build script has nowhere to work. They are here, behind
/// `cfg(windows)`, so one list serves every suite and the next one added
/// does not have to rediscover this.
pub fn toolchain_env() -> Vec<(String, String)> {
    const SHARED: &[&str] = &[
        "PATH",
        "HOME",
        "CARGO",
        "CARGO_HOME",
        "RUSTUP_HOME",
        // Without the toolchain pin the child's cargo and rustc can resolve
        // differently from the caller's, and the sample's cached
        // dependencies then look "compiled by an incompatible version".
        "RUSTUP_TOOLCHAIN",
        "RUSTC",
    ];
    #[cfg(windows)]
    const PLATFORM: &[&str] = &[
        "SystemRoot",
        "windir",
        "SystemDrive",
        "ComSpec",
        "PATHEXT",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramData",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "TEMP",
        "TMP",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
    ];
    #[cfg(not(windows))]
    const PLATFORM: &[&str] = &[];

    SHARED
        .iter()
        .chain(PLATFORM)
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_string(), value))
        })
        .collect()
}
