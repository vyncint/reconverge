//! The reconverge TUI binary.
//!
//! Usage:
//!   `reconverge-tui --version | --help`
//!   `reconverge-tui [--ascii] <artifact.json>...`      — the artifact shell
//!   `reconverge-tui inspect [--ascii] <artifact.json>...`    — the Inspector
//!   `reconverge-tui witness [--ascii] <witness.json>...`
//!                                                — the 32-lane debugger
//!   `reconverge-tui learn [--ascii]`            — the SIMT lessons
//!                                                (fully embedded, offline)
//!   `reconverge-tui triage [--ascii] --baseline <path>
//!    <findings.json>...`                          — interactive review
//!
//! Event-driven by construction: one draw at startup, then redraws only on
//! input that changes state, or on resize. No timers, no animation
//! (docs/ARCHITECTURE.md) — which is what makes termlens's quiet-period detection
//! reliable in the smoke and flow tests.

#![forbid(unsafe_code)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use reconverge_tui::inspect::{self, InspectorState, KeyAction};
use reconverge_tui::load;
use reconverge_tui::view::{self, ShellModel};
use reconverge_tui::{learn, triage, witness};

type Term = Terminal<CrosstermBackend<io::Stdout>>;

/// One repaint, bracketed in DEC 2026 synchronized updates.
///
/// Without the brackets a terminal shows whatever has arrived so far, which is
/// usually fine to look at and fatal to test: the golden-frame gate in
/// `reconverge-tui/tests/smoke.rs` used to sync on a 150ms quiet period, and a
/// loaded macOS runner that paused mid-repaint handed it a torn frame. That is
/// a real flake, not a slow machine — it fails against a golden that is
/// correct.
///
/// With them, termlens's `wait_frame` can wait for a *complete* frame instead
/// of for silence, so the test is gated on the repaint finishing rather than on
/// a duration nobody can pick correctly.
///
/// `End` is sent even when the draw fails, so a terminal is never left inside
/// an update that never closes.
fn sync_draw(terminal: &mut Term, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
    execute!(io::stdout(), BeginSynchronizedUpdate)?;
    let drawn = terminal.draw(render).map(|_| ());
    execute!(io::stdout(), EndSynchronizedUpdate)?;
    drawn
}

const USAGE: &str = "\
usage: reconverge-tui [--ascii] <artifact.json>...
       reconverge-tui inspect [--ascii] <unimap.json> [findings.json]...
       reconverge-tui witness [--ascii] <witness.json>...
       reconverge-tui learn [--ascii]
       reconverge-tui triage [--ascii] --baseline <path> <findings.json>...
       reconverge-tui --version | --help

Usually launched by `cargo reconverge`, which picks the artifacts.
";

