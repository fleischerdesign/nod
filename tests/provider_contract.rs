//! Contract tests for the provider seam (ADR-030).
//!
//! These tests run the real supervisor against a shell provider: they prove
//! what the core promises when a provider half-fails or reports outside its
//! declaration. A unit test cannot reach that seam.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nod::application::use_cases::adopt::{execute as adopt, AdoptOptions};
use nod::application::use_cases::reconcile::{execute, rollback, Desired, ReconcileOptions};
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
        ReconcileOptions { apply: false },
        &Policy::default(),
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect_err("an undeclared kind must fail the run");

    assert!(error.to_string().contains("mail.mailbox"), "{error}");
}
