//! Contract tests for the provider seam (ADR-030).
//!
//! These tests run the real supervisor against a shell provider: they prove
//! what the core promises when a provider half-fails or reports outside its
//! declaration. A unit test cannot reach that seam.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nod::application::use_cases::adopt::{execute as adopt, AdoptOptions};
use nod::application::use_cases::derive::{DeriveRule, Source, Template};
use nod::application::use_cases::reconcile::{execute, rollback, Desired, ReconcileOptions};
use nod::domain::errors::NodError;
use nod::domain::ownership::{NoopSink, OwnedId, Ownership, OwnershipSink};
use nod::domain::policy::Policy;
use nod::domain::provider::{Change, ChangeKind, DesiredResource, InverseChange, RollbackPlan};

/// These tests write an executable and spawn it. Run in parallel, one test's
/// `fork` can inherit another's still-open write descriptor and make the
/// subsequent `exec` fail with ETXTBSY. The supervisor is not at fault — a
/// store path is immutable — so the harness serialises instead of retrying.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn script(body: impl AsRef<str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake-provider.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{}", body.as_ref())).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    (dir, path)
}

fn desired(ids: &[&str]) -> Desired {
    Desired {
        resources: ids
            .iter()
            .map(|id| DesiredResource {
                kind: "dns.record".to_string(),
                id: (*id).to_string(),
                attributes: serde_json::json!({ "content": "192.0.2.1" }),
            })
            .collect(),
        absent: Vec::new(),
        derive: Vec::new(),
    }
}

/// A provider that plans two creates and reports only one as applied.
const HALF_FAILING: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":["dns.record"],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      echo '{"changes":[{"kind":"create","owned":true,"destructive":false,"resource":{"kind":"dns.record","id":"a"}},{"kind":"create","owned":true,"destructive":false,"resource":{"kind":"dns.record","id":"b"}}],"conditions":[]}'
      ;;
    *'"op":"apply"'*)
      echo '{"applied":[{"id":"a","ok":true},{"id":"b","ok":false,"error":{"kind":"auth","message":"token expired"}}],"conditions":[]}'
      ;;
  esac
done
"#;

#[tokio::test]
async fn a_half_applied_plan_records_only_what_happened() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(HALF_FAILING);

    let report = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &desired(&["a", "b"]),
        &Ownership::default(),
        &NoopSink,
        ReconcileOptions { apply: true },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert_eq!(report.applied, 1, "only the reported change counts");
    assert_eq!(report.failed.len(), 1);
    assert!(
        report.failed[0].contains("auth: token expired"),
        "{:?}",
        report.failed
    );
    assert_eq!(
        report.inverse.len(),
        1,
        "an undo must not cover a change that never happened"
    );
    assert_eq!(report.inverse[0].change.resource.id, "a");
    assert_eq!(report.verified, 0, "the fake provider never converged");
    assert_eq!(report.desired, 2);
}

#[tokio::test]
async fn a_missing_result_is_not_a_success() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      echo '{"changes":[{"kind":"create","owned":true,"destructive":false,"resource":{"kind":"dns.record","id":"a"}}],"conditions":[]}'
      ;;
    *'"op":"apply"'*)
      echo '{"applied":[],"conditions":[]}'
      ;;
  esac
done
"#,
    );

    let report = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &desired(&["a"]),
        &Ownership::default(),
        &NoopSink,
        ReconcileOptions { apply: true },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert_eq!(report.applied, 0);
    assert!(
        report.failed[0].contains("no result"),
        "{:?}",
        report.failed
    );
    assert!(report.inverse.is_empty());
}

#[tokio::test]
async fn a_failed_undo_is_reported_and_never_counted_as_restored() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[{"kind":"dns.record","id":"a","attributes":{"content":"1"},"owned":true}],"conditions":[]}'
      ;;
    *'"op":"apply"'*)
      echo '{"applied":[{"id":"a","ok":false,"error":{"kind":"auth","message":"token expired"}}],"conditions":[]}'
      ;;
  esac
