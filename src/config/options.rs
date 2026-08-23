use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "nod",
    version = "2.0.0",
    about = "Universal Nix Orchestration & Deployment Engine",
    long_about = "Universal, zero-config, high-performance Nix Flake deployment, orchestration, and monitoring engine."
)]
pub struct Cli {
    /// Enable verbose output with detailed build paths and metrics
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Quiet mode, suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Rebuild and activate NixOS configurations (local or remote)
    Switch {
        /// Target host: 'local', host name (e.g. 'jello'), or 'all' for fleet deployment
        #[arg(default_value = "local")]
        target: String,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Filter the fleet to hosts carrying this tag (repeatable)
        #[arg(long, value_name = "TAG")]
        tag: Option<String>,

        /// Filter the fleet to hosts with this role (e.g. 'server')
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,

        /// Override the SSH user for every targeted host
        #[arg(long, value_name = "USER")]
        user: Option<String>,

        /// Override the SSH port for every targeted host
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,

        /// Override the SSH identity file for every targeted host
        #[arg(long, value_name = "PATH")]
        identity_file: Option<String>,

        /// Preview the deployment plan without activating any host
        #[arg(long)]
        dry_run: bool,

        /// Maximum hosts deployed in-flight at once
        #[arg(long, value_name = "N", default_value = "4")]
        concurrency: usize,

        /// Rollout strategy: 'all', 'canary' or 'batch'
        #[arg(long, value_name = "STRATEGY", default_value = "batch")]
        strategy: String,

        /// Wave size for --strategy batch
        #[arg(long, value_name = "N", default_value = "0")]
        batch_size: usize,

        /// Abort the whole run on the first host failure
        #[arg(long)]
        fail_fast: bool,

        /// Attempt a rollback before failing a host
        #[arg(long)]
        auto_rollback: bool,

        /// Error mode: fail-fast (default) or continue-on-error
        #[arg(long, value_name = "MODE")]
        on_error: Option<String>,

        /// Deployment action: switch, boot or test
        #[arg(long, value_name = "ACTION", default_value = "switch")]
        action: String,
    },

