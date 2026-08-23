//! App model for the interactive dashboard: pure, IO-free state machine.
//!
//! `DashboardApp` owns the fleet matrix selection, the active pane and the
//! operation log. Rendering is deliberately absent — `views` turns this state
//! into Ratatui widgets, and the unit tests exercise the transitions here
//! without any terminal.

use crate::domain::host::HostEntity;

/// The active pane rendered in the dashboard body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    /// Host reachability matrix table.
    Matrix,
    /// Detail view for the selected host.
    Details,
    /// Operation log / event stream.
    Logs,
}

/// Normalized key events the model understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardKey {
    /// Up / `k`.
    Up,
    /// Down / `j`.
    Down,
    /// `Tab` — switch the active pane.
    Tab,
    /// `q` — request a clean quit.
    Quit,
    /// `s` — trigger a switch intent.
    Switch,
    /// `r` — trigger a rollback intent.
    Rollback,
    /// `d` — trigger a diff intent.
    Diff,
}

/// Operations the dashboard can hand off to the command layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardAction {
    Switch,
    Rollback,
    Diff,
}

/// Stateful dashboard model; mutates only via the provided handlers.
pub struct DashboardApp {
    /// Fleet loaded from Nix host discovery.
    pub hosts: Vec<HostEntity>,
    /// Row currently highlighted in the host matrix.
    pub selected_index: usize,
    /// Active pane.
    pub active_tab: Tab,
    /// Most recent log entries, newest last.
    pub logs: Vec<String>,
    /// Transient operator-facing status line.
    pub status_message: Option<String>,
    /// Requests the event loop to terminate after this frame.
    pub should_quit: bool,
}

impl DashboardApp {
    /// Builds a dashboard over `hosts` with default selection and pane.
    pub fn new(hosts: Vec<HostEntity>) -> Self {
        Self {
            hosts,
            selected_index: 0,
            active_tab: Tab::Matrix,
            logs: Vec::new(),
            status_message: None,
            should_quit: false,
        }
    }

    /// Returns the currently selected host, or `None` on an empty fleet.
    pub fn selected_host(&self) -> Option<&HostEntity> {
        if self.hosts.is_empty() {
            None
        } else {
            Some(&self.hosts[self.selected_index])
        }
    }

    /// Advances the selection, wrapping from the last host to the first.
    pub fn next_host(&mut self) {
        if self.hosts.is_empty() {
            return;
        }
        if self.selected_index + 1 < self.hosts.len() {
            self.selected_index += 1;
        } else {
            self.selected_index = 0;
        }
    }