fn main() -> ExitCode {
    let mut ascii = false;
    let mut baseline: Option<PathBuf> = None;
    let mut mode: Option<String> = None;
    let mut paths = Vec::new();
    let mut only_paths = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if only_paths {
            paths.push(PathBuf::from(arg));
            continue;
        }
        match arg.as_str() {
            "--ascii" => ascii = true,
            "--baseline" => match args.next() {
                Some(path) => baseline = Some(PathBuf::from(path)),
                None => {
                    eprintln!("reconverge-tui: --baseline requires a path");
                    return ExitCode::from(2);
                }
            },
            "--help" | "-h" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            // `cargo reconverge setup` installs this binary beside the CLI
            // and the driver, and the three must be one release; this is how
            // a user checks the third.
            "--version" | "-V" => {
                println!("reconverge-tui {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--" => only_paths = true,
            "inspect" | "witness" | "learn" | "triage" if mode.is_none() && paths.is_empty() => {
                mode = Some(arg);
            }
            // Anything else that looks like a flag is a typo, not a file: a
            // misspelt `--asci` used to be opened as an artifact and reported
            // inside the view as `No such file or directory`.
            flag if flag.starts_with('-') && flag != "-" => {
                eprintln!("reconverge-tui: unknown option `{flag}`\n\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());

    let result = match mode.as_deref() {
        Some("inspect") => run_inspector(&paths, ascii, color).map(|()| ExitCode::SUCCESS),
        Some("witness") => run_witness(&paths, ascii, color).map(|()| ExitCode::SUCCESS),
        Some("learn") => run_learn(ascii, color).map(|()| ExitCode::SUCCESS),
        Some("triage") => {
            let Some(baseline) = baseline else {
                eprintln!("reconverge-tui: triage requires --baseline <path>");
                return ExitCode::from(2);
            };
            run_triage(&paths, &baseline, ascii, color)
        }
        _ => run_shell(&paths, ascii, color).map(|()| ExitCode::SUCCESS),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("reconverge-tui: {error}");
            ExitCode::from(2)
        }
    }
}

/// Put the terminal back the way the shell had it: cooked mode, the main
/// screen, a visible cursor, and no synchronized update left open — the
/// last because a signal can land mid-repaint. Shared by the panic hook and
/// the signal thread, which have no caller to report an error to.
fn restore_terminal(out: &mut impl io::Write) {
    let _ = disable_raw_mode();
    let _ = execute!(out, EndSynchronizedUpdate, LeaveAlternateScreen, Show);
}

/// Restore the terminal on SIGTERM, SIGHUP and SIGQUIT, then die of the same
/// signal.
///
/// Without this, `timeout`, `tmux kill-pane` or a parent shutting down left
/// the user at a shell in raw mode, on the alternate screen, with no cursor
/// and no echo — `q`, Ctrl-C and a panic all restored, and the signals that
/// end a process from outside did not. SIGINT is not here: raw mode turns
/// Ctrl-C into a key, which every view already handles.
///
/// A thread rather than a flag polled by the event loop: polling would need
/// a read timeout, and the views are event-driven by construction — no
/// timers, so termlens' frame detection stays exact. The thread takes the
/// stdout lock and never releases it, so the event loop cannot paint over
/// the restored screen in the moment before the process ends. The signal is
/// then re-raised with its default disposition, so whoever sent it sees a
/// death by that signal (shell status 128+N), not a clean exit.
#[cfg(unix)]
fn restore_on_signal() -> io::Result<()> {
    use signal_hook::consts::{SIGHUP, SIGQUIT, SIGTERM};
    use signal_hook::low_level::emulate_default_handler;
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGHUP, SIGQUIT])?;
    std::thread::spawn(move || {
        let mut pending = signals.forever();
        let Some(signal) = pending.next() else {
            return;
        };
        // The restore waits for the stdout lock, and a repaint blocked on a
        // terminal that stopped reading holds it for as long as that lasts.
        // So the restore gets a thread of its own, and a second signal still
        // ends the process at once — as the first did before this handler.
        std::thread::spawn(move || {
            // Raw mode first: it needs no lock, so the shell gets its echo
            // back even if the screen never does.
            let _ = disable_raw_mode();
            let mut out = io::stdout().lock();
            restore_terminal(&mut out);
            let _ = emulate_default_handler(signal);
            // Not reached for these three signals, whose default is to end
            // the process; an exit status is still better than carrying on.
            std::process::exit(128 + signal);
        });
        if let Some(signal) = pending.next() {
            let _ = emulate_default_handler(signal);
            std::process::exit(128 + signal);
        }
    });
    Ok(())
}

/// Terminal setup/teardown shared by every view, with a panic hook that
/// restores the screen first so panics stay readable.
fn with_terminal(body: impl FnOnce(&mut Term) -> io::Result<()>) -> io::Result<()> {
    // Say what is actually wrong before raw-mode setup turns it into a
    // bare errno: every view is interactive, so a pipe or a CI job cannot
    // host it.
    use std::io::IsTerminal;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "an interactive terminal is required (stdin/stdout is not a TTY); \
             for machine-readable output use `cargo reconverge check \
             --message-format json` or `--sarif`",
        ));
    }
    // Before raw mode, so there is no instant in which a signal could leave
    // the terminal raw.
    #[cfg(unix)]
    restore_on_signal()?;
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(&mut io::stdout());
        default_hook(info);
    }));

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let result = body(&mut terminal);

    let _ = std::panic::take_hook();
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    result
}

fn is_quit(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('q') | KeyCode::Esc)
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
}

fn run_shell(paths: &[PathBuf], ascii: bool, color: bool) -> io::Result<()> {
    let mut model = ShellModel {
        ascii,
        color,
        ..ShellModel::default()
    };
    for path in paths {
        match load::load(path) {
            Ok(artifact) => model.artifacts.push(artifact),
            Err(error) => model.errors.push(error),
        }
    }

    with_terminal(|terminal| {
        sync_draw(terminal, |frame| view::render(frame, &model))?;
        loop {
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press && is_quit(key.code, key.modifiers) =>
                {
                    return Ok(());
                }
                Event::Resize(_, _) => {
                    sync_draw(terminal, |frame| view::render(frame, &model))?;
                }
                _ => {}
            }
        }
    })
}

