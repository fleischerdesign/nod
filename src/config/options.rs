use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Common target-selection flags shared by the commands that scope a fleet:
/// a positional target plus `--tag`/`--role`/`--all` filters (DRY — these
/// were previously re-declared per command). Only the long flag forms live
/// here; `ssh`/`exec` keep their own short forms (`-t`/`-r`/`-a`) and inline
/// declarations to preserve their exact CLI surface.
#[derive(clap::Args, Debug, Default)]
pub struct TargetArgs {
    /// Target host: 'local', a host name or glob (e.g. 'web-*'), or 'all'
    pub target: Option<String>,

    /// Filter the fleet to hosts carrying this tag
    #[arg(long, value_name = "TAG")]
    pub tag: Option<String>,

    /// Filter the fleet to hosts with this role (e.g. 'server')
    #[arg(long, value_name = "ROLE")]
    pub role: Option<String>,

    /// Target every host in the discovered fleet
    #[arg(long)]
    pub all: bool,
}

/// Common SSH connection overrides shared by the deploy/preview/diff
/// commands (DRY — previously re-declared per command). Rollback deliberately
/// keeps its own `--user`/`--port` (it has no `--identity-file`), so only
/// commands that genuinely accept all three flatten this in.
#[derive(clap::Args, Debug, Default)]
pub struct SshArgs {
    /// Override the SSH user for every targeted host
    #[arg(long, value_name = "USER")]
    pub user: Option<String>,

    /// Override the SSH port for every targeted host
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,

    /// Override the SSH identity file for every targeted host
    #[arg(long, value_name = "PATH")]
    pub identity_file: Option<PathBuf>,
}

