//! Ratatui views for the interactive dashboard.
//!
//! Rendering is pure: each frame is derived from a `DashboardApp` plus the
//! latest per-host reachability snapshot, drawn to any [`Backend`]. This lets
//! the unit tests render a full frame into a `TestBackend` buffer without
//! opening a real terminal.

use ratatui::prelude::*;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Cell, Paragraph, Row, Table};

use crate::domain::host::HostEntity;
use crate::ui::app::{DashboardApp, Tab};

/// Draws one full dashboard frame: header, body pane and footer help.
pub fn draw(f: &mut Frame, app: &DashboardApp, online: &[bool]) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(f.size());

    draw_header(f, areas[0], app, online);
    draw_body(f, areas[1], app, online);
    draw_footer(f, areas[2]);
}

/// Header: brand identity, total host count and the online/offline split.
fn draw_header(f: &mut Frame, area: Rect, app: &DashboardApp, online: &[bool]) {
    let online_n = count_online(online);
    let offline_n = online.len() - online_n;

    let line = Line::from(vec![
        Span::styled(
            " ◈ nod ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " Nix Orchestration & Deployment Dashboard ",
            Style::new()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} hosts ", app.hosts.len()),
            Style::new().fg(Color::Gray),
        ),
        Span::styled(
            format!(" online {} ", online_n),
            Style::new().fg(Color::Green),
        ),
        Span::styled(
            format!(" offline {} ", offline_n),
            Style::new().fg(Color::Red),
        ),
    ]);

    let paragraph = Paragraph::new(line)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Fleet Overview ")
                .title_style(Style::new().fg(Color::Cyan)),
        )
        .style(Style::new().bg(Color::Black));

    f.render_widget(paragraph, area);
}

/// Dispatches the body pane by the active tab.
fn draw_body(f: &mut Frame, area: Rect, app: &DashboardApp, online: &[bool]) {
    match app.active_tab {
        Tab::Matrix => render_matrix(f, area, app, online),
        Tab::Details => render_details(f, area, app),
        Tab::Logs => render_logs(f, area, app),
    }
}

/// Host reachability matrix table.
fn render_matrix(f: &mut Frame, area: Rect, app: &DashboardApp, online: &[bool]) {
    let header = Row::new(vec![
        Cell::new("Name"),
        Cell::new("Role"),
        Cell::new("Target"),
        Cell::new("Tags"),
        Cell::new("Reachability"),
        Cell::new("Active Closure"),
    ])
    .style(Style::new().add_modifier(Modifier::BOLD).fg(Color::Black))
    .height(1);

    let mut rows = Vec::new();
    for i in 0..app.hosts.len() {
        let host = &app.hosts[i];
        let highlighted = i == app.selected_index;
        let row_style = if highlighted {
            Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::new().fg(Color::Gray)
        };
        let is_up = i < online.len() && online[i];
        let status = if is_up { "● online" } else { "○ offline" };
        rows.push(
            Row::new(vec![
                Cell::new(host.name.clone()),
                Cell::new(host.role.to_str()),
                Cell::new(host.target_host.clone()),
                Cell::new(tags_text(host)),
                Cell::new(status),
                Cell::new(closure_text(host)),
            ])
            .style(row_style)
            .height(1),
        );
    }

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Min(18),
            Constraint::Min(10),
            Constraint::Length(12),
            Constraint::Min(14),
        ],
    )
    .header(header)
    .column_spacing(1)
    .block(
        Block::bordered()
            .border_type(BorderType::Rounded)
            .title(" Host Matrix "),
    );

    f.render_widget(table, area);
}

/// Detail view for the currently selected host.
fn render_details(f: &mut Frame, area: Rect, app: &DashboardApp) {
    let lines = app
        .selected_host()
        .map(|host| {
            let profile = host.ssh_profile();
            vec![
                Line::styled(
                    " Selected Host ",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Line::from(format!("  name      {}", host.name)),
                Line::from(format!("  role      {}", host.role.to_str())),
                Line::from(format!("  target    {}", host.target_host)),
                Line::from(format!(
                    "  ssh       {}@{}:{}",
                    profile.user(),
                    host.target_host,
                    profile.port()
                )),
                Line::from(format!("  closure   {}", closure_text(host))),
                Line::from(format!("  tags      {}", tags_text(host))),
                Line::from(format!("  locality  {}", locality_text(host))),
            ]
        })
        .unwrap_or(vec![Line::from("  no host selected")]);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Detail "),
        )
        .style(Style::new().fg(Color::Gray));

    f.render_widget(paragraph, area);
}

/// Operation log stream, newest entries first.
fn render_logs(f: &mut Frame, area: Rect, app: &DashboardApp) {
    let mut newest = Vec::new();
    let n = app.logs.len();
    for i in 0..n {
        newest.push(Line::from(format!("  {}", app.logs[n - 1 - i])));
    }
    if newest.is_empty() {
        newest.push(Line::from("  no operations yet"));
    }

    let paragraph = Paragraph::new(newest)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Operation Log "),
        )
        .style(Style::new().fg(Color::Gray));

    f.render_widget(paragraph, area);
}

