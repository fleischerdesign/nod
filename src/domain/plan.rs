//! Domain deployment-plan value objects (ADR-003, ADR-005).
//!
//! `DeploymentPlan`, `TargetPlan`, `DeploymentAction`, `TargetDiff` plus the
//! pure rollout wave-partition (`DeploymentPlan::waves`) are dependency-free;
//! engineers extend them without dragging in Application/Infrastructure.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::domain::errors::NodError;
use crate::domain::host::{BuilderHost, HostEntity};

/// What an individual target plan asks the pipeline to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeploymentAction {
    /// Rebuild and activate the NixOS configuration.
    Switch,
    /// Boot/toggle a host subsystem (forward-looking hook).
    Boot,
    /// Run post-activation verification only.
    Test,
    /// Build & deploy the new closure without activating it.
    DryRun,
    /// Build closures and create out-links without transferring or activating.
    Build,
}

impl DeploymentAction {
    /// Returns the canonical string form.
    pub fn to_str(&self) -> String {
        match self {
            DeploymentAction::Switch => String::from("switch"),
            DeploymentAction::Boot => String::from("boot"),
            DeploymentAction::Test => String::from("test"),
            DeploymentAction::DryRun => String::from("dry-run"),
            DeploymentAction::Build => String::from("build"),
        }
    }

    /// Parses a CLI `--action <...>` value, or `None` when unrecognised.
    pub fn parse(s: &str) -> Option<DeploymentAction> {
        match s.to_lowercase().as_str() {
            "switch" => Some(DeploymentAction::Switch),
            "boot" => Some(DeploymentAction::Boot),
            "test" => Some(DeploymentAction::Test),
            "dry-run" => Some(DeploymentAction::DryRun),
            "build" => Some(DeploymentAction::Build),
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
    /// Optional symlink target for build closures (`nod build --out-link`).
    pub out_link: Option<PathBuf>,
    /// Optional builder fleet host on which to compile closures remotely
    /// (`nod build --builder`). Empty for a plain local build.
    pub builder: Option<BuilderHost>,
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
            out_link: None,
            builder: None,
        }
    }

    /// `default_policy` for the given `action`, for planning helpers that
    /// only carry the action (see [`DeploymentPlan::from_hosts`]).
    pub fn default_for(action: DeploymentAction) -> Self {
        let mut options = Self::default_policy();
        options.action = action;
        options
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
    /// Hosts that must be deployed before this target.
    #[serde(default)]
    pub depends_on: Vec<String>,
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

    /// Builds a plan for `hosts` without resolving closures (dry-run /
    /// preview): every target carries `action` and no `new_closure` /
    /// `current_closure`. The plan's options default to `default_for(action)`;
    /// callers that need a full policy override `options` afterwards.
    pub fn from_hosts(hosts: Vec<HostEntity>, action: DeploymentAction) -> DeploymentPlan {
        let targets = hosts
            .into_iter()
            .map(|host| {
                let depends_on = if !host.nod_config.rollout.depends_on.is_empty() {
                    host.nod_config.rollout.depends_on.clone()
                } else {
                    host.nod_config.depends_on.clone()
                };
                TargetPlan {
                    host_name: host.name,
                    action: action.clone(),
                    new_closure: None,
                    current_closure: None,
                    depends_on,
                }
            })
            .collect();
        DeploymentPlan {
            targets,
            options: DeploymentOptions::default_for(action),
        }
    }

    /// Partitions target indices into topological DAG levels based on `depends_on`.
    /// Returns an error if a cyclic dependency is detected.
    pub fn topological_levels(&self) -> Result<Vec<Vec<usize>>, NodError> {
        let count = self.targets.len();
        if count == 0 {
            return Ok(Vec::new());
        }

        // Map host_name -> index
        let name_to_idx: std::collections::HashMap<&str, usize> = self
            .targets
            .iter()
            .enumerate()
            .map(|(idx, t)| (t.host_name.as_str(), idx))
            .collect();

        // Build adjacency and in-degree counts
        let mut in_degrees = vec![0usize; count];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];

        for (idx, target) in self.targets.iter().enumerate() {
            for dep in &target.depends_on {
                if let Some(&dep_idx) = name_to_idx.get(dep.as_str()) {
                    if dep_idx != idx {
                        in_degrees[idx] += 1;
                        dependents[dep_idx].push(idx);
                    }
                }
            }
        }

        // Collect initial 0-degree nodes
        let mut current_level: Vec<usize> = in_degrees
            .iter()
            .enumerate()
            .filter_map(|(idx, &deg)| if deg == 0 { Some(idx) } else { None })
            .collect();

        let mut levels = Vec::<Vec<usize>>::new();
        let mut processed_count = 0usize;

        while !current_level.is_empty() {
            processed_count += current_level.len();
            let mut next_level = Vec::<usize>::new();

            for &node in &current_level {
                for &dependent in &dependents[node] {
                    in_degrees[dependent] -= 1;
                    if in_degrees[dependent] == 0 {
                        next_level.push(dependent);
                    }
                }
            }

            levels.push(current_level);
            current_level = next_level;
        }

        if processed_count < count {
            let unresolved: Vec<String> = in_degrees
                .iter()
                .enumerate()
                .filter(|(_, &deg)| deg > 0)
                .map(|(idx, _)| self.targets[idx].host_name.clone())
                .collect();
            return Err(NodError::config(format!(
                "cyclic dependency detected in deployment plan involving hosts: {}",
                unresolved.join(", ")
            )));
        }

        Ok(levels)
    }