done
"#,
    );

    let resource = DesiredResource {
        kind: "dns.record".to_string(),
        id: "a".to_string(),
        attributes: serde_json::json!({ "content": "1" }),
    };
    let plan = RollbackPlan {
        provider: "fake".to_string(),
        applied_at: 1,
        changes: vec![InverseChange {
            change: Change {
                kind: ChangeKind::Delete,
                owned: true,
                destructive: true,
                resource: resource.clone(),
            },
            expected: Some(resource),
        }],
    };

    let report = rollback(
        Path::new(&path),
        &serde_json::json!({}),
        &plan,
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert_eq!(report.restored, 0);
    assert_eq!(report.failed.len(), 1);
    assert!(
        report.failed[0].contains("auth: token expired"),
        "{:?}",
        report.failed
    );
}

#[tokio::test]
async fn adoption_is_a_dry_run_until_asked_and_stamps_only_eligible_resources() {
    let _guard = SERIAL.lock().await;
    let marker = tempfile::tempdir().unwrap();
    let marker_path = marker.path().join("stamped");
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"operations":["adopt"],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[{"kind":"dns.record","id":"a","attributes":{"content":"1","record_id":"cf-1"},"owned":false}],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      echo '{"changes":[],"conditions":[]}'
      ;;
    *'"op":"adopt"'*)
      touch @MARKER@
      echo '{"adopted":[{"id":"a","ok":true}],"conditions":[]}'
      ;;
  esac
done
"#
        .replace("@MARKER@", marker_path.to_str().unwrap()),
    );

    let desired = vec![DesiredResource {
        kind: "dns.record".to_string(),
        id: "a".to_string(),
        attributes: serde_json::json!({ "content": "1" }),
    }];

    let dry = adopt(
        Path::new(&path),
        &serde_json::json!({}),
        &desired,
        AdoptOptions::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(dry.unowned, 1);
    assert_eq!(dry.eligible, 1);
    assert_eq!(dry.adopted, 0);
    assert!(
        !marker_path.exists(),
        "a dry run must not call the provider's adopt"
    );

    let applied = adopt(
        Path::new(&path),
        &serde_json::json!({}),
        &desired,
        AdoptOptions {
            apply: true,
            force: Vec::new(),
        },
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(applied.adopted, 1);
    assert!(marker_path.exists());
}

#[tokio::test]
async fn a_conflict_needs_an_explicit_force_naming_the_resource() {
    let _guard = SERIAL.lock().await;
    let marker = tempfile::tempdir().unwrap();
    let marker_path = marker.path().join("stamped");
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"operations":["adopt"],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[{"kind":"dns.record","id":"a","attributes":{"content":"2","record_id":"cf-1"},"owned":false}],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      echo '{"changes":[{"kind":"update","owned":true,"destructive":false,"resource":{"kind":"dns.record","id":"a"}}],"conditions":[]}'
      ;;
    *'"op":"adopt"'*)
      touch @MARKER@
      echo '{"adopted":[{"id":"a","ok":true}],"conditions":[]}'
      ;;
  esac
done
"#
        .replace("@MARKER@", marker_path.to_str().unwrap()),
    );

    let desired = vec![DesiredResource {
        kind: "dns.record".to_string(),
        id: "a".to_string(),
        attributes: serde_json::json!({ "content": "1" }),
    }];

    let plain = adopt(
        Path::new(&path),
        &serde_json::json!({}),
        &desired,
        AdoptOptions {
            apply: true,
            force: Vec::new(),
        },
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(plain.eligible, 0);
    assert_eq!(plain.conflicts.len(), 1);
    assert_eq!(plain.adopted, 0);
    assert!(!marker_path.exists());

    let forced = adopt(
        Path::new(&path),
        &serde_json::json!({}),
        &desired,
        AdoptOptions {
            apply: true,
            force: vec!["a".to_string()],
        },
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(forced.eligible, 1);
    assert_eq!(forced.adopted, 1);
    assert!(marker_path.exists());
}

#[tokio::test]
async fn a_provider_without_the_adopt_operation_is_refused() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"ownership":"marker","settings_schema":{}}'
      ;;
  esac
done
"#,
    );

    let error = adopt(
        Path::new(&path),
        &serde_json::json!({}),
        &[],
        AdoptOptions::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect_err("a provider without adopt must be refused");
    assert!(error.to_string().contains("adopt"), "{error}");
}

