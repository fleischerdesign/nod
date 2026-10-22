//! The example provider is the documentation.
//!
//! If this test passes, a provider can be written and verified without reading
//! anything else: the protocol is complete enough to run a full cycle.

use std::path::PathBuf;
use std::time::Duration;

use nod::domain::provider::{DesiredResource, OwnershipMode};
use nod::infrastructure::provider::process::ProcessProvider;

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("providers/example/nod-provider-example.py")
}

#[tokio::test]
async fn the_example_provider_runs_a_full_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let settings = serde_json::json!({ "state": dir.path().join("state.json") });

    let (mut provider, handshake) = ProcessProvider::open(&example(), Duration::from_secs(10))
        .await
        .expect("the example provider must complete the handshake");
    assert_eq!(handshake.provider, "example");
    assert_eq!(handshake.ownership, OwnershipMode::Marker);

    let observed = provider.observe(&settings).await.unwrap();
    assert!(observed.resources.is_empty());

    let desired = vec![DesiredResource {
        kind: "example.record".to_string(),
        id: "one".to_string(),
        attributes: serde_json::json!({ "value": "1" }),
    }];
    let plan = provider
        .plan(&settings, &desired, &observed.resources)
        .await
        .unwrap();
    assert_eq!(plan.changes.len(), 1);
    assert!(plan.changes[0].owned);
    assert!(!plan.changes[0].destructive);

    let applied = provider.apply(&settings, &plan.changes).await.unwrap();
    assert_eq!(applied.applied.len(), 1);
    assert!(applied.applied[0].ok);

    let after = provider.observe(&settings).await.unwrap();
    assert_eq!(after.resources.len(), 1);
    assert!(after.resources[0].owned);
}

#[tokio::test]
async fn a_missing_setting_is_an_error_not_an_empty_world() {
    let (mut provider, _) = ProcessProvider::open(&example(), Duration::from_secs(10))
        .await
        .unwrap();

    let error = provider
        .observe(&serde_json::json!({}))
        .await
        .expect_err("an empty config must be reported, not read as an empty world");
    assert!(error.to_string().contains("config.state"), "got: {error}");
}
