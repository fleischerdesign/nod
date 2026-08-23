//! Deployment state machine (ADR-003).
//!
//! Each host drives a single explicit lifecycle: `Prepared` -> ... ->
//! `Completed`, or into `RollbackTriggered` -> `RolledBack` / `Failed` on the
//! first failing stage. Every transition is guarded: only the legal successor
//! event mutates the machine; anything else raises `NodError::invariant`.

use crate::domain::errors::NodError;

/// The lifecycle stages a host may occupy during deployment (ADR-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeploymentState {
    /// Constructed but not yet started.
    Prepared,
    /// Host discovery / closure resolution.
    Evaluating,
    /// Building the toplevel closure.
    Building,
    /// Copying the closure into the (remote) store.
    Transferring,
    /// Activating `switch-to-configuration`.
    Switching,
    /// Post-activation health / closure probing.
    Verifying,
    /// Successful activation and verification.
    Completed,
    /// A stage failed; rollback recovery is underway.
    RollbackTriggered,
    /// Rollback completed; the host is on its previous known-good state.
    RolledBack,
    /// An irreversible failure (including failed rollback).
    Failed,
}

/// Advances the machine. Carry the human-readable stage oldertyped message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeploymentEvent {
    /// `Prepared -> Evaluating`
    Begin,
    /// `Evaluating -> Building`
    EvalOk,
    /// `Building -> Transferring`
    BuildOk,
    /// `Transferring -> Switching`
    TransferOk,
    /// `Switching -> Verifying`
    SwitchOk,
    /// `Verifying -> Completed`
    VerifyOk,
    /// `Evaluating -> RollbackTriggered`
    EvalFail,
    /// `Building -> RollbackTriggered`
    BuildFail,
    /// `Transferring -> RollbackTriggered`
    TransferFail,
    /// `Switching -> RollbackTriggered`
    SwitchFail,
    /// `Verifying -> RollbackTriggered`
    VerifyFail,
    /// `RollbackTriggered -> RolledBack`
    RollbackOk,
    /// `RollbackTriggered -> Failed`
    RollbackFail,
}

impl DeploymentEvent {
    /// Human-readable event name for diagnostics.
    pub fn to_str(&self) -> String {
        match self {
            DeploymentEvent::Begin => String::from("begin"),
            DeploymentEvent::EvalOk => String::from("eval-ok"),
            DeploymentEvent::BuildOk => String::from("build-ok"),
            DeploymentEvent::TransferOk => String::from("transfer-ok"),
            DeploymentEvent::SwitchOk => String::from("switch-ok"),
            DeploymentEvent::VerifyOk => String::from("verify-ok"),
            DeploymentEvent::EvalFail => String::from("eval-fail"),
            DeploymentEvent::BuildFail => String::from("build-fail"),
            DeploymentEvent::TransferFail => String::from("transfer-fail"),
            DeploymentEvent::SwitchFail => String::from("switch-fail"),
            DeploymentEvent::VerifyFail => String::from("verify-fail"),
            DeploymentEvent::RollbackOk => String::from("rollback-ok"),
            DeploymentEvent::RollbackFail => String::from("rollback-fail"),
        }
    }
}

/// A structurally-guarded, single-host deployment lifecycle.
#[derive(Debug, Clone)]
pub struct DeploymentStateMachine {
    state: DeploymentState,
}

impl DeploymentStateMachine {
    /// Builds a machine rooted in `Prepared`.
    pub fn prepared() -> Self {
        Self { state: DeploymentState::Prepared }
    }

    /// The current state.
    pub fn state(&self) -> DeploymentState {
        self.state.clone()
    }

    /// Stable lower-case label for observability / persistence.
    pub fn name(&self) -> String {
        self.state.to_str()
    }

    /// True when `event` is the legal successor of the current state.
    pub fn can(&self, event: DeploymentEvent) -> bool {
        Self::target_of(self.state.clone(), event.clone()).is_some()
    }

    /// Advances the machine one step, or raises an invariant error when the
    /// event is not legal from the current state.
    pub fn tick(&mut self, event: DeploymentEvent) -> Result<DeploymentState, NodError> {
        let next = Self::target_of(self.state.clone(), event.clone());
        match next {
            Some(n) => {
                self.state = n;
                Ok(self.state.clone())
            }
            None => Err(NodError::invariant(format!(
                "illegal transition {} -> {}",
                self.state.to_str(),
                event.to_str()
            ))),
        }
    }