#[tokio::test]
async fn a_provider_that_stops_answering_is_killed_at_the_declared_timeout() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      sleep 30
      ;;
  esac
done
"#,
    );

    let error = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &desired(&["a"]),
        &Ownership::default(),
        &NoopSink,
        ReconcileOptions { apply: false },
        &Policy::default(),
        &[],
        Duration::from_millis(200),
    )
    .await
    .expect_err("a provider that never answers must not block the reconciler");

    assert!(error.to_string().contains("timed out"), "{error}");
}

#[tokio::test]
async fn a_provider_that_observes_an_undeclared_kind_is_rejected() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":["dns.record"],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[{"kind":"mail.mailbox","id":"inbox","owned":true}],"conditions":[]}'
      ;;
  esac
done
"#,
    );

    let error = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &desired(&["a"]),
        &Ownership::default(),
        &NoopSink,
        ReconcileOptions { apply: false },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect_err("an undeclared kind must fail the run");

    assert!(error.to_string().contains("mail.mailbox"), "{error}");
}

fn deriving_rule(max_deletions: usize) -> DeriveRule {
    DeriveRule {
        id: "per-member".to_string(),
        from: Source {
            kind: "authentik.group".to_string(),
            id: "ai-users".to_string(),
            attribute: "users".to_string(),
        },
        template: Template {
            kind: "authentik.group".to_string(),
            id: "per-{member}".to_string(),
            attributes: serde_json::json!({ "description": "{member}" }),
        },
        max_deletions,
    }
}

#[tokio::test]
async fn a_derivation_reaches_the_provider_and_is_recorded() {
    let _guard = SERIAL.lock().await;
    let work = tempfile::tempdir().unwrap();
    let requests = work.path().join("plan-request.json");
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[{"kind":"authentik.group","id":"ai-users","attributes":{"users":["ada","zoe"]},"owned":true}],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      printf '%s\n' "$line" > @REQUESTS@
      echo '{"changes":[],"conditions":[]}'
      ;;
  esac
done
"#
        .replace("@REQUESTS@", requests.to_str().unwrap()),
    );

    let declaration = Desired {
        resources: Vec::new(),
        absent: Vec::new(),
        derive: vec![deriving_rule(0)],
    };

    let report = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &declaration,
        &Ownership::default(),
        &NoopSink,
        ReconcileOptions { apply: false },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("the derivation should run");

    let request = std::fs::read_to_string(&requests).unwrap();
    assert!(request.contains("per-ada"), "{request}");
    assert!(request.contains("per-zoe"), "{request}");
    assert_eq!(report.derived, 2, "both members materialise");
    assert_eq!(report.ownership.rule("per-member").len(), 2);
    assert!(
        report
            .derived_resources
            .iter()
            .any(|entry| entry.member == "ada" && entry.rule == "per-member"),
        "{:?}",
        report.derived_resources
    );
}

/// ADR-037: a kind without a marker is owned by the record, not by the object.
///
/// The provider reports `owned: false` because it has no marker to show. The
/// core must still hand it an owned resource: an unowned update is refused by
/// the core itself, so without the record a derived kind could never converge.
#[tokio::test]
async fn a_recorded_derivation_is_owned_without_a_marker() {
    let _guard = SERIAL.lock().await;
    let work = tempfile::tempdir().unwrap();
    let requests = work.path().join("plan.json");
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[{"kind":"authentik.group","id":"ai-users","attributes":{"users":["ada"]},"owned":true},{"kind":"authentik.application","id":"app-ada","attributes":{},"owned":false}],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      printf '%s\n' "$line" > @REQUESTS@
      case "$line" in
        *'"kind":"authentik.application","id":"app-ada","attributes":{},"owned":true'*)
          echo '{"changes":[],"conditions":[]}'
          ;;
        *)
          # No marker and no record: the core must not be able to get here.
          echo '{"changes":[{"kind":"update","owned":false,"destructive":false,"resource":{"kind":"authentik.application","id":"app-ada","attributes":{}}}],"conditions":[]}'
          ;;
      esac
      ;;
  esac
