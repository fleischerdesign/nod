//! Deployment pipeline: the guarded ADR-003 state machine each host drives.
//!
//! Application-led; no port I/O lives here. Use cases compose the machine
//! transitions around "real" port calls, see `src/application/use_cases`.

pub mod state_machine;