    /// Pure transition table: `Some(next)` when `state + event` is legal.
    fn is_not_terminal(state: &DeploymentState) -> bool {
        match state {
            DeploymentState::Completed
            | DeploymentState::RolledBack
            | DeploymentState::Failed => false,
            _ => true,
        }
    }

    fn target_of(state: DeploymentState, event: DeploymentEvent) -> Option<DeploymentState> {
        // A terminal state refuses every event.
        if !Self::is_not_terminal(&state) {
            return None;
        }
        match event {
            DeploymentEvent::Begin => {
                if state == DeploymentState::Prepared {
                    Some(DeploymentState::Evaluating)
                } else {
                    None
                }
            }
            DeploymentEvent::EvalOk => {
                if state == DeploymentState::Evaluating {
                    Some(DeploymentState::Building)
                } else {
                    None
                }
            }
            DeploymentEvent::BuildOk => {
                if state == DeploymentState::Building {
                    Some(DeploymentState::Transferring)
                } else {
                    None
                }
            }
            DeploymentEvent::TransferOk => {
                if state == DeploymentState::Transferring {
                    Some(DeploymentState::Switching)
                } else {
                    None
                }
            }
            DeploymentEvent::SwitchOk => {
                if state == DeploymentState::Switching {
                    Some(DeploymentState::Verifying)
                } else {
                    None
                }
            }
            DeploymentEvent::VerifyOk => {
                if state == DeploymentState::Verifying {
                    Some(DeploymentState::Completed)
                } else {
                    None
                }
            }
            DeploymentEvent::EvalFail => Self::rollback_from(state, DeploymentState::Evaluating),
            DeploymentEvent::BuildFail => Self::rollback_from(state, DeploymentState::Building),
            DeploymentEvent::TransferFail => {
                Self::rollback_from(state, DeploymentState::Transferring)
            }
            DeploymentEvent::SwitchFail => Self::rollback_from(state, DeploymentState::Switching),
            DeploymentEvent::VerifyFail => Self::rollback_from(state, DeploymentState::Verifying),
            DeploymentEvent::RollbackOk => {
                if state == DeploymentState::RollbackTriggered {
                    Some(DeploymentState::RolledBack)
                } else {
                    None
                }
            }
            DeploymentEvent::RollbackFail => {
                if state == DeploymentState::RollbackTriggered {
                    Some(DeploymentState::Failed)
                } else {
                    None
                }
            }
        }
    }

    /// `state -> RollbackTriggered` when `state` currently sits on `stage`.
    fn rollback_from(state: DeploymentState, stage: DeploymentState) -> Option<DeploymentState> {
        if state == stage {
            Some(DeploymentState::RollbackTriggered)
        } else {
            None
        }
    }
}