done
"#
        .replace("@REQUESTS@", requests.to_str().unwrap()),
    );

    let declaration = Desired {
        resources: Vec::new(),
        absent: Vec::new(),
        derive: vec![DeriveRule {
            id: "per-member".to_string(),
            from: Source {
                kind: "authentik.group".to_string(),
                id: "ai-users".to_string(),
                attribute: "users".to_string(),
            },
            template: Template {
                kind: "authentik.application".to_string(),
                id: "app-{member}".to_string(),
                attributes: serde_json::json!({}),
            },
            max_deletions: 0,
        }],
    };
    let mut record = Ownership::default();
    record.rules.insert(
        "per-member".to_string(),
        vec![OwnedId {
            kind: "authentik.application".to_string(),
            id: "app-ada".to_string(),
        }],
    );

    let report = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &declaration,
        &record,
        &NoopSink,
        ReconcileOptions { apply: false },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("a recorded derivation must reconcile");

    assert_eq!(report.planned, 0, "the provider must see a satisfied set");
    assert_eq!(report.unowned, 0, "the record is the ownership evidence");
    assert_eq!(report.verified, 1);

    let request: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&requests).unwrap()).unwrap();
    let observed = request["observed"].as_array().expect("observed list");
    let application = observed
        .iter()
        .find(|resource| resource["id"] == "app-ada")
        .expect("the application is observed");
    assert_eq!(application["owned"], true, "the core owns the record");
}

#[tokio::test]
async fn a_derived_removal_over_its_budget_refuses_the_plan() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(
        r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":[],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[{"kind":"authentik.group","id":"ai-users","attributes":{"users":["zoe"]},"owned":true},{"kind":"authentik.group","id":"per-ada","attributes":{"description":"ada"},"owned":true}],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      echo '{"changes":[{"kind":"delete","owned":true,"destructive":true,"resource":{"kind":"authentik.group","id":"per-ada","attributes":{}}}],"conditions":[]}'
      ;;
  esac
done
"#,
    );

    let declaration = Desired {
        resources: Vec::new(),
        absent: Vec::new(),
        derive: vec![deriving_rule(0)],
    };
    let mut record = Ownership::default();
    record.rules.insert(
        "per-member".to_string(),
        vec![nod::domain::ownership::OwnedId {
            kind: "authentik.group".to_string(),
            id: "per-ada".to_string(),
        }],
    );

    let report = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &declaration,
        &record,
        &NoopSink,
        ReconcileOptions { apply: false },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("the refusal is a report, not an error");

    assert!(
        report.approved.is_empty(),
        "a refused plan must apply nothing"
    );
    assert!(
        report
            .refused
            .iter()
            .any(|reason| reason.contains("derivation `per-member`")),
        "{:?}",
        report.refused
    );
}

/// A creation is evidence that outlives the run: the next reconcile still owns
/// the resource, even though the provider reports `owned: false`.
#[tokio::test]
async fn a_created_resource_is_still_owned_on_the_next_run() {
    let _guard = SERIAL.lock().await;
    let work = tempfile::tempdir().unwrap();
    let state = work.path().join("created");
    let (_dir, path) = script(
        r#"
if [ -f @STATE@ ]; then
  observed='[{"kind":"authentik.application","id":"canary","attributes":{},"owned":false}]'
  changes='[]'
else
  observed='[]'
  changes='[{"kind":"create","owned":true,"destructive":false,"resource":{"kind":"authentik.application","id":"canary","attributes":{}}}]'
fi
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":["authentik.application"],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      printf '{"resources":%s,"conditions":[]}\n' "$observed"
      ;;
    *'"op":"plan"'*)
      printf '{"changes":%s,"conditions":[]}\n' "$changes"
      ;;
    *'"op":"apply"'*)
      touch @STATE@
      echo '{"applied":[{"id":"canary","ok":true,"message":""}],"conditions":[]}'
      ;;
  esac
