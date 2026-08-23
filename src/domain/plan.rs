//! Domain deployment-plan value objects (ADR-003, ADR-005).
//!
//! `DeploymentPlan`, `TargetPlan`, `DeploymentAction`, `TargetDiff` plus the
//! pure rollout wave-partition (`DeploymentPlan::waves`) are dependency-free;
//! engineers extend them without dragging in Application/Infrastructure.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What an individual target plan asks the pipeline to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeploymentAction {
    /// Rebuild and activate the NixOS configuration.
    Switch,
    /// Boot/toggle a host subsystem (forward-looking hook).
    Boot,
    /// Run post-activation verification only.
    Test,
    /// Build & inspect the new closure without activating it.
    DryRun,
}

impl DeploymentAction {
    /// Returns the canonical string form.
    pub fn to_str(&self) -> String {
        match self {
            DeploymentAction::Switch => String::from("switch"),
            DeploymentAction::Boot => String::from("boot"),
            DeploymentAction::Test => String::from("test"),
            DeploymentAction::DryRun => String::from("dry-run"),
        }
    }

    /// Parses a CLI `--action <...>` value, or `None` when unrecognised.
    pub fn parse(s: &str) -> Option<DeploymentAction> {
        match s.to_lowercase().as_str() {
            "switch" => Some(DeploymentAction::Switch),
            "boot" => Some(DeploymentAction::Boot),
            "test" => Some(DeploymentAction::Test),
            "dry-run" => Some(DeploymentAction::DryRun),
            _ => None,
        }
    }
}

/// Fleet rollout strategy (ADR-005): the ordering spine for concurrency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolloutStrategy {
    /// Every host at once, bounded by the concurrency budget.
    All,
    /// Warm one host first, observe it, then run the remainder.
    Canary,
    /// Fixed-size waves (`batch_size` from the options).
    Batch,
}

impl RolloutStrategy {
    /// Parses a CLI `--strategy <all|canary|batch>` value, or `None`.
    pub fn parse(s: &str) -> Option<RolloutStrategy> {
        match s.to_lowercase().as_str() {
            "all" => Some(RolloutStrategy::All),
            "canary" => Some(RolloutStrategy::Canary),
            "batch" => Some(RolloutStrategy::Batch),
            _ => None,
        }
    }
}

/// Per-run deployment policy: concurrency, strategy, recovery, dry-run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentOptions {
    /// Maximum in-flight hosts (semaphore permits).
    pub concurrency: usize,
    /// Rollout strategy (`All`, `Canary`, `Batch`).
    pub strategy: RolloutStrategy,
    /// Wave size for `Batch`; ignored by `All`/`Canary` when 0.
    pub batch_size: usize,
    /// Abort the whole run on the first failure.
    pub fail_fast: bool,
    /// Attempt a rollback before failing a host.
    pub auto_rollback: bool,
    /// Preview only: never reach the switch command.
    pub dry_run: bool,
    /// Per-target action for `--action`.
    pub action: DeploymentAction,
    /// Emit per-step detail.
    pub verbose: bool,
}

impl DeploymentOptions {
    /// Conservative defaults matching the ADR-005 "batch by default" stance.
    pub fn default_policy() -> Self {
        Self {
            concurrency: 4,
            strategy: RolloutStrategy::Batch,
            batch_size: 0,
            fail_fast: true,
            auto_rollback: true,
            dry_run: false,
            action: DeploymentAction::Switch,
            verbose: false,
        }
    }

    /// Returns `true` when no host should be activated.
    pub fn is_dry_run(&self) -> bool {
        self.dry_run || self.action == DeploymentAction::DryRun
    }
}

/// A single host's measured/current closure pair (built without activating).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetDiff {
    /// Host the diff belongs to.
    pub host_name: String,
    /// The currently-live sys closure, when known.
    pub current_closure: Option<PathBuf>,
    /// The freshly built / staged closure.
    pub new_closure: PathBuf,
    /// True when the two closures differ (or current is unknown).
    pub changed: bool,
}

/// One staged target within a `DeploymentPlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetPlan {
    /// Host name (matches a `HostEntity.name`).
    pub host_name: String,
    /// The action this target will take.
    pub action: DeploymentAction,
    /// The freshly-built toplevel closure, when built.
    pub new_closure: Option<PathBuf>,
    /// The live/current closure, when discoverable.
    pub current_closure: Option<PathBuf>,
}

/// A full, ordered deployment preview (ADR-003 planning stage).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentPlan {
    /// Staged targets, in precedence order.
    pub targets: Vec<TargetPlan>,
    /// The policy shape the plan was constructed with.
    pub options: DeploymentOptions,
}

impl DeploymentPlan {
    /// Returns the number of staged targets.
    pub fn size(&self) -> usize {
        self.targets.len()
    }