impl DeploymentState {
    /// Stable lower-case label used by observability and persistence.
    pub fn to_str(&self) -> String {
        match self {
            DeploymentState::Prepared => String::from("prepared"),
            DeploymentState::Evaluating => String::from("evaluating"),
            DeploymentState::Building => String::from("building"),
            DeploymentState::Transferring => String::from("transferring"),
            DeploymentState::Switching => String::from("switching"),
            DeploymentState::Verifying => String::from("verifying"),
            DeploymentState::Completed => String::from("completed"),
            DeploymentState::RollbackTriggered => String::from("rollback-triggered"),
            DeploymentState::RolledBack => String::from("rolled-back"),
            DeploymentState::Failed => String::from("failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_state(mut machine: &mut DeploymentStateMachine, event: DeploymentEvent) -> DeploymentState {
        machine.tick(event).unwrap()
    }

    #[test]
    fn healthy_host_advances_to_completed() {
        let mut machine = DeploymentStateMachine::prepared();
        assert_eq!(machine.state(), DeploymentState::Prepared);

        assert_eq!(assert_state(&mut machine, DeploymentEvent::Begin), DeploymentState::Evaluating);
        assert_eq!(assert_state(&mut machine, DeploymentEvent::EvalOk), DeploymentState::Building);
        assert_eq!(
            assert_state(&mut machine, DeploymentEvent::BuildOk),
            DeploymentState::Transferring
        );
        assert_eq!(
            assert_state(&mut machine, DeploymentEvent::TransferOk),
            DeploymentState::Switching
        );
        assert_eq!(
            assert_state(&mut machine, DeploymentEvent::SwitchOk),
            DeploymentState::Verifying
        );
        assert_eq!(
            assert_state(&mut machine, DeploymentEvent::VerifyOk),
            DeploymentState::Completed
        );
    }

    #[test]
    fn illegal_forward_transition_is_rejected() {
        let mut machine = DeploymentStateMachine::prepared();
        // Jump straight to "switching" is not legal.
        let err = machine.tick(DeploymentEvent::SwitchOk).err().unwrap();
        assert!(matches!(err, NodError::Internal { .. }));
        assert_eq!(machine.state(), DeploymentState::Prepared);
    }

    #[test]
    fn switch_failure_triggers_rollback_and_recovery() {
        let mut machine = DeploymentStateMachine::prepared();
        step_assert(&mut machine, DeploymentState::Switching);
        let state = assert_state(&mut machine, DeploymentEvent::SwitchFail);
        assert_eq!(state, DeploymentState::RollbackTriggered);
        assert_eq!(
            assert_state(&mut machine, DeploymentEvent::RollbackOk),
            DeploymentState::RolledBack
        );
    }

    #[test]
    fn verify_failure_with_failed_rollback_ends_failed() {
        let mut machine = DeploymentStateMachine::prepared();
        step_assert(&mut machine, DeploymentState::Verifying);
        assert_eq!(
            assert_state(&mut machine, DeploymentEvent::VerifyFail),
            DeploymentState::RollbackTriggered
        );
        assert_eq!(
            assert_state(&mut machine, DeploymentEvent::RollbackFail),
            DeploymentState::Failed
        );
    }

    #[test]
    fn active_stage_failures_enter_recovery() {
        for (stage, event) in [
            (DeploymentState::Evaluating, DeploymentEvent::EvalFail),
            (DeploymentState::Building, DeploymentEvent::BuildFail),
            (DeploymentState::Transferring, DeploymentEvent::TransferFail),
            (DeploymentState::Switching, DeploymentEvent::SwitchFail),
            (DeploymentState::Verifying, DeploymentEvent::VerifyFail),
        ] {
            let mut machine = DeploymentStateMachine::prepared();
            step_assert(&mut machine, stage);
            let state = assert_state(&mut machine, event);
            assert_eq!(state, DeploymentState::RollbackTriggered);
        }
    }

    #[test]
    fn terminal_states_refuse_further_events() {
        let mut machine = DeploymentStateMachine::prepared();
        step_assert(&mut machine, DeploymentState::Completed);
        let err = machine.tick(DeploymentEvent::VerifyOk).err().unwrap();
        assert!(matches!(err, NodError::Internal { .. }));
    }

    #[test]
    fn rollback_only_from_triggered_not_from_active_stage() {
        // A host in `Prepared` cannot roll back (never began).
        let mut machine = DeploymentStateMachine::prepared();
        assert!(machine.tick(DeploymentEvent::RollbackOk).is_err());
        // A host in `Building` rejects a verifying failure.
        machine.tick(DeploymentEvent::Begin).unwrap();
        machine.tick(DeploymentEvent::EvalOk).unwrap();
        assert!(machine.tick(DeploymentEvent::VerifyFail).err().is_some());
    }

    /// Advances the machine to a given non-terminal stage.
    fn step_assert(mut machine: &mut DeploymentStateMachine, to: DeploymentState) {
        let mut m = machine;
        match to {
            DeploymentState::Evaluating => {
                m.tick(DeploymentEvent::Begin).unwrap();
            }
            DeploymentState::Building => {
                m.tick(DeploymentEvent::Begin).unwrap();
                m.tick(DeploymentEvent::EvalOk).unwrap();
            }
            DeploymentState::Transferring => {
                step_assert(m, DeploymentState::Building);
                m.tick(DeploymentEvent::BuildOk).unwrap();
            }
            DeploymentState::Switching => {
                step_assert(m, DeploymentState::Transferring);
                m.tick(DeploymentEvent::TransferOk).unwrap();
            }
            DeploymentState::Verifying => {
                step_assert(m, DeploymentState::Switching);
                m.tick(DeploymentEvent::SwitchOk).unwrap();
            }
            DeploymentState::Completed => {
                step_assert(m, DeploymentState::Verifying);
                m.tick(DeploymentEvent::VerifyOk).unwrap();
            }
            _ => panic!("cannot drive to this state"),
        }
    }
}