fn run_witness(paths: &[PathBuf], ascii: bool, color: bool) -> io::Result<()> {
    let data = witness::data::load(paths);
    let mut state = witness::WitnessState::new(&data);

    with_terminal(|terminal| {
        let draw = |state: &witness::WitnessState, terminal: &mut Term| -> io::Result<()> {
            sync_draw(terminal, |frame| {
                witness::view::render(
                    frame,
                    &witness::view::WitnessView {
                        data: &data,
                        state,
                        ascii,
                        color,
                    },
                )
            })
        };
        draw(&state, terminal)?;
        loop {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if is_quit(key.code, key.modifiers) {
                        return Ok(());
                    }
                    use witness::KeyAction as W;
                    let action = match key.code {
                        KeyCode::Char('l') | KeyCode::Right => Some(W::StepForward),
                        KeyCode::Char('h') | KeyCode::Left => Some(W::StepBack),
                        KeyCode::Char('g') | KeyCode::Home => Some(W::First),
                        KeyCode::Char('G') | KeyCode::End => Some(W::Last),
                        KeyCode::Char('d') => Some(W::JumpDivergence),
                        KeyCode::Char('v') => Some(W::JumpVerdict),
                        KeyCode::Char('n') => Some(W::NextWitness),
                        KeyCode::Char('N') => Some(W::PrevWitness),
                        _ => None,
                    };
                    if let Some(action) = action
                        && state.update(action, &data)
                    {
                        draw(&state, terminal)?;
                    }
                }
                Event::Resize(_, _) => {
                    draw(&state, terminal)?;
                }
                _ => {}
            }
        }
    })
}

fn run_triage(
    paths: &[PathBuf],
    baseline_path: &Path,
    ascii: bool,
    color: bool,
) -> io::Result<ExitCode> {
    let (data, baseline) = triage::data::load(paths, baseline_path);
    let mut state = triage::TriageState::new(baseline);
    let mut interrupted = false;

    with_terminal(|terminal| {
        let draw = |state: &triage::TriageState, terminal: &mut Term| -> io::Result<()> {
            sync_draw(terminal, |frame| {
                triage::view::render(
                    frame,
                    &triage::view::TriageView {
                        data: &data,
                        state,
                        ascii,
                        color,
                    },
                )
            })
        };
        draw(&state, terminal)?;
        loop {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    use triage::KeyAction as T;
                    // Ctrl-C always leaves, even mid-reason — and says so
                    // below, after teardown, when that threw edits away.
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        interrupted = true;
                        return Ok(());
                    }
                    let editing = state.editing.is_some();
                    if !editing {
                        // `Q` discards unsaved edits deliberately; inside the
                        // reason editor it is just a capital letter.
                        if key.code == KeyCode::Char('Q') {
                            return Ok(());
                        }
                        if is_quit(key.code, key.modifiers) {
                            if state.request_quit() {
                                return Ok(());
                            }
                            draw(&state, terminal)?;
                            continue;
                        }
                    }
                    let action = if editing {
                        match key.code {
                            KeyCode::Char(c) => Some(T::ReasonChar(c)),
                            KeyCode::Backspace => Some(T::ReasonBackspace),
                            KeyCode::Enter => Some(T::ReasonCommit),
                            KeyCode::Esc => Some(T::ReasonCancel),
                            _ => None,
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('j') | KeyCode::Down => Some(T::Next),
                            KeyCode::Char('k') | KeyCode::Up => Some(T::Prev),
                            KeyCode::Char('s') | KeyCode::Enter => Some(T::BeginSuppress),
                            KeyCode::Char('u') => Some(T::Unsuppress),
                            KeyCode::Char('w') => Some(T::RequestWrite),
                            _ => None,
                        }
                    };
                    let mut changed = action.is_some_and(|action| state.update(action, &data));
                    if state.take_write_request() {
                        // The one filesystem write in the whole TUI, to the
                        // one path the launcher named — and only when this
                        // session can actually see what is in it. A baseline
                        // that did not parse still holds every reviewed
                        // acceptance, with its date and its ticket number,
                        // and `w` used to replace all of it with an empty
                        // document and report success.
                        let outcome = match &data.baseline_unreadable {
                            Some(why) => Err(format!("write refused — {why}; fix or delete it")),
                            None => state
                                .baseline
                                .write_to(&data.baseline_path)
                                .map_err(|e| e.to_string()),
                        };
                        state.record_write(outcome);
                        changed = true;
                    }
                    if changed {
                        draw(&state, terminal)?;
                    }
                }
                Event::Resize(_, _) => {
                    draw(&state, terminal)?;
                }
                _ => {}
            }
        }
    })?;

    // `q` asks before it discards, and Ctrl-C cannot: it is the exit that
    // always works. It used to exit 0 having dropped a review pass without a
    // word, so a script — or the person who pressed it — could not tell that
    // anything was lost. On stderr, after teardown, where it survives the
    // alternate screen; 130 is the shell's code for an interrupted command.
    let discarded = state.unsaved_edits();
    if interrupted && discarded > 0 {
        eprintln!(
            "reconverge-tui: discarded {discarded} unsaved baseline {}",
            reconverge_artifacts::plural(discarded, "edit", "edits")
        );
        return Ok(ExitCode::from(130));
    }
    Ok(ExitCode::SUCCESS)
}

