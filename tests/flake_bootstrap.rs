//! Regression tests for the ancestor `.nod.toml` discovery bootstrap.
//!
//! These reproduce the production call shape — `effective_flake(_, Path::new("."))`
//! — so the ancestor walk must climb real directories above the invocation
//! cwd. The in-crate unit tests cannot stage this because they would have to
//! mutate the shared process cwd; each integration-test binary runs in its own
//! process and this file holds a single test, so `chdir` is safe here.

use nod::infrastructure::config::toml_config::effective_flake;
use std::path::Path;

#[test]
fn relative_dot_search_start_finds_config_in_ancestor_directory() {
    // Layout: <tmp>/root/.nod.toml carrying [defaults].flake, with the
    // invocation cwd pointed at <tmp>/root/deep/nested — a directory UNDER the
    // config file — exactly like `nod <cmd>` run without --flake from a
    // checkout subdirectory.
    let original = std::env::current_dir().unwrap();
    let dir = tempfile::Builder::new()
        .prefix("nod-flake-ancestor-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    std::fs::write(
        root.join(".nod.toml"),
        "[defaults]\nflake = \"flakes/web-01\"\n",
    )
    .unwrap();

    let nested = root.join("deep").join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::env::set_current_dir(&nested).unwrap();

    // A bare `.` search start: per std semantics `Path::new(".").parent()`
    // returns `Some("")` and `Path::new("").parent()` returns `None`, so the
    // pre-fix walk checked only the cwd and the ancestor config was never
    // found (the run resolved to `.` instead).
    let flake_path = effective_flake(Path::new("."), Path::new(".")).unwrap();

    std::env::set_current_dir(&original).unwrap();

    assert_eq!(flake_path, root.join("flakes/web-01"));
}
