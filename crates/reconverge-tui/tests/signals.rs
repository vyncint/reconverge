//! The signals that end a process from outside — `timeout`, `tmux
//! kill-pane`, a parent shutting down — restore the terminal before the
//! process dies of them (#205).
//!
//! The TUI runs under a shell in the PTY, because what a killed TUI breaks
//! is the *next* program on that terminal: raw mode outlives the process
//! that set it, so the only witness is whoever reads the terminal after.
//! The shell reports the TUI's exit status — 128 plus the signal number
//! when it died of the signal, which is the half of the contract that
//! re-raising keeps — and then asks `stty` whether the line discipline is
//! back in canonical mode with echo. The PTY's own grid says the rest: off
//! the alternate screen, cursor visible.
//!
//! Unix only: there are no such signals on Windows, and termlens cannot
//! send one there.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::{env, fs};

use termlens::Terminal;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Prints the tty state before and after, and the TUI's status between.
/// The inner shell writes its pid and then execs, so the pid is the TUI's
/// and the signal reaches the TUI alone. `ulimit -c 0`: SIGQUIT's default
/// action dumps core, and a test has no use for one.
const SCRIPT: &str = r#"
ulimit -c 0 2>/dev/null
tty_state() {
    case " $(stty -a | tr '\n' ' ') " in
        *" -icanon "*|*" -echo "*) echo raw ;;
        *) echo cooked ;;
    esac
}
echo "before=$(tty_state)"
/bin/sh -c 'echo $$ > "$1"; exec "$2" learn' sh "$PIDFILE" "$TUI"
echo "rc=$?"
echo "after=$(tty_state)"
"#;

fn killed_by(signal: &str, number: u32) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("signal-{signal}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let pidfile = dir.join("pid");

    let mut t = Terminal::builder()
        .size(80, 24)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PIDFILE", &pidfile)
        .env("TUI", env!("CARGO_BIN_EXE_reconverge-tui"))
        .timeout(TIMEOUT)
        .arg("-c")
        .arg(SCRIPT)
        .spawn("/bin/sh")
        .expect("failed to spawn the shell in a PTY");

    // A drawn frame means the handler is installed: it goes in before raw
    // mode, and raw mode before the first draw.
    t.wait_frame(|s| s.alternate_screen() && s.contains("reconverge learn"))
        .expect("learn mode drawn");
    let pid = fs::read_to_string(&pidfile).expect("the TUI's pid");
    let sent = Command::new("kill")
        .args(["-s", signal, pid.trim()])
        .status()
        .expect("run kill");
    assert!(sent.success(), "kill -s {signal} {pid}");

    let status = t.wait_exit().expect("the shell finishes after the TUI");
    assert!(status.success(), "the script itself: {status:?}");
    let screen = t.screen();
    let context = format!("SIG{signal}:\n{screen}");
    assert!(screen.contains("before=cooked"), "{context}");
    assert!(
        screen.contains(&format!("rc={}", 128 + number)),
        "the TUI must die of SIG{signal}, not exit cleanly: {context}"
    );
    assert!(
        screen.contains("after=cooked"),
        "raw mode must not outlive the TUI: {context}"
    );
    assert!(!screen.alternate_screen(), "{context}");
    assert!(screen.cursor_visible(), "{context}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn sigterm_restores_the_terminal() {
    killed_by("TERM", 15);
}

#[test]
fn sighup_restores_the_terminal() {
    killed_by("HUP", 1);
}

#[test]
fn sigquit_restores_the_terminal() {
    killed_by("QUIT", 3);
}
