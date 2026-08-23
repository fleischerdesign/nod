//! Ratatui interactive dashboard for fleet monitoring and management.
//!
//! `run_dashboard` boots the terminal (raw mode + alternate screen), installs
//! a panic hook that restores the terminal on a crash, runs the event loop,
//! and always restores the terminal before returning.

pub mod app;
pub mod event;
pub mod views;

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};

use crate::application::context::AppContext;
use crate::domain::errors::NodError;
use crate::ui::app::{DashboardAction, DashboardApp};
use crate::ui::event::{poll as poll_event, UiEvent};

/// Boots the event loop for the interactive dashboard.
///
/// `flake` may carry the flake root; `None` resolves to the current
/// directory. Terminal restoration is guaranteed both on normal quit and on
/// panic.
pub async fn run_dashboard(ctx: AppContext, flake: Option<PathBuf>) -> Result<(), NodError> {
    let flake_buf = flake.unwrap_or(PathBuf::from("."));
    let flake_path: &Path = flake_buf.as_path();

    let evaluator = ctx.evaluator();
    let hosts = evaluator.discover_hosts(flake_path, false).await?;
    if hosts.is_empty() {
        println!("Dashboard: no hosts discovered in the flake.");
        return Ok(());
    }

    // Snapshot reachability before entering the loop so the header shows live
    // online/offline counts without blocking redraws.
    let mut online = Vec::new();
    for host in hosts.iter() {
        let deployer = ctx.deployer_for(host);
        let up = deployer.check_reachability(host).await.unwrap_or(false);
        online.push(up);
    }

    let mut app = DashboardApp::new(hosts);

    // Restore a sane terminal even when a panic interrupts the loop.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        reset_terminal().unwrap();
        original_hook(panic);
    }));

    let mut terminal = init_terminal().map_err(io_err)?;

    loop {
        terminal
            .draw(|frame| views::draw(frame, &app, &online))
            .map_err(io_err)?;
        if app.should_quit {
            break;
        }
        drive_events(&mut app, &mut terminal, flake_path)?;
    }

    reset_terminal().map_err(io_err)?;
    Ok(())
}

/// Handles one input batch for the current loop iteration.
fn drive_events(
    app: &mut DashboardApp,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    flake_path: &Path,
) -> Result<(), NodError> {
    match poll_event(Duration::from_millis(250)) {
        Some(UiEvent::Resize(width, height)) => {
            terminal
                .resize(Rect::new(0, 0, width, height))
                .map_err(io_err)?;
        }
        Some(UiEvent::Key(key)) => {
            if let Some(action) = app.handle_key(key) {
                run_action(action, app, flake_path);
            }
        }
        None => {}
    }
    Ok(())
}

/// Dispatches an action intent from the view layer to the operation log and
/// the status line.
fn run_action(action: DashboardAction, app: &mut DashboardApp, flake_path: &Path) {
    let label = match action {
        DashboardAction::Switch => "switch",
        DashboardAction::Rollback => "rollback",
        DashboardAction::Diff => "diff",
    };
    let detail = app
        .selected_host()
        .map(|h| h.name.clone())
        .unwrap_or("unknown".to_string());
    app.append_log(format!("{} {} → {}", label, detail, flake_path.display()).as_str());
}

/// Enters raw mode, switches to the alternate screen and hides the cursor.
fn init_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    enable_raw_mode()?;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.hide_cursor()?;
    Ok(terminal)
}

/// Leaves the alternate screen, disables raw mode and restores the cursor.
fn reset_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

/// Carries an `io::Error` across the `NodError` boundary.
fn io_err(err: std::io::Error) -> NodError {
    NodError::internal(format!("TUI terminal error: {}", err))
}