impl From<SshArgs> for crate::domain::config::CliOverrides {
    fn from(args: SshArgs) -> Self {
        Self {
            user: args.user,
            port: args.port,
            identity_file: args.identity_file,
        }
    }
}

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
        #[command(flatten)]
        target_args: TargetArgs,

        #[command(flatten)]
        ssh_args: SshArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

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

    /// Run `switch-to-configuration test` on one host or a fleet
    Test {
        #[command(flatten)]
        target_args: TargetArgs,

        #[command(flatten)]
        ssh_args: SshArgs,

        /// Custom path to flake root directory
        #[arg(long)]
        flake: Option<PathBuf>,

        /// Maximum hosts processed in-flight at once
        #[arg(long, value_name = "N")]
        concurrency: Option<usize>,

        /// Rollout strategy: 'all', 'canary' or 'batch'
        #[arg(long, value_name = "STRATEGY")]
        strategy: Option<String>,

        /// Wave size for --strategy batch
        #[arg(long, value_name = "N")]
        batch_size: Option<usize>,

        /// Abort the whole run on the first host failure
        #[arg(long)]
        fail_fast: bool,

        /// Attempt a rollback before failing a host
        #[arg(long)]
        auto_rollback: bool,
    },

    /// Run `switch-to-configuration boot` on one host or a fleet
    Boot {
        #[command(flatten)]
        target_args: TargetArgs,

        #[command(flatten)]
        ssh_args: SshArgs,

        /// Custom path to flake root directory
        #[arg(long)]
        flake: Option<PathBuf>,

        /// Maximum hosts processed in-flight at once
        #[arg(long, value_name = "N")]
        concurrency: Option<usize>,

        /// Rollout strategy: 'all', 'canary' or 'batch'
        #[arg(long, value_name = "STRATEGY")]
        strategy: Option<String>,

        /// Wave size for --strategy batch
        #[arg(long, value_name = "N")]
        batch_size: Option<usize>,

        /// Abort the whole run on the first host failure
        #[arg(long)]
        fail_fast: bool,

        /// Attempt a rollback before failing a host
        #[arg(long)]
        auto_rollback: bool,
    },

    /// Build a host or fleet's toplevel closure without transferring it
    Build {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long)]
        flake: Option<PathBuf>,

        /// Build through a configured fleet host (exactly one; flake `buildHost`
        /// default is the lower tier)
        #[arg(long, value_name = "HOST")]
        builder: Option<String>,

        /// Symlink the built closure to this path
        #[arg(long, value_name = "PATH")]
        out_link: Option<PathBuf>,

        /// Maximum hosts built in-flight at once
        #[arg(long, value_name = "N")]
        concurrency: Option<usize>,
    },

    /// Run strict repository quality gates (nixfmt + deadnix + statix)
    Check {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Display live status matrix of all discovered NixOS hosts
    Status {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Generate package & systemd unit diff preview before switching
    Diff {
        #[command(flatten)]
        target_args: TargetArgs,

        #[command(flatten)]
        ssh_args: SshArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Preview the deployment plan for a target without activating it
    Plan {
        #[command(flatten)]
        target_args: TargetArgs,

        #[command(flatten)]
        ssh_args: SshArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Revert exactly one host to its previous NixOS profile generation
    ///
    /// Rollback is single-host: matching more than one host is rejected
    /// rather than silently operating on a subset of the fleet.
    Rollback {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Override the SSH user for the target
        #[arg(long, value_name = "USER")]
        user: Option<String>,

        /// Override the SSH port for the target
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
    },

    /// Detect configuration drift between live closures and the flake
    Drift {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Emit the drift report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Read the recorded deployment audit history
    Audit {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Narrow the audit to one host
        target: Option<String>,

        /// Cap the newest entries returned
        #[arg(long, value_name = "N")]
        limit: Option<usize>,

        /// Emit the audit as JSON
        #[arg(long)]
        json: bool,
    },

    /// Open an interactive SSH session or execute a remote command on a host
    Ssh {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Target host name or selector
        target: Option<String>,

        /// Filter by tag
        #[arg(short, long)]
        tag: Option<String>,

        /// Filter by role
        #[arg(short, long)]
        role: Option<String>,

        /// Run remote command with sudo or open root shell
        #[arg(long)]
        sudo: bool,

        /// Trailing command and arguments to execute remotely
        #[arg(last = true)]
        command: Vec<String>,
    },

    /// Execute an arbitrary shell command across resolved target hosts
    Exec {
        /// Target host name or glob pattern
        target: Option<String>,

        /// Path to flake directory
        #[arg(short, long)]
        flake: Option<PathBuf>,

        /// Filter by tag
        #[arg(short, long)]
        tag: Option<String>,

        /// Filter by role
        #[arg(short, long)]
        role: Option<String>,

        /// Execute across all discovered hosts
        #[arg(short, long)]
        all: bool,

        /// Run command with sudo on remote host
        #[arg(long)]
        sudo: bool,

        /// Maximum concurrent remote executions
        #[arg(short, long)]
        concurrency: Option<usize>,

        /// Abort remaining executions on first failure
        #[arg(long)]
        fail_fast: bool,

        /// Emit structured JSON output
        #[arg(long)]
        json: bool,

        /// Shell command and trailing arguments to execute
        #[arg(last = true)]
        command: Vec<String>,
    },

    /// Launch interactive Ratatui TUI dashboard for fleet management
    Dashboard {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Inspect declared and locked flake inputs
    Inputs {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Emit input list as JSON
        #[arg(long)]
        json: bool,
    },

    /// Inspect flake repository and lockfile metadata
    Metadata {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Emit metadata as JSON
        #[arg(long)]
        json: bool,
    },

    /// Update flake inputs and display revision deltas
    Update {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Specific input names to update (e.g. 'nixpkgs', 'nod')
        inputs: Vec<String>,

        /// Emit update report as JSON
        #[arg(long)]
        json: bool,
    },

    /// List installed NixOS profile generations across the fleet
    Generations {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Emit generations list as JSON
        #[arg(long)]
        json: bool,
    },

    /// Collect garbage and remove old generations across target hosts
    Gc {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Delete generations older than specified duration (e.g. '14d', '30d')
        #[arg(long, value_name = "AGE")]
        older_than: Option<String>,

        /// Retain at least N recent generations
        #[arg(long, value_name = "N")]
        keep: Option<usize>,

        /// Preview reclaimable space without deleting closures
        #[arg(long)]
        dry_run: bool,

        /// Maximum concurrent garbage collection runs
        #[arg(long, value_name = "N", default_value = "4")]
        concurrency: usize,

        /// Emit GC report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Pre-stage store closures on target hosts without activation
    Copy {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Destination store URI (e.g. 'ssh://user@host')
        #[arg(long, value_name = "URI")]
        to: Option<String>,

        /// Source store URI to copy closures from
        #[arg(long, value_name = "URI")]
        from: Option<String>,

        /// Emit copy report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Evaluate a Nix expression in the host configuration context
    Eval {
        /// Nix attribute expression to evaluate (e.g. 'services.nginx.enable')
        expr: String,

        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Output raw unquoted string representation
        #[arg(long)]
        raw: bool,

        /// Emit evaluated value as JSON
        #[arg(long)]
        json: bool,
    },

    /// Launch interactive Nix REPL with host configuration pre-loaded
    Repl {
        /// Target host name (defaults to local host if omitted)
        target: Option<String>,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Inspect comprehensive configuration and live host diagnostics
    Info {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Emit host diagnostics as JSON
        #[arg(long)]
        json: bool,
    },

    /// Reboot target hosts with rollout orchestration and recovery verification
    Reboot {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Rollout strategy: 'all', 'canary', or 'batch'
        #[arg(long, value_name = "STRATEGY", default_value = "batch")]
        strategy: String,

        /// Wave size for --strategy batch
        #[arg(long, value_name = "N", default_value = "1")]
        batch_size: usize,

        /// Maximum concurrent reboots per wave
        #[arg(long, value_name = "N", default_value = "4")]
        concurrency: usize,

        /// Do not wait for hosts to cycle and recover online
        #[arg(long)]
        no_wait: bool,

        /// Maximum seconds to wait for host recovery
        #[arg(long, value_name = "SECS", default_value = "180")]
        timeout: u64,
    },

    /// Manage host secrets (verification and rekeying)
    Secret {
        #[command(subcommand)]
        command: SecretCommands,
    },

    /// Manage Nix store operations (optimization and deduplication)
    Store {
        #[command(subcommand)]
        command: StoreCommands,
    },

    /// Manage binary cache operations (pushing closures)
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },

    /// Render fleet topology as a diagram (Mermaid, DOT/Graphviz, or JSON)
    Graph {
        /// Diagram format: 'mermaid', 'dot', or 'json'
        #[arg(long, default_value = "mermaid")]
        format: String,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Export fleet inventory to external tooling (Ansible, Prometheus, JSON)
    Export {
        /// Target export format: 'ansible', 'prometheus', or 'json'
        #[arg(value_name = "FORMAT")]
        format: String,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,
    },

    /// Bootstrap a bare-metal machine from live ISO using nixos-anywhere and disko
    Bootstrap {
        /// Target host defined in flake
        target: String,

        /// IP address or hostname of machine in live installer environment
        #[arg(long)]
        ip: String,

        /// SSH username for live target (default: root)
        #[arg(long, default_value = "root")]
        user: String,

        /// SSH port on live target
        #[arg(long, default_value = "22")]
        port: u16,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Bypass disko partitioning phase
        #[arg(long)]
        no_disko: bool,

        /// Skip kexec and assume machine is already in an installer kernel
        #[arg(long)]
        no_kexec: bool,

        /// Enable verbose debug output from nixos-anywhere
        #[arg(long)]
        debug: bool,

        /// Emit report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Scaffold a new Nix flake repository with nod configuration templates
    Init {
        /// Destination directory (default: current directory)
        dir: Option<String>,

        /// Template style: 'minimal', 'fleet', or 'server'
        #[arg(long, default_value = "minimal")]
        template: String,

        /// Descriptive name for the fleet
        #[arg(long)]
        name: Option<String>,
    },

    /// Build a bootable installer ISO or disk image for a host
    Iso {
        /// Target host configuration (default: first discovered host)
        target: Option<String>,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Output format (e.g. iso, raw-efi, qcow2)
        #[arg(long, default_value = "iso")]
        format: String,

        /// Emit image path as JSON
        #[arg(long)]
        json: bool,
    },

    /// Watch flake repository and trigger live preview rebuild and diff on file save
    Watch {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Polling interval in seconds
        #[arg(long, default_value = "2")]
        interval: u64,
    },

    /// Pull-based GitOps synchronization from upstream Git repository
    Sync {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Upstream Git remote name
        #[arg(long, default_value = "origin")]
        remote: String,

        /// Target Git branch
        #[arg(long, default_value = "main")]
        branch: String,

        /// Run a single reconciliation cycle and exit
        #[arg(long)]
        once: bool,

        /// Polling interval in seconds when running continuously
        #[arg(long, default_value = "300")]
        interval: u64,
    },

    /// Run as a background systemd GitOps reconciler daemon
    Daemon {
        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Reconciliation interval in seconds
        #[arg(long, default_value = "300")]
        interval: u64,
    },
}

#[derive(Subcommand, Debug)]
pub enum StoreCommands {
    /// Deduplicate identical files in the Nix store via hardlinks
    Optimize {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Emit optimization report as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Push built system closures to a remote binary cache
    Push {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Destination binary cache URI (e.g. s3://cache, ssh://cache, https://cache.example.com)
        #[arg(long)]
        cache: Option<String>,

        /// Maximum concurrent uploads
        #[arg(long, default_value = "4")]
        concurrency: usize,

        /// Preview cache push operations without uploading
        #[arg(long)]
        dry_run: bool,

        /// Emit cache push report as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SecretCommands {
    /// Verify that declared secrets for target hosts are valid and decryptable
    Check {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Emit verification report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Re-encrypt secrets across target hosts with updated recipient keys
    Rekey {
        #[command(flatten)]
        target_args: TargetArgs,

        /// Custom path to flake root directory
        #[arg(long, default_value = ".")]
        flake: String,

        /// Preview rekey actions without modifying files
        #[arg(long)]
        dry_run: bool,

        /// Bypass creation of .bak backup files
        #[arg(long)]
        no_backup: bool,

        /// Emit rekey report as JSON
        #[arg(long)]
        json: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_switch_target_tag_role_and_all() {
        let cli = Cli::try_parse_from(vec![
            "nod",
            "switch",
            "atlas",
            "--flake",
            "/tmp/flake",
            "--tag",
            "server",
            "--role",
            "desktop",
            "--all",
        ])
        .expect("valid switch invocation should parse");
        match cli.command {
            Commands::Switch {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
                dry_run,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                on_error,
                action,
            } => {
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(flake, "/tmp/flake");
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("desktop".to_string()));
                assert!(all);
                assert_eq!(user, None);
                assert_eq!(port, None);
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
    fn parses_switch_with_no_target_declines_all() {
        let cli = Cli::parse_from(vec!["nod", "switch"]);
        match cli.command {
            Commands::Switch {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
                dry_run,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                on_error,
                action,
            } => {
                assert_eq!(target, None);
                assert_eq!(tag, None);
                assert_eq!(role, None);
                assert!(!all);
                let _ = (
                    flake,
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
                );
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
            "--concurrency",
            "2",
            "--strategy",
            "canary",
            "--batch-size",
            "3",
            "--fail-fast",
            "--auto-rollback",
            "--action",
            "boot",
        ]);
        match cli.command {
            Commands::Switch {
                target_args:
                    TargetArgs {
                        all,
                        target,
                        tag,
                        role,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
                dry_run,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                on_error,
                action,
            } => {
                assert!(dry_run);
                assert_eq!(concurrency, 2);
                assert_eq!(strategy, "canary");
                assert_eq!(batch_size, 3);
                assert!(fail_fast);
                assert!(auto_rollback);
                assert_eq!(on_error, None);
                assert_eq!(action, "boot");
                assert!(!all);
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
                target_args:
                    TargetArgs {
                        all,
                        target,
                        tag,
                        role,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
                dry_run,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
                on_error,
                action,
            } => {
                assert_eq!(on_error, Some("continue".to_string()));
                assert!(!all);
                let _ = (
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
                );
            }
            _ => panic!("expected a switch command"),
        }
    }

    #[test]
    fn parses_plan_target_tag_role_and_all() {
        let plan = Cli::try_parse_from(vec![
            "nod", "plan", "atlas", "--tag", "server", "--role", "desktop", "--all",
        ])
        .expect("valid plan invocation should parse");
        match plan.command {
            Commands::Plan {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
            } => {
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("desktop".to_string()));
                assert!(all);
                let _ = (flake, user, port, identity_file);
            }
            _ => panic!("expected a plan command"),
        }
    }

    #[test]
    fn parses_diff_target_tag_role_and_all() {
        let diff = Cli::try_parse_from(vec![
            "nod", "diff", "web-*", "--tag", "prod", "--role", "server", "--all",
        ])
        .expect("valid diff invocation should parse");
        match diff.command {
            Commands::Diff {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
            } => {
                assert_eq!(target, Some("web-*".to_string()));
                assert_eq!(tag, Some("prod".to_string()));
                assert_eq!(role, Some("server".to_string()));
                assert!(all);
                let _ = (flake, user, port, identity_file);
            }
            _ => panic!("expected a diff command"),
        }
    }

    #[test]
    fn parses_status_target_tag_role_and_all() {
        let status = Cli::try_parse_from(vec![
            "nod", "status", "atlas", "--tag", "server", "--role", "desktop", "--all",
        ])
        .expect("valid status invocation should parse");
        match status.command {
            Commands::Status {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                flake,
            } => {
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("desktop".to_string()));
                assert!(all);
                let _ = flake;
            }
            _ => panic!("expected a status command"),
        }

        let bare = Cli::try_parse_from(vec!["nod", "status"]).expect("bare status should parse");
        match bare.command {
            Commands::Status {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                flake,
            } => {
                assert_eq!(target, None);
                assert_eq!(tag, None);
                assert_eq!(role, None);
                assert!(!all);
                let _ = flake;
            }
            _ => panic!("expected a status command"),
        }
    }

    #[test]
    fn parses_rollback_target_tag_role_and_all() {
        let roll = Cli::try_parse_from(vec![
            "nod", "rollback", "atlas", "--tag", "server", "--role", "db", "--all",
        ])
        .expect("valid rollback invocation should parse");
        match roll.command {
            Commands::Rollback {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                flake,
                user,
                port,
            } => {
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("db".to_string()));
                assert!(all);
                let _ = (flake, user, port);
            }
            _ => panic!("expected a rollback command"),
        }
    }

    #[test]
    fn parses_audit_limit_and_json() {
        let audit = Cli::try_parse_from(vec!["nod", "audit", "atlas", "--limit", "5", "--json"])
            .expect("valid audit invocation should parse");
        match audit.command {
            Commands::Audit {
                flake,
                target,
                limit,
                json,
            } => {
                assert_eq!(flake, ".");
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(limit, Some(5));
                assert!(json);
            }
            _ => panic!("expected an audit command"),
        }
    }

    #[test]
    fn parses_audit_with_flake() {
        let audit = Cli::try_parse_from(vec![
            "nod",
            "audit",
            "--flake",
            "/srv/nixos",
            "--limit",
            "3",
        ])
        .expect("valid audit invocation with the flake flag should parse");
        match audit.command {
            Commands::Audit {
                flake,
                target,
                limit,
                json,
            } => {
                assert_eq!(flake, "/srv/nixos");
                assert_eq!(target, None);
                assert_eq!(limit, Some(3));
                assert!(!json);
            }
            _ => panic!("expected an audit command"),
        }
    }

    #[test]
    fn parses_drift_target_tag_role_all_and_json() {
        let drift = Cli::try_parse_from(vec![
            "nod", "drift", "atlas", "--tag", "server", "--role", "db", "--all", "--json",
        ])
        .expect("valid drift invocation should parse");
        match drift.command {
            Commands::Drift {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                flake,
                json,
            } => {
                assert_eq!(flake, ".");
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("db".to_string()));
                assert!(all);
                assert!(json);
            }
            _ => panic!("expected a drift command"),
        }
    }

    #[test]
    fn parses_drift_with_flake() {
        let drift = Cli::try_parse_from(vec![
            "nod",
            "drift",
            "atlas",
            "--flake",
            "/etc/nixos",
            "--all",
            "--json",
        ])
        .expect("valid drift invocation with the flake flag should parse");
        match drift.command {
            Commands::Drift {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                flake,
                json,
            } => {
                assert_eq!(flake, "/etc/nixos");
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, None);
                assert_eq!(role, None);
                assert!(all);
                assert!(json);
            }
            _ => panic!("expected a drift command"),
        }
    }

    #[test]
    fn parses_ssh_bare_with_defaults() {
        let ssh = Cli::parse_from(vec!["nod", "ssh"]);
        match ssh.command {
            Commands::Ssh {
                flake,
                target,
                tag,
                role,
                sudo,
                command,
            } => {
                assert_eq!(flake, ".");
                assert_eq!(target, None);
                assert_eq!(tag, None);
                assert_eq!(role, None);
                assert!(!sudo);
                assert!(command.is_empty());
            }
            _ => panic!("expected an ssh command"),
        }
    }

    #[test]
    fn parses_ssh_target_sudo_and_filters() {
        let ssh = Cli::try_parse_from(vec![
            "nod", "ssh", "atlas", "--sudo", "--tag", "prod", "--role", "api",
        ])
        .expect("valid ssh invocation should parse");
        match ssh.command {
            Commands::Ssh {
                flake,
                target,
                tag,
                role,
                sudo,
                command,
            } => {
                assert_eq!(flake, ".");
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, Some("prod".to_string()));
                assert_eq!(role, Some("api".to_string()));
                assert!(sudo);
                assert!(command.is_empty());
            }
            _ => panic!("expected an ssh command"),
        }
    }

    #[test]
    fn parses_ssh_trailing_command_after_separator() {
        let ssh = Cli::try_parse_from(vec![
            "nod", "ssh", "atlas", "--", "uname", "-a", "-o", "flag",
        ])
        .expect("valid ssh command should parse");
        match ssh.command {
            Commands::Ssh {
                flake,
                target,
                tag,
                role,
                sudo,
                command,
            } => {
                assert_eq!(flake, ".");
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(command, ["uname", "-a", "-o", "flag"]);
                assert!(!sudo);
                let _ = (tag, role);
            }
            _ => panic!("expected an ssh command"),
        }
    }

    #[test]
    fn parses_ssh_short_tag_and_role_flags() {
        let ssh = Cli::try_parse_from(vec![
            "nod", "ssh", "atlas", "-t", "prod", "-r", "db", "--sudo", "--", "uname", "-a",
        ])
        .expect("valid ssh short flags should parse");
        match ssh.command {
            Commands::Ssh {
                flake,
                target,
                tag,
                role,
                sudo,
                command,
            } => {
                assert_eq!(flake, ".");
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, Some("prod".to_string()));
                assert_eq!(role, Some("db".to_string()));
                assert!(sudo);
                assert_eq!(command, ["uname", "-a"]);
            }
            _ => panic!("expected an ssh command"),
        }
    }

    #[test]
    fn parses_ssh_with_flake() {
        let ssh = Cli::try_parse_from(vec!["nod", "ssh", "--flake", "/srv/nixos", "atlas"])
            .expect("valid ssh invocation with the flake flag should parse");
        match ssh.command {
            Commands::Ssh {
                flake,
                target,
                tag,
                role,
                sudo,
                command,
            } => {
                assert_eq!(flake, "/srv/nixos");
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(tag, None);
                assert_eq!(role, None);
                assert!(!sudo);
                assert!(command.is_empty());
            }
            _ => panic!("expected an ssh command"),
        }
    }

    #[test]
    fn parses_test_target_tag_role_and_all() {
        let cli = Cli::try_parse_from(vec![
            "nod",
            "test",
            "atlas",
            "--flake",
            "/tmp/flake",
            "--tag",
            "server",
            "--role",
            "desktop",
            "--all",
            "--user",
            "root",
            "--port",
            "2222",
            "--identity-file",
            "/tmp/key",
            "--concurrency",
            "2",
            "--strategy",
            "canary",
            "--batch-size",
            "3",
            "--fail-fast",
            "--auto-rollback",
        ])
        .expect("valid test invocation should parse");
        match cli.command {
            Commands::Test {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
            } => {
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(flake, Some(PathBuf::from("/tmp/flake")));
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("desktop".to_string()));
                assert!(all);
                assert_eq!(user, Some("root".to_string()));
                assert_eq!(port, Some(2222));
                assert_eq!(identity_file, Some(PathBuf::from("/tmp/key")));
                assert_eq!(concurrency, Some(2));
                assert_eq!(strategy, Some("canary".to_string()));
                assert_eq!(batch_size, Some(3));
                assert!(fail_fast);
                assert!(auto_rollback);
            }
            _ => panic!("expected a test command"),
        }
    }

    #[test]
    fn parses_bare_test_with_defaults() {
        let cli = Cli::parse_from(vec!["nod", "test"]);
        match cli.command {
            Commands::Test {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
            } => {
                assert_eq!(target, None);
                assert_eq!(flake, None);
                assert_eq!(tag, None);
                assert_eq!(role, None);
                assert!(!all);
                assert_eq!(user, None);
                assert_eq!(port, None);
                assert_eq!(identity_file, None);
                assert_eq!(concurrency, None);
                assert_eq!(strategy, None);
                assert_eq!(batch_size, None);
                assert!(!fail_fast);
                assert!(!auto_rollback);
            }
            _ => panic!("expected a test command"),
        }
    }

    #[test]
    fn parses_boot_target_tag_role_and_all() {
        let cli = Cli::try_parse_from(vec![
            "nod",
            "boot",
            "atlas",
            "--flake",
            "/tmp/flake",
            "--tag",
            "server",
            "--role",
            "desktop",
            "--all",
        ])
        .expect("valid boot invocation should parse");
        match cli.command {
            Commands::Boot {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                ssh_args:
                    SshArgs {
                        user,
                        port,
                        identity_file,
                    },
                flake,
                concurrency,
                strategy,
                batch_size,
                fail_fast,
                auto_rollback,
            } => {
                assert_eq!(target, Some("atlas".to_string()));
                assert_eq!(flake, Some(PathBuf::from("/tmp/flake")));
                assert_eq!(tag, Some("server".to_string()));
                assert_eq!(role, Some("desktop".to_string()));
                assert!(all);
                assert_eq!(user, None);
                assert_eq!(port, None);
                assert_eq!(identity_file, None);
                assert_eq!(concurrency, None);
                assert_eq!(strategy, None);
                assert_eq!(batch_size, None);
                assert!(!fail_fast);
                assert!(!auto_rollback);
            }
            _ => panic!("expected a boot command"),
        }
    }

    #[test]
    fn parses_build_target_out_link_and_concurrency() {
        let cli = Cli::try_parse_from(vec![
            "nod",
            "build",
            "web-*",
            "--tag",
            "prod",
            "--role",
            "server",
            "--all",
            "--out-link",
            "/tmp/result",
            "--concurrency",
            "2",
        ])
        .expect("valid build invocation should parse");
        match cli.command {
            Commands::Build {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                flake,
                builder,
                out_link,
                concurrency,
            } => {
                assert_eq!(target, Some("web-*".to_string()));
                assert_eq!(flake, None);
                assert_eq!(tag, Some("prod".to_string()));
                assert_eq!(role, Some("server".to_string()));
                assert!(all);
                assert_eq!(builder, None);
                assert_eq!(out_link, Some(PathBuf::from("/tmp/result")));
                assert_eq!(concurrency, Some(2));
            }
            _ => panic!("expected a build command"),
        }
    }

    #[test]
    fn parses_build_with_builder() {
        let cli = Cli::parse_from(vec![
            "nod",
            "build",
            "web-01",
            "--builder",
            "buildy",
            "--out-link",
            "/tmp/result",
        ]);
        match cli.command {
            Commands::Build {
                target_args:
                    TargetArgs {
                        target,
                        tag,
                        role,
                        all,
                    },
                flake,
                builder,
                out_link,
                concurrency,
            } => {
                assert_eq!(target, Some("web-01".to_string()));
                assert_eq!(builder, Some("buildy".to_string()));
                assert_eq!(out_link, Some(PathBuf::from("/tmp/result")));
                let _ = (flake, tag, role, all, concurrency);
            }
            _ => panic!("expected a build command"),
        }
    }

    #[test]
    fn parses_exec_target_tag_role_all_filters_and_command() {
        let cli = Cli::try_parse_from(vec![
            "nod",
            "exec",
            "web-*",
            "--flake",
            "/tmp/flake",
            "--tag",
            "prod",
            "--role",
            "server",
            "--all",
            "--sudo",
            "--concurrency",
            "2",
            "--fail-fast",
            "--json",
            "--",
            "uptime",
            "-p",
        ])
        .expect("valid exec invocation should parse");
        match cli.command {
            Commands::Exec {
                target,
                flake,
                tag,
                role,
                all,
                sudo,
                concurrency,
                fail_fast,
                json,
                command,
            } => {
                assert_eq!(target, Some("web-*".to_string()));
                assert_eq!(flake, Some(PathBuf::from("/tmp/flake")));
                assert_eq!(tag, Some("prod".to_string()));
                assert_eq!(role, Some("server".to_string()));
                assert!(all);
                assert!(sudo);
                assert_eq!(concurrency, Some(2));
                assert!(fail_fast);
                assert!(json);
                assert_eq!(command, ["uptime", "-p"]);
            }
            _ => panic!("expected an exec command"),
        }
    }

    #[test]
    fn parses_bare_exec_with_defaults() {
        let cli = Cli::parse_from(vec!["nod", "exec", "--", "echo", "hi"]);
        match cli.command {
            Commands::Exec {
                target,
                flake,
                tag,
                role,
                all,
                sudo,
                concurrency,
                fail_fast,
                json,
                command,
            } => {
                assert_eq!(target, None);
                assert_eq!(flake, None);
                assert_eq!(tag, None);
                assert_eq!(role, None);
                assert!(!all);
                assert!(!sudo);
                assert_eq!(concurrency, None);
                assert!(!fail_fast);
                assert!(!json);
                assert_eq!(command, ["echo", "hi"]);
            }
            _ => panic!("expected an exec command"),
        }
    }

    #[test]
    fn parses_exec_short_flags_and_globbing_target() {
        let cli = Cli::try_parse_from(vec![
            "nod",
            "exec",
            "db-*",
            "-t",
            "prod",
            "-r",
            "db",
            "-a",
            "-f",
            "/tmp/flake",
            "-c",
            "3",
            "--",
            "systemctl",
            "status",
            "postgres",
        ])
        .expect("valid exec short flags should parse");
        match cli.command {
            Commands::Exec {
                target,
                flake,
                tag,
                role,
                all,
                sudo,
                concurrency,
                fail_fast,
                json,
                command,
            } => {
                assert_eq!(target, Some("db-*".to_string()));
                assert_eq!(flake, Some(PathBuf::from("/tmp/flake")));
                assert_eq!(tag, Some("prod".to_string()));
                assert_eq!(role, Some("db".to_string()));
                assert!(all);
                assert!(!sudo);
                assert_eq!(concurrency, Some(3));
                assert!(!fail_fast);
                assert!(!json);
                assert_eq!(command, ["systemctl", "status", "postgres"]);
            }
            _ => panic!("expected an exec command"),
        }
    }

    #[test]
    fn ssh_args_converts_into_cli_overrides() {
        use crate::domain::config::CliOverrides;
        let args = SshArgs {
            user: Some("philipp".to_string()),
            port: Some(2222),
            identity_file: Some(PathBuf::from("/home/philipp/.ssh/id_ed25519")),
        };
        let overrides: CliOverrides = args.into();
        assert_eq!(overrides.user, Some("philipp".to_string()));
        assert_eq!(overrides.port, Some(2222));
        assert_eq!(
            overrides.identity_file,
            Some(PathBuf::from("/home/philipp/.ssh/id_ed25519"))
        );
    }
}