    /// Partitions target indices into rollout waves for the active strategy,
    /// respecting DAG dependencies and topological levels.
    pub fn wave_indices(&self) -> Result<Vec<Vec<usize>>, NodError> {
        let levels = self.topological_levels()?;
        let batch_size = self.options.batch_size;
        let mut waves = Vec::<Vec<usize>>::new();

        let mut is_first_level = true;

        for level in levels {
            let count = level.len();
            if count == 0 {
                continue;
            }

            match self.options.strategy {
                RolloutStrategy::Canary => {
                    if is_first_level {
                        let canary = vec![level[0]];
                        waves.push(canary);
                        if count > 1 {
                            let size = if batch_size == 0 {
                                count - 1
                            } else {
                                batch_size
                            };
                            Self::append_slices_from_vec(&mut waves, &level, 1, size);
                        }
                    } else {
                        let size = if batch_size == 0 { count } else { batch_size };
                        Self::append_slices_from_vec(&mut waves, &level, 0, size);
                    }
                }
                RolloutStrategy::Batch => {
                    let size = if batch_size == 0 { count } else { batch_size };
                    Self::append_slices_from_vec(&mut waves, &level, 0, size);
                }
                RolloutStrategy::All => {
                    waves.push(level);
                }
            }
            is_first_level = false;
        }

        Ok(waves)
    }