    /// Moves the selection backwards, wrapping from the first to the last host.
    pub fn previous_host(&mut self) {
        if self.hosts.is_empty() {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = self.hosts.len() - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    /// Cycles the active pane Matrix → Details → Logs → Matrix.
    pub fn toggle_tab(&mut self) {
        self.active_tab = match self.active_tab {
            Tab::Matrix => Tab::Details,
            Tab::Details => Tab::Logs,
            Tab::Logs => Tab::Matrix,
        };
    }

    /// Appends an operation log entry.
    pub fn append_log(&mut self, entry: &str) {
        self.logs.push(entry.to_string());
    }

    /// Applies a normalized keypress.
    ///
    /// Navigation and pane keys mutate selection/state directly; quit sets the
    /// quit flag; the three action keys return the operation intent so the
    /// event loop can forward it to the command layer.
    pub fn handle_key(&mut self, key: DashboardKey) -> Option<DashboardAction> {
        match key {
            DashboardKey::Down => {
                self.next_host();
                None
            }
            DashboardKey::Up => {
                self.previous_host();
                None
            }
            DashboardKey::Tab => {
                self.toggle_tab();
                None
            }
            DashboardKey::Quit => {
                self.should_quit = true;
                None
            }
            DashboardKey::Switch => {
                let host = self
                    .selected_host()
                    .map(|h| h.name.clone())
                    .unwrap_or("?".to_string());
                self.status_message = Some(format!("switch queued for {}", host));
                Some(DashboardAction::Switch)
            }
            DashboardKey::Rollback => {
                let host = self
                    .selected_host()
                    .map(|h| h.name.clone())
                    .unwrap_or("?".to_string());
                self.status_message = Some(format!("rollback queued for {}", host));
                Some(DashboardAction::Rollback)
            }
            DashboardKey::Diff => {
                let host = self
                    .selected_host()
                    .map(|h| h.name.clone())
                    .unwrap_or("?".to_string());
                self.status_message = Some(format!("diff queued for {}", host));
                Some(DashboardAction::Diff)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::HostRole;

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

    #[test]
    fn new_selects_first_host() {
        let app = DashboardApp::new(fleet());
        assert_eq!(app.selected_index, 0);
        assert!(app.selected_host().is_some());
        assert_eq!(app.selected_host().unwrap().name, "jello");
    }

    #[test]
    fn next_host_advances_then_wraps() {
        let mut app = DashboardApp::new(fleet());

        app.next_host();
        assert_eq!(app.selected_host().unwrap().name, "atlas");

        app.next_host();
        assert_eq!(app.selected_host().unwrap().name, "orbit");

        // Wraps back to the first host.
        app.next_host();
        assert_eq!(app.selected_host().unwrap().name, "jello");
    }

    #[test]
    fn previous_host_wraps_forward() {
        let mut app = DashboardApp::new(fleet());

        // Moving up past the first host wraps to the last.
        app.previous_host();
        assert_eq!(app.selected_host().unwrap().name, "orbit");

        app.previous_host();
        assert_eq!(app.selected_host().unwrap().name, "atlas");
    }

    #[test]
    fn navigation_is_harmless_on_an_empty_fleet() {
        let mut app = DashboardApp::new(Vec::new());
        assert!(app.selected_host().is_none());

        app.next_host();
        app.previous_host();
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn tab_cycles_matrix_details_logs() {
        let mut app = DashboardApp::new(fleet());
        assert_eq!(app.active_tab, Tab::Matrix);

        app.toggle_tab();
        assert_eq!(app.active_tab, Tab::Details);

        app.toggle_tab();
        assert_eq!(app.active_tab, Tab::Logs);

        app.toggle_tab();
        assert_eq!(app.active_tab, Tab::Matrix);
    }

    #[test]
    fn selection_survives_a_pane_switch() {
        let mut app = DashboardApp::new(fleet());
        app.next_host();
        app.next_host();

        app.toggle_tab();
        app.toggle_tab();

        assert_eq!(app.selected_host().unwrap().name, "orbit");
    }

    #[test]
    fn append_log_records_entries() {
        let mut app = DashboardApp::new(fleet());
        app.append_log("switch jello");
        app.append_log("rollback orbit");

        assert_eq!(app.logs.len(), 2);
        assert_eq!(app.logs[0], "switch jello");
        assert_eq!(app.logs[1], "rollback orbit");
    }

    #[test]
    fn handle_key_navigates_switches_panes_and_quits() {
        let mut app = DashboardApp::new(fleet());

        app.handle_key(DashboardKey::Down);
        assert_eq!(app.selected_host().unwrap().name, "atlas");

        app.handle_key(DashboardKey::Tab);
        assert_eq!(app.active_tab, Tab::Details);

        app.handle_key(DashboardKey::Up);
        assert_eq!(app.selected_host().unwrap().name, "jello");

        app.handle_key(DashboardKey::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn action_keys_emit_intents_and_note_the_selected_host() {
        let mut app = DashboardApp::new(fleet());

        let switched = app.handle_key(DashboardKey::Switch);
        assert_eq!(switched, Some(DashboardAction::Switch));
        assert_eq!(
            app.status_message,
            Some("switch queued for jello".to_string())
        );

        app.next_host();

        let rolled = app.handle_key(DashboardKey::Rollback);
        assert_eq!(rolled, Some(DashboardAction::Rollback));
        assert_eq!(
            app.status_message,
            Some("rollback queued for atlas".to_string())
        );

        let diffed = app.handle_key(DashboardKey::Diff);
        assert_eq!(diffed, Some(DashboardAction::Diff));
        assert_eq!(
            app.status_message,
            Some("diff queued for atlas".to_string())
        );
    }
}