    /// Partitions target indices into rollout waves for the active strategy.
    ///
    /// - `All`: a single wave containing every index.
    /// - `Batch`: fixed waves of `batch_size` (one wave when 0).
    /// - `Canary`: `[0]` alone, then the remainder in `batch_size` chunks
    ///   (or a single remainder wave when `batch_size` is 0).
    pub fn wave_indices(&self) -> Vec<Vec<usize>> {
        let count = self.targets.len();
        let batch_size = self.options.batch_size;
        let mut waves = Vec::<Vec<usize>>::new();

        match self.options.strategy {
            RolloutStrategy::Canary => {
                if count == 0 {
                    return waves;
                }
                let canary = vec![0];
                waves.push(canary);
                if count > 1 {
                    let size = if batch_size == 0 { count - 1 } else { batch_size };
                    Self::append_slices(&mut waves, 1, count, size);
                }
            }
            RolloutStrategy::Batch => {
                let size = if batch_size == 0 { count } else { batch_size };
                Self::append_slices(&mut waves, 0, count, size);
            }
            RolloutStrategy::All => {
                if count == 0 {
                    return waves;
                }
                let mut all = Vec::<usize>::with_capacity(count);
                let mut i = 0;
                while i < count {
                    all.push(i);
                    i += 1;
                }
                waves.push(all);
            }
        }
        waves
    }

    /// Appends `[start, count)` sliced into `size`-sized subsequences.
    fn append_slices(waves: &mut Vec<Vec<usize>>, start: usize, count: usize, size: usize) {
        let mut first = start;
        while first < count {
            let mut wave = Vec::<usize>::new();
            let mut i = first;
            let mut taken = 0;
            while i < count && taken < size {
                wave.push(i);
                i += 1;
                taken += 1;
            }
            waves.push(wave);
            first = i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(count: usize, strategy: RolloutStrategy, batch_size: usize) -> DeploymentPlan {
        let mut targets = Vec::<TargetPlan>::with_capacity(count);
        let mut i = 0;
        while i < count {
            targets.push(TargetPlan {
                host_name: format!("h{}", i),
                action: DeploymentAction::Switch,
                new_closure: None,
                current_closure: None,
            });
            i += 1;
        }
        let mut options = DeploymentOptions::default_policy();
        options.strategy = strategy;
        options.batch_size = batch_size;
        DeploymentPlan { targets, options }
    }

    #[test]
    fn all_strategy_is_a_single_wave() {
        let plan = plan(6, RolloutStrategy::All, 0);
        let waves = plan.wave_indices();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn empty_plan_has_no_waves() {
        for strategy in [RolloutStrategy::All, RolloutStrategy::Canary, RolloutStrategy::Batch] {
            let mut options = DeploymentOptions::default_policy();
            options.strategy = strategy;
            let plan = DeploymentPlan {
                targets: Vec::new(),
                options,
            };
            assert!(plan.wave_indices().is_empty());
        }
    }

    #[test]
    fn batch_splits_fixed_size_slices() {
        let waves = plan(6, RolloutStrategy::Batch, 2).wave_indices();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec![0, 1]);
        assert_eq!(waves[1], vec![2, 3]);
        assert_eq!(waves[2], vec![4, 5]);
    }

    #[test]
    fn batch_zero_is_one_remaining_wave() {
        let waves = plan(5, RolloutStrategy::Batch, 0).wave_indices();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn canary_slices_first_host_then_remainder() {
        // Matches the documented semantics: `[0]` alone, then the remainder in
        // `batch_size` chunks (`wave_indices` doc comment), so the tail `[5]`
        // is its own final wave, not absorbed into the previous one.
        let waves = plan(6, RolloutStrategy::Canary, 2).wave_indices();
        assert_eq!(waves.len(), 4);
        assert_eq!(waves[0], vec![0]);
        assert_eq!(waves[1], vec![1, 2]);
        assert_eq!(waves[2], vec![3, 4]);
        assert_eq!(waves[3], vec![5]);
    }

    #[test]
    fn canary_single_host_has_no_second_wave() {
        let waves = plan(1, RolloutStrategy::Canary, 2).wave_indices();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec![0]);
    }

    #[test]
    fn action_round_trips() {
        assert_eq!(DeploymentAction::parse("switch"), Some(DeploymentAction::Switch));
        assert_eq!(DeploymentAction::parse("Boot"), Some(DeploymentAction::Boot));
        assert_eq!(DeploymentAction::parse("test"), Some(DeploymentAction::Test));
        assert_eq!(DeploymentAction::parse("dry-run"), Some(DeploymentAction::DryRun));
        assert_eq!(DeploymentAction::parse("nope"), None);
    }

    #[test]
    fn dry_run_detection() {
        let mut options = DeploymentOptions::default_policy();
        assert!(!options.is_dry_run());
        options.dry_run = true;
        assert!(options.is_dry_run());
        options.dry_run = false;
        options.action = DeploymentAction::DryRun;
        assert!(options.is_dry_run());
    }
}