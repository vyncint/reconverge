//! The reconverge TUI binary.
//!
//! Usage:
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

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use reconverge_tui::inspect::{self, InspectorState, KeyAction};
use reconverge_tui::load;
use reconverge_tui::view::{self, ShellModel};
use reconverge_tui::{learn, triage, witness};

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> ExitCode {
    let mut ascii = false;
    let mut baseline: Option<PathBuf> = None;
    let mut mode: Option<String> = None;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
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
                println!(
                    "usage: reconverge-tui [inspect|witness|learn|triage] [--ascii] \
                     [--baseline <path>] <artifact.json>..."
                );
                return ExitCode::SUCCESS;
            }
            "inspect" | "witness" | "learn" | "triage" if mode.is_none() && paths.is_empty() => {
                mode = Some(arg);
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());

    let result = match mode.as_deref() {
        Some("inspect") => run_inspector(&paths, ascii, color),
        Some("witness") => run_witness(&paths, ascii, color),
        Some("learn") => run_learn(ascii, color),
        Some("triage") => {
            let Some(baseline) = baseline else {
                eprintln!("reconverge-tui: triage requires --baseline <path>");
                return ExitCode::from(2);
            };
            run_triage(&paths, &baseline, ascii, color)
        }
        _ => run_shell(&paths, ascii, color),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("reconverge-tui: {error}");
            ExitCode::from(2)
        }
    }
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
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
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
        terminal.draw(|frame| view::render(frame, &model))?;
        loop {
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press && is_quit(key.code, key.modifiers) =>
                {
                    return Ok(());
                }
                Event::Resize(_, _) => {
                    terminal.draw(|frame| view::render(frame, &model))?;
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
            terminal
                .draw(|frame| {
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
                .map(|_| ())
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

fn run_triage(paths: &[PathBuf], baseline_path: &Path, ascii: bool, color: bool) -> io::Result<()> {
    let (data, baseline) = triage::data::load(paths, baseline_path);
    let mut state = triage::TriageState::new(baseline);

    with_terminal(|terminal| {
        let draw = |state: &triage::TriageState, terminal: &mut Term| -> io::Result<()> {
            terminal
                .draw(|frame| {
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
                .map(|_| ())
        };
        draw(&state, terminal)?;
        loop {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    use triage::KeyAction as T;
                    // Ctrl-C always leaves, even mid-reason.
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
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
                        // one path the launcher named.
                        let outcome = state
                            .baseline
                            .write_to(&data.baseline_path)
                            .map_err(|e| e.to_string());
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
    })
}

fn run_learn(ascii: bool, color: bool) -> io::Result<()> {
    let lessons = learn::lessons();
    let mut state = learn::LearnState::new();

    with_terminal(|terminal| {
        let draw = |state: &learn::LearnState, terminal: &mut Term| -> io::Result<()> {
            terminal
                .draw(|frame| {
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
                .map(|_| ())
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
            terminal
                .draw(|frame| {
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
                .map(|_| ())
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
