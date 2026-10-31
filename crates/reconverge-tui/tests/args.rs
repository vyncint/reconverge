//! The command line, before any terminal is involved: everything here exits
//! before raw mode, so a plain `Command` is the right harness and no PTY is
//! needed — which also keeps it running on every platform CI has.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reconverge-tui"))
        .args(args)
        .output()
        .expect("failed to run reconverge-tui")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `cargo reconverge setup` installs three binaries that must be one
/// release, and this was the one that could not say which it was:
/// `--version` opened the shell on an artifact called `--version`.
#[test]
fn version_names_the_release() {
    let expected = format!("reconverge-tui {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag}: {:?}", out.status);
        assert_eq!(text(&out.stdout), expected, "{flag}");
        assert!(out.stderr.is_empty(), "{flag}: {}", text(&out.stderr));
    }
}

#[test]
fn help_prints_the_usage_of_every_view() {
    for flag in ["--help", "-h"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag}: {:?}", out.status);
        let stdout = text(&out.stdout);
        for view in ["inspect", "witness", "learn", "triage", "--version"] {
            assert!(stdout.contains(view), "{flag} omits {view}:\n{stdout}");
        }
    }
}

/// A typo is refused as a typo. `--asci` used to be opened as an artifact
/// path, and the error surfaced inside the view as a missing file.
#[test]
fn an_unknown_flag_is_refused_before_any_view_opens() {
    for args in [&["--asci"][..], &["witness", "--no-such-flag"], &["-x"]] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let stderr = text(&out.stderr);
        assert!(
            stderr.contains(&format!("unknown option `{}`", args[args.len() - 1])),
            "{args:?}: {stderr}"
        );
        assert!(stderr.contains("usage:"), "{args:?}: {stderr}");
        assert!(out.stdout.is_empty(), "{args:?}: nothing drawn");
    }
}

/// After `--`, a name that starts with a dash is a file again — so it gets
/// as far as the terminal check, which no pipe passes.
#[test]
fn double_dash_ends_the_options() {
    let out = run(&["--", "--asci"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = text(&out.stderr);
    assert!(!stderr.contains("unknown option"), "{stderr}");
    assert!(
        stderr.contains("interactive terminal is required"),
        "{stderr}"
    );
}
