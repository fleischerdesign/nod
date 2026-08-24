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
use std::sync::Arc;
use std::time::Duration;

use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};

use crate::application::context::AppContext;
use crate::application::use_cases::deploy_fleet::DeployFleetUseCase;
use crate::application::use_cases::detect_drift::DetectDriftUseCase;
use crate::application::use_cases::rollback::RollbackUseCase;
use crate::domain::errors::NodError;
use crate::domain::plan::{DeploymentAction, DeploymentOptions};
use crate::ui::app::{DashboardAction, DashboardApp};
use crate::ui::event::{poll as poll_event, UiEvent};

/// Message streamed from background tasks to update dashboard state.
enum AppUpdate {
    Log(String),
    Status(String),
}

/// Boots the event loop for the interactive dashboard.
///
/// `flake` may carry the flake root; `None` resolves to the current
/// directory. Terminal restoration is guaranteed both on normal quit and on
/// panic. The shared `AppContext` is threaded through the event loop so action
/// keypresses can dispatch real deploy/rollback/diff use cases on the selected
/// host asynchronously without blocking the UI.
pub async fn run_dashboard(ctx: Arc<AppContext>, flake: Option<PathBuf>) -> Result<(), NodError> {
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppUpdate>();

    // Restore a sane terminal even when a panic interrupts the loop.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let _ = reset_terminal();
        original_hook(panic);
    }));

    let mut terminal = init_terminal().map_err(io_err)?;

    loop {
        // Drain any pending log/status updates from background tasks
        while let Ok(update) = rx.try_recv() {
            match update {
                AppUpdate::Log(line) => app.append_log(&line),
                AppUpdate::Status(line) => app.status_message = Some(line),
            }
        }

        terminal
            .draw(|frame| views::draw(frame, &app, &online))
            .map_err(io_err)?;
        if app.should_quit {
            break;
        }
        drive_events(&mut app, &mut terminal, flake_path, &ctx, &tx).await?;
    }

    reset_terminal().map_err(io_err)?;
    Ok(())
}

/// Handles one input batch for the current loop iteration.
///
/// The operation keys (`s`/`r`/`d`) dispatch real single-host use cases on the
/// selected host asynchronously in background tasks, keeping the TUI fluid
/// and responsive (ADR-009).
async fn drive_events(
    app: &mut DashboardApp,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    flake_path: &Path,
    ctx: &Arc<AppContext>,
    tx: &tokio::sync::mpsc::UnboundedSender<AppUpdate>,
) -> Result<(), NodError> {
    match poll_event(Duration::from_millis(100)) {
        Some(UiEvent::Resize(width, height)) => {
            terminal
                .resize(Rect::new(0, 0, width, height))
                .map_err(io_err)?;
        }
        Some(UiEvent::Key(key)) => {
            if let Some(action) = app.handle_key(key) {
                spawn_action(action, app, flake_path, ctx.clone(), tx.clone());
            }
        }
        None => {}
    }
    Ok(())
}

/// Spawns a background task for an action intent from the view layer (ADR-009).
///
/// The selected host is passed to the matching single-host use case with the
/// shared context and flake path in a detached Tokio task, streaming log/status
/// outcomes through the channel back to the event loop.
fn spawn_action(
    action: DashboardAction,
    app: &mut DashboardApp,
    flake_path: &Path,
    ctx: Arc<AppContext>,
    tx: tokio::sync::mpsc::UnboundedSender<AppUpdate>,
) {
    let Some(host) = app.selected_host().cloned() else {
        app.append_log("warning: no host selected; action skipped");
        return;
    };

    let flake_path = flake_path.to_path_buf();
    tokio::spawn(async move {
        let outcome = match action {
            DashboardAction::Switch => DeployFleetUseCase::new(ctx)
                .execute(
                    vec![host.clone()],
                    DeploymentOptions::default_for(DeploymentAction::Switch),
                    &flake_path,
                )
                .await
                .map(|summary| {
                    format!(
                        "switch complete ({} succeeded, {} failed)",
                        summary.succeeded(),
                        summary.failed()
                    )
                }),
            DashboardAction::Rollback => RollbackUseCase::new(ctx)
                .execute(&host)
                .await
                .map(|()| "rollback complete".to_string()),
            DashboardAction::Diff => DetectDriftUseCase::new(ctx)
                .execute(&host, &flake_path, false)
                .await
                .map(|report| {
                    if report.drifted {
                        "diff: drifted".to_string()
                    } else {
                        "diff: in sync".to_string()
                    }
                }),
        };

        match outcome {
            Ok(message) => {
                let line = format!("{}: {}", host.name, message);
                let _ = tx.send(AppUpdate::Log(line.clone()));
                let _ = tx.send(AppUpdate::Status(line));
            }
            Err(err) => {
                let line = format!("{}: error: {}", host.name, err);
                let _ = tx.send(AppUpdate::Log(line.clone()));
                let _ = tx.send(AppUpdate::Status(line));
            }
        }
    });
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