done
"#
        .replace("@STATE@", state.to_str().unwrap()),
    );

    let declaration = Desired {
        resources: vec![DesiredResource {
            kind: "authentik.application".to_string(),
            id: "canary".to_string(),
            attributes: serde_json::json!({}),
        }],
        absent: Vec::new(),
        derive: Vec::new(),
    };

    let first = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &declaration,
        &Ownership::default(),
        &NoopSink,
        ReconcileOptions { apply: true },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("the create runs");
    assert_eq!(first.applied, 1);
    assert!(
        first.ownership.owns("authentik.application", "canary"),
        "a create is recorded as evidence"
    );

    let second = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &declaration,
        &first.ownership,
        &NoopSink,
        ReconcileOptions { apply: false },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("the next run reads the record");
    assert_eq!(second.unowned, 0, "the record owns what it created");
    assert_eq!(second.planned, 0, "a recorded creation is already owned");
    assert!(
        second.ownership.owns("authentik.application", "canary"),
        "the next record still carries the creation"
    );
}

/// Records every persist so a test can read what the run wrote, and when.
#[derive(Default)]
struct RecordingSink {
    writes: std::cell::RefCell<Vec<Ownership>>,
}

impl OwnershipSink for RecordingSink {
    fn persist(&self, record: &Ownership) -> Result<(), NodError> {
        self.writes.borrow_mut().push(record.clone());
        Ok(())
    }
}

/// A provider that plans one create and reports it as applied.
/// A provider that reports nothing and plans nothing.
const IDLE: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":["dns.record"],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      echo '{"changes":[],"conditions":[]}'
      ;;
  esac
done
"#;

const CREATING: &str = r#"
while IFS= read -r line; do
  case "$line" in
    *'"protocol"'*)
      echo '{"provider":"fake","version":"0.1.0","protocol":1,"capabilities":["dns.record"],"ownership":"marker","settings_schema":{}}'
      ;;
    *'"op":"observe"'*)
      echo '{"resources":[],"conditions":[]}'
      ;;
    *'"op":"plan"'*)
      echo '{"changes":[{"kind":"create","owned":true,"destructive":false,"resource":{"kind":"dns.record","id":"a"}}],"conditions":[]}'
      ;;
    *'"op":"apply"'*)
      echo '{"applied":[{"id":"a","ok":true}],"conditions":[]}'
      ;;
  esac
done
"#;

#[tokio::test]
async fn a_create_is_recorded_before_the_provider_performs_it() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(CREATING);

    let sink = RecordingSink::default();
    let report = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &desired(&["a"]),
        &Ownership::default(),
        &sink,
        ReconcileOptions { apply: true },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    let writes = sink.writes.borrow();
    assert_eq!(
        writes.len(),
        2,
        "one write-ahead and one finished record, got {writes:#?}"
    );
    assert_eq!(
        writes[0].pending.len(),
        1,
        "the intent is on disk before the provider is asked"
    );
    assert!(
        writes[0].created.is_empty(),
        "an intent is not yet evidence of ownership"
    );
    assert!(
        writes[1].pending.is_empty(),
        "the finished record carries no open intent"
    );
    assert!(
        writes[1].owns("dns.record", "a"),
        "a successful create is the evidence"
    );
    assert!(report.ownership.owns("dns.record", "a"));
}

#[tokio::test]
async fn an_interrupted_intent_is_settled_by_the_next_observation() {
    let _guard = SERIAL.lock().await;
    let (_dir, path) = script(IDLE);

    // The previous run wrote the intent and died before the provider answered.
    let mut interrupted = Ownership::default();
    interrupted.mark_pending(&[OwnedId::new("dns.record", "a")]);

    let sink = RecordingSink::default();
    let report = execute(
        Path::new(&path),
        &serde_json::json!({}),
        &desired(&[]),
        &interrupted,
        &sink,
        ReconcileOptions { apply: true },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    let writes = sink.writes.borrow();
    // Two writes: the resolved record before the plan runs, and the finished one
    // after it. Nothing was planned, so both carry the same answer.
    assert_eq!(writes.len(), 2, "got {writes:#?}");
    for write in writes.iter() {
        assert!(
            write.pending.is_empty(),
            "an intent the observation does not confirm must be forgotten"
        );
    }
    assert!(
        !report.ownership.owns("dns.record", "a"),
        "the provider reports nothing, so nothing was created"
    );
}
