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

/// Boots the event loop for the interactive dashboard.
///
/// `flake` may carry the flake root; `None` resolves to the current
/// directory. Terminal restoration is guaranteed both on normal quit and on
/// panic. The shared `AppContext` is threaded through the event loop so action
/// keypresses can dispatch real deploy/rollback/diff use cases on the selected
/// host.
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
        drive_events(&mut app, &mut terminal, flake_path, &ctx).await?;
    }

    reset_terminal().map_err(io_err)?;
    Ok(())
}

/// Handles one input batch for the current loop iteration.
///
/// The operation keys (`s`/`r`/`d`) dispatch real single-host use cases on the
/// selected host, passing the shared context through. A long deploy runs
/// **inline** (awaited here), blocking the TUI until it completes — acceptable
/// for v2; a background-task render is future work and out of scope.
async fn drive_events(
    app: &mut DashboardApp,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    flake_path: &Path,
    ctx: &Arc<AppContext>,
) -> Result<(), NodError> {
    match poll_event(Duration::from_millis(250)) {
        Some(UiEvent::Resize(width, height)) => {
            terminal
                .resize(Rect::new(0, 0, width, height))
                .map_err(io_err)?;
        }
        Some(UiEvent::Key(key)) => {
            if let Some(action) = app.handle_key(key) {
                run_action(action, app, flake_path, ctx).await;
            }
        }
        None => {}
    }
    Ok(())
}

/// Dispatches an action intent from the view layer to the command layer.
///
/// The selected host (from `app.selected_host()`) is passed to the matching
/// single-host use case with the shared context and flake path:
///
/// - `Switch` → [`DeployFleetUseCase`] with `DeploymentOptions` defaulted to
///   `DeploymentAction::Switch`.
/// - `Rollback` → [`RollbackUseCase`].
/// - `Diff` → [`DetectDriftUseCase`] (non-verbose).
///
/// Each outcome — success or error — is written to the operation log and the
/// status line so the operator sees the result of the action they triggered.
/// If no host is selected the action is a no-op: a warning is logged and no
/// use case runs, so the dashboard never panics and never makes a silent
/// change.
async fn run_action(
    action: DashboardAction,
    app: &mut DashboardApp,
    flake_path: &Path,
    ctx: &Arc<AppContext>,
) {
    // AC3: without a selected host there is nothing to act on — warn and bail.
    let Some(host) = app.selected_host().cloned() else {
        app.append_log("warning: no host selected; action skipped");
        return;
    };

    let outcome = match action {
        DashboardAction::Switch => DeployFleetUseCase::new(ctx.clone())
            .execute(
                vec![host.clone()],
                DeploymentOptions::default_for(DeploymentAction::Switch),
                flake_path,
            )
            .await
            .map(|summary| {
                format!(
                    "switch complete ({} succeeded, {} failed)",
                    summary.succeeded(),
                    summary.failed()
                )
            }),
        DashboardAction::Rollback => RollbackUseCase::new(ctx.clone())
            .execute(&host)
            .await
            .map(|()| "rollback complete".to_string()),
        DashboardAction::Diff => DetectDriftUseCase::new(ctx.clone())
            .execute(&host, flake_path, false)
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
            app.append_log(&line);
            app.status_message = Some(line);
        }
        Err(err) => {
            let line = format!("{}: error: {}", host.name, err);
            app.append_log(&line);
            app.status_message = Some(line);
        }
    }
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