/// Keybinding help along the bottom edge.
///
/// The `s`/`r`/`d` keys are **live** actions: they perform a real
/// switch/rollback/diff on the currently selected host. They are labelled as
/// such so the operator knows a keypress triggers a state-changing operation.
fn draw_footer(f: &mut Frame, area: Rect) {
    let paragraph = Paragraph::new(Line::from(
        "[q] Quit | [j/k] Navigate | [s] live switch | [r] live rollback | [d] live diff | [Tab] Switch Pane",
    ))
    .style(Style::new().fg(Color::DarkGray))
    .centered();

    f.render_widget(paragraph, area);
}

/// Comma-joined tag list, or a dash when the host has none.
fn tags_text(host: &HostEntity) -> String {
    if host.tags.is_empty() {
        return String::from("-");
    }
    let mut parts = String::new();
    for (i, tag) in host.tags.iter().enumerate() {
        if i > 0 {
            parts = format!("{}, {}", parts, tag);
        } else {
            parts = tag.to_string();
        }
    }
    parts
}

/// The active closure path, or a placeholder when the host has none.
fn closure_text(host: &HostEntity) -> String {
    match &host.active_closure {
        Some(path) => path.display().to_string(),
        None => String::from("unattached"),
    }
}

/// Human-readable locality for the detail pane.
fn locality_text(host: &HostEntity) -> String {
    if host.is_local {
        String::from("localhost")
    } else {
        String::from("remote")
    }
}

/// Counts the reachable hosts in a reachability snapshot.
fn count_online(online: &[bool]) -> usize {
    let mut n: usize = 0;
    for up in online.iter() {
        if *up {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::draw;
    use crate::domain::host::{HostEntity, HostRole};
    use crate::ui::app::{DashboardApp, DashboardKey};
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    fn fleet() -> Vec<HostEntity> {
        let mut jello = HostEntity::new("jello", "jello-machine", true);
        jello.role = HostRole::parse("desktop");
        jello.tags = vec!["home".to_string()];
        vec![
            jello,
            HostEntity::new("atlas", "10.0.0.8", false),
            HostEntity::new("orbit", "10.0.0.9", false),
        ]
    }

    fn render(app: &DashboardApp, online: &[bool]) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, app, online)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_text(buffer: &Buffer, y: u16) -> String {
        let mut s = String::new();
        for x in 0..buffer.area().width {
            s.push_str(buffer.get(x, y).symbol());
        }
        s
    }

    fn board_text(buffer: &Buffer) -> String {
        let mut s = String::new();
        for y in 0..buffer.area().height {
            s.push_str(row_text(buffer, y).as_str());
            s.push('\n');
        }
        s
    }

    fn contains_text(board: &str, needle: &str) -> bool {
        board.find(needle).is_some()
    }

    #[test]
    fn matrix_frame_lists_every_host_and_counts() {
        let app = DashboardApp::new(fleet());
        let online = vec![true, false, true];
        let buffer = render(&app, &online);
        let board = board_text(&buffer);

        assert!(contains_text(&board, "Fleet Overview"));
        assert!(contains_text(&board, "Host Matrix"));
        assert!(contains_text(&board, "jello"));
        assert!(contains_text(&board, "atlas"));
        assert!(contains_text(&board, "orbit"));
        assert!(contains_text(&board, "hosts"));
        assert!(contains_text(&board, "online"));
        assert!(contains_text(&board, "offline"));
    }

    #[test]
    fn matrix_frame_has_the_footer_keybindings() {
        let app = DashboardApp::new(fleet());
        let online = vec![false, false, false];
        let board = board_text(&render(&app, &online));

        assert!(contains_text(&board, "[q]"));
        assert!(contains_text(&board, "[j/k]"));
        assert!(contains_text(&board, "[s]"));
        assert!(contains_text(&board, "[r]"));
        assert!(contains_text(&board, "[d]"));
        assert!(contains_text(&board, "[Tab]"));
    }

    #[test]
    fn footer_does_not_label_actions_as_preview() {
        let app = DashboardApp::new(fleet());
        let online = vec![false, false, false];
        let board = board_text(&render(&app, &online));

        assert!(!contains_text(&board, "preview"));
        assert!(contains_text(&board, "live switch"));
        assert!(contains_text(&board, "live rollback"));
        assert!(contains_text(&board, "live diff"));
    }

    #[test]
    fn details_pane_names_the_selected_host() {
        let mut app = DashboardApp::new(fleet());
        app.handle_key(DashboardKey::Down);
        app.toggle_tab();

        let online = vec![true, false, true];
        let board = board_text(&render(&app, &online));
        assert!(contains_text(&board, "atlas"));
        assert!(contains_text(&board, "10.0.0.8"));
    }

    #[test]
    fn logs_pane_renders_recorded_operations() {
        let mut app = DashboardApp::new(fleet());
        app.toggle_tab(); // Matrix -> Details
        app.toggle_tab(); // Details -> Logs
        app.append_log("switch jello");

        let online = vec![true, false, true];
        let board = board_text(&render(&app, &online));
        assert!(contains_text(&board, "switch jello"));
    }
}
