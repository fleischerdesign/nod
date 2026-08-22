use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "nod",
    version = "2.0.0",
    about = "Universal Nix Orchestration & Deployment Engine",
    long_about = "Universal, zero-config, high-performance Nix Flake deployment, orchestration, and monitoring engine."
)]
pub struct Cli {
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

        /// Skip pre-flight quality checks (nixfmt, statix, deadnix)
        #[arg(long)]
        no_check: bool,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Run strict repository quality gates (nixfmt + deadnix + statix + lix check)
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
    },

    /// Generate package & systemd unit diff preview before switching
    Diff {
        /// Target host name
        #[arg(default_value = "local")]
        target: String,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Roll back a remote or local host to its previous NixOS profile generation
    Rollback {
        /// Target host name
        target: String,
    },

    /// Launch interactive Ratatui TUI dashboard for fleet management
    Dashboard {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },
}