fn run_learn(ascii: bool, color: bool) -> io::Result<()> {
    let lessons = learn::lessons();
    let mut state = learn::LearnState::new();

    with_terminal(|terminal| {
        let draw = |state: &learn::LearnState, terminal: &mut Term| -> io::Result<()> {
            sync_draw(terminal, |frame| {
                learn::view::render(
                    frame,
                    &learn::view::LearnView {
                        lessons: &lessons,
                        state,
                        ascii,
                        color,
                    },
                )
            })
        };
        draw(&state, terminal)?;
        loop {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    use learn::{KeyAction as L, Screen};
                    let on_page = state.screen == Screen::Page;
                    // Esc backs out of a page; anywhere else it quits, like
                    // every other view.
                    if key.code == KeyCode::Esc && on_page {
                        if state.update(L::Back, &lessons) {
                            draw(&state, terminal)?;
                        }
                        continue;
                    }
                    if is_quit(key.code, key.modifiers) {
                        return Ok(());
                    }
                    let action = match (on_page, key.code) {
                        (false, KeyCode::Char('j') | KeyCode::Down) => Some(L::Down),
                        (false, KeyCode::Char('k') | KeyCode::Up) => Some(L::Up),
                        (false, KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right) => {
                            Some(L::Open)
                        }
                        (true, KeyCode::Char('n') | KeyCode::Char(' ') | KeyCode::PageDown) => {
                            Some(L::NextPage)
                        }
                        (true, KeyCode::Char('p') | KeyCode::PageUp) => Some(L::PrevPage),
                        (true, KeyCode::Char('l') | KeyCode::Right) => Some(L::StepForward),
                        (true, KeyCode::Char('h') | KeyCode::Left) => Some(L::StepBack),
                        (true, KeyCode::Char('d')) => Some(L::JumpDivergence),
                        (true, KeyCode::Char('v')) => Some(L::JumpVerdict),
                        (true, KeyCode::Char('b')) => Some(L::Back),
                        _ => None,
                    };
                    if let Some(action) = action
                        && state.update(action, &lessons)
                    {
                        draw(&state, terminal)?;
                    }
                }
                Event::Resize(_, _) => {
                    draw(&state, terminal)?;
                }
                _ => {}
            }
        }
    })
}

fn run_inspector(paths: &[PathBuf], ascii: bool, color: bool) -> io::Result<()> {
    let data = inspect::data::load(paths);
    let mut state = InspectorState::new(&data);

    with_terminal(|terminal| {
        let draw = |state: &InspectorState, terminal: &mut Term| -> io::Result<()> {
            sync_draw(terminal, |frame| {
                inspect::view::render(
                    frame,
                    &inspect::view::InspectorView {
                        data: &data,
                        state,
                        ascii,
                        color,
                    },
                )
            })
        };
        draw(&state, terminal)?;
        loop {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if is_quit(key.code, key.modifiers) {
                        return Ok(());
                    }
                    let action = match key.code {
                        KeyCode::Char('j') | KeyCode::Down => Some(KeyAction::NextValue),
                        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::PrevValue),
                        KeyCode::Char('p') | KeyCode::Enter => Some(KeyAction::WalkProvenance),
                        KeyCode::Char('u') | KeyCode::Backspace => Some(KeyAction::WalkBack),
                        KeyCode::Char('n') => Some(KeyAction::NextFinding),
                        KeyCode::Char('N') => Some(KeyAction::PrevFinding),
                        KeyCode::Char('f') => Some(KeyAction::NextFunction),
                        _ => None,
                    };
                    if let Some(action) = action
                        && state.update(action, &data)
                    {
                        draw(&state, terminal)?;
                    }
                }
                Event::Resize(_, _) => {
                    draw(&state, terminal)?;
                }
                _ => {}
            }
        }
    })
}