    /// Run strict repository quality gates (nixfmt + deadnix + statix)
    Check {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Display live status matrix of all discovered NixOS hosts
    Status {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Filter the fleet to hosts carrying this tag
        #[arg(long, value_name = "TAG")]
        tag: Option<String>,

        /// Filter the fleet to hosts with this role
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,
    },

    /// Generate package & systemd unit diff preview before switching
    Diff {
        /// Target host name
        #[arg(default_value = "local")]
        target: String,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Filter the fleet to hosts carrying this tag
        #[arg(long, value_name = "TAG")]
        tag: Option<String>,

        /// Filter the fleet to hosts with this role
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,

        /// Override the SSH user for every targeted host
        #[arg(long, value_name = "USER")]
        user: Option<String>,

        /// Override the SSH port for every targeted host
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,

        /// Override the SSH identity file for every targeted host
        #[arg(long, value_name = "PATH")]
        identity_file: Option<String>,
    },

    /// Preview the deployment plan for a target without activating it
    Plan {
        /// Target host: 'local', host name, or 'all'
        #[arg(default_value = "local")]
        target: String,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Filter the fleet to hosts carrying this tag
        #[arg(long, value_name = "TAG")]
        tag: Option<String>,

        /// Filter the fleet to hosts with this role
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,

        /// Override the SSH user for every targeted host
        #[arg(long, value_name = "USER")]
        user: Option<String>,

        /// Override the SSH port for every targeted host
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,

        /// Override the SSH identity file for every targeted host
        #[arg(long, value_name = "PATH")]
        identity_file: Option<String>,
    },

    /// Roll back a host to its previous NixOS profile generation
    Rollback {
        /// Target host name
        target: String,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Override the SSH user for the target
        #[arg(long, value_name = "USER")]
        user: Option<String>,

        /// Override the SSH port for the target
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,

        /// Override the connection timeout for the target
        #[arg(long, value_name = "SECS")]
        timeout: Option<u32>,

        /// Target host 'local", host name, or 'all'
        #[arg(long, value_name = "TARGET")]
        target_opt: Option<String>,
    },

    /// Detect configuration drift between live closures and the flake
    Drift {
        /// Target host: 'local', a host name, or 'all' (defaults to 'local')
        #[arg(long, value_name = "TARGET")]
        target: Option<String>,

        /// Narrow drift detection to hosts carrying this tag
        #[arg(long, value_name = "TAG")]
        tag: Option<String>,

        /// Emit the drift report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Read the recorded deployment audit history
    History {
        /// Narrow the history to one host
        #[arg(long, value_name = "TARGET")]
        target: Option<String>,

        /// Cap the newest entries returned
        #[arg(long, value_name = "N")]
        limit: Option<usize>,

        /// Emit the history as JSON
        #[arg(long)]
        json: bool,
    },

    /// Launch interactive Ratatui TUI dashboard for fleet management
    Dashboard {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tag_role_user_and_port_flags() {
        let cli = Cli::parse_from(vec![
            "nod",
            "switch",
            "--flake", "/tmp/flake",
            "--tag", "server",
            "--role", "desktop",
            "--user", "philipp",
            "--port", "2200",
        ]);
        assert!(!cli.verbose);
        match cli.command {
            Commands::Switch {
                target,
                flake,
                tag,
                role,
                user,
                port,
                identity_file,
                dry_run,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                on_error,
                action,
            } => {
                assert_eq!(target, "local");
                assert_eq!(flake, "/tmp/flake");
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("desktop".to_string()));
                assert_eq!(user, Some("philipp".to_string()));
                assert_eq!(port, Some(2200));
                assert_eq!(identity_file, None);
                assert!(!dry_run);
                assert_eq!(concurrency, 4);
                assert_eq!(strategy, "batch");
                assert_eq!(batch_size, 0);
                assert!(!fail_fast);
                assert!(!auto_rollback);
                assert_eq!(on_error, None);
                assert_eq!(action, "switch");
            }
            _ => panic!("expected a switch command"),
        }
    }

    #[test]
    fn parses_dry_run_strategy_and_concurrency() {
        let cli = Cli::parse_from(vec![
            "nod",
            "switch",
            "--dry-run",
            "--concurrency", "2",
            "--strategy", "canary",
            "--batch-size", "3",
            "--fail-fast",
            "--auto-rollback",
            "--action", "boot",
        ]);
        match cli.command {
            Commands::Switch {
                dry_run,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                action,
                on_error,
                target,
                flake,
                tag,
                role,
                user,
                port,
                identity_file,
            } => {
                assert!(dry_run);
                assert_eq!(concurrency, 2);
                assert_eq!(strategy, "canary");
                assert_eq!(batch_size, 3);
                assert!(fail_fast);
                assert!(auto_rollback);
                assert_eq!(on_error, None);
                assert_eq!(action, "boot");
                let _ = (target, flake, tag, role, user, port, identity_file);
            }
            _ => panic!("expected a switch command"),
        }
    }

    #[test]
    fn parses_on_error_flag() {
        let cli = Cli::parse_from(vec!["nod", "switch", "--on-error", "continue"]);
        match cli.command {
            Commands::Switch {
                on_error,
                target,
                flake,
                tag,
                role,
                user,
                port,
                identity_file,
                dry_run,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                action,
            } => {
                assert_eq!(on_error, Some("continue".to_string()));
                let _ = (
                    target, flake, tag, role, user, port, identity_file, dry_run, concurrency,
                    strategy, batch_size, fail_fast, auto_rollback, action,
                );
            }
            _ => panic!("expected a switch command"),
        }
    }

    #[test]
    fn parses_plan_and_rollback_commands() {
        // `Plan.target` is a positional argument (defaults to "local"), so it
        // must be given positionally, not via `--target`.
        let plan = Cli::try_parse_from(vec!["nod", "plan", "atlas"])
            .expect("valid plan invocation should parse");
        match plan.command {
            Commands::Plan { target, .. } => {
                assert_eq!(target, "atlas");
            }
            _ => panic!("expected a plan command"),
        }

        let roll = Cli::try_parse_from(vec!["nod", "rollback", "atlas"])
            .expect("valid rollback invocation should parse");
        match roll.command {
            Commands::Rollback { target, .. } => {
                assert_eq!(target, "atlas");
            }
            _ => panic!("expected a rollback command"),
        }
    }

    #[test]
    fn parses_drift_and_history_commands() {
        let drift = Cli::try_parse_from(vec![
            "nod", "drift", "--target", "atlas", "--tag", "server", "--json",
        ])
            .expect("valid drift invocation should parse");
        match drift.command {
            Commands::Drift { target, tag, json } => {
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, Some("server".to_string()));
                assert!(json);
            }
            _ => panic!("expected a drift command"),
        }

        let history = Cli::try_parse_from(vec!["nod", "history", "--limit", "5"])
            .expect("valid history invocation should parse");
        match history.command {
            Commands::History { target, limit, json } => {
                assert_eq!(target, None);
                assert_eq!(limit, Some(5));
                assert!(!json);
            }
            _ => panic!("expected a history command"),
        }
    }
}