    /// Appends slices of `items[start..]` of length `size` into `waves`.
    fn append_slices_from_vec(
        waves: &mut Vec<Vec<usize>>,
        items: &[usize],
        start: usize,
        size: usize,
    ) {
        let total = items.len();
        let mut first = start;
        while first < total {
            let mut wave = Vec::<usize>::new();
            let mut i = first;
            let mut taken = 0;
            while i < total && taken < size {
                wave.push(items[i]);
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
                depends_on: Vec::new(),
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
        let waves = plan.wave_indices().unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn empty_plan_has_no_waves() {
        for strategy in [
            RolloutStrategy::All,
            RolloutStrategy::Canary,
            RolloutStrategy::Batch,
        ] {
            let mut options = DeploymentOptions::default_policy();
            options.strategy = strategy;
            let plan = DeploymentPlan {
                targets: Vec::new(),
                options,
            };
            assert!(plan.wave_indices().unwrap().is_empty());
        }
    }

    #[test]
    fn batch_splits_fixed_size_slices() {
        let waves = plan(6, RolloutStrategy::Batch, 2).wave_indices().unwrap();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec![0, 1]);
        assert_eq!(waves[1], vec![2, 3]);
        assert_eq!(waves[2], vec![4, 5]);
    }

    #[test]
    fn batch_zero_is_one_remaining_wave() {
        let waves = plan(5, RolloutStrategy::Batch, 0).wave_indices().unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn canary_slices_first_host_then_remainder() {
        // Matches the documented semantics: `[0]` alone, then the remainder in
        // `batch_size` chunks (`wave_indices` doc comment), so the tail `[5]`
        // is its own final wave, not absorbed into the previous one.
        let waves = plan(6, RolloutStrategy::Canary, 2).wave_indices().unwrap();
        assert_eq!(waves.len(), 4);
        assert_eq!(waves[0], vec![0]);
        assert_eq!(waves[1], vec![1, 2]);
        assert_eq!(waves[2], vec![3, 4]);
        assert_eq!(waves[3], vec![5]);
    }

    #[test]
    fn canary_single_host_has_no_second_wave() {
        let waves = plan(1, RolloutStrategy::Canary, 2).wave_indices().unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec![0]);
    }

    #[test]
    fn dag_dependency_levels_partition_correctly() {
        // h0 (db) -> h1 (api1), h2 (api2) -> h3 (proxy)
        let mut plan = plan(4, RolloutStrategy::All, 0);
        plan.targets[1].depends_on = vec!["h0".to_string()];
        plan.targets[2].depends_on = vec!["h0".to_string()];
        plan.targets[3].depends_on = vec!["h1".to_string(), "h2".to_string()];

        let waves = plan.wave_indices().unwrap();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec![0]);
        assert_eq!(waves[1], vec![1, 2]);
        assert_eq!(waves[2], vec![3]);
    }

    #[test]
    fn dag_cycle_detection_returns_error() {
        let mut plan = plan(2, RolloutStrategy::All, 0);
        plan.targets[0].depends_on = vec!["h1".to_string()];
        plan.targets[1].depends_on = vec!["h0".to_string()];

        let err = plan.wave_indices().unwrap_err();
        assert!(matches!(err, NodError::Config { .. }));
        assert!(err.to_string().contains("cyclic dependency detected"));
    }

    #[test]
    fn action_round_trips() {
        assert_eq!(
            DeploymentAction::parse("switch"),
            Some(DeploymentAction::Switch)
        );
        assert_eq!(
            DeploymentAction::parse("Boot"),
            Some(DeploymentAction::Boot)
        );
        assert_eq!(
            DeploymentAction::parse("test"),
            Some(DeploymentAction::Test)
        );
        assert_eq!(
            DeploymentAction::parse("dry-run"),
            Some(DeploymentAction::DryRun)
        );
        assert_eq!(
            DeploymentAction::parse("build"),
            Some(DeploymentAction::Build)
        );
        assert_eq!(DeploymentAction::parse("nope"), None);
    }

    #[test]
    fn default_policy_has_no_builder() {
        let options = DeploymentOptions::default_policy();
        assert!(options.builder.is_none());
        assert!(options.out_link.is_none());
    }

    #[test]
    fn deployment_options_round_trip_with_builder() {
        let mut options = DeploymentOptions::default_policy();
        options.builder = Some(BuilderHost {
            target_host: "buildy".to_string(),
            profile: crate::domain::host::SshProfile::new("dep", 2200),
        });
        let json = serde_json::to_string(&options).unwrap();
        let back: DeploymentOptions = serde_json::from_str(&json).unwrap();
        if let Some(builder) = back.builder {
            assert_eq!(builder.target_host, "buildy");
            assert_eq!(builder.profile.user(), "dep");
            assert_eq!(builder.profile.port(), 2200);
        } else {
            panic!("expected the builder to survive serialization");
        }
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
