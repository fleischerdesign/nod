//! Contract tests for the provider seam (ADR-030).
//!
//! These tests run the real supervisor against a shell provider: they prove
//! what the core promises when a provider half-fails or reports outside its
//! declaration. A unit test cannot reach that seam.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nod::application::use_cases::reconcile::{execute, ReconcileOptions};
use nod::domain::policy::Policy;
use nod::domain::provider::DesiredResource;

/// These tests write an executable and spawn it. Run in parallel, one test's
/// `fork` can inherit another's still-open write descriptor and make the
/// subsequent `exec` fail with ETXTBSY. The supervisor is not at fault — a
/// store path is immutable — so the harness serialises instead of retrying.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake-provider.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    (dir, path)
}

fn desired(ids: &[&str]) -> Vec<DesiredResource> {
    ids.iter()
        .map(|id| DesiredResource {
            kind: "dns.record".to_string(),
            id: (*id).to_string(),
            attributes: serde_json::json!({ "content": "192.0.2.1" }),
        })
        .collect()
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
