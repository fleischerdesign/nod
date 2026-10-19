//! CLI Git adapter for [`GitPort`] (ADR-001, ADR-028).
//!
//! Every operation shells out to `git` with an explicit `-C <repo>`; none of
//! them can rewrite history. `fast_forward` refuses a checkout that is not on
//! the configured branch and refuses a non-fast-forward integration, so a
//! rewritten upstream history surfaces as an error instead of a silent reset.

use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;

use crate::domain::errors::NodError;
use crate::domain::ports::git::GitPort;

/// Git adapter backed by the `git` executable on `PATH`.
#[derive(Debug, Default, Clone)]
pub struct CliGit;

impl CliGit {
    /// Creates the adapter.
    pub fn new() -> Self {
        Self
    }
}

/// Runs `git -C <repo> <args>` and returns trimmed stdout on success.
async fn git(repo: &Path, args: &[&str]) -> Result<String, NodError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .await
        .map_err(|e| NodError::internal(format!("cannot run git {}: {e}", args.join(" "))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NodError::deployment(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo.display(),
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[async_trait]
impl GitPort for CliGit {
    async fn is_dirty(&self, repo: &Path) -> Result<bool, NodError> {
        let status = git(repo, &["status", "--porcelain", "--untracked-files=no"]).await?;
        Ok(!status.is_empty())
    }

    async fn local_head(&self, repo: &Path) -> Result<String, NodError> {
        git(repo, &["rev-parse", "HEAD"]).await
    }

    async fn remote_head(
        &self,
        repo: &Path,
        remote: &str,
        branch: &str,
    ) -> Result<String, NodError> {
        let reference = format!("refs/heads/{branch}");
        let output = git(repo, &["ls-remote", remote, &reference]).await?;
        let sha = output
            .split_whitespace()
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                NodError::config(format!(
                    "remote '{remote}' has no branch '{branch}' in {}",
                    repo.display()
                ))
            })?;
        Ok(sha.to_string())
    }

    async fn fetch(&self, repo: &Path, remote: &str, branch: &str) -> Result<(), NodError> {
        git(repo, &["fetch", "--prune", remote, branch]).await?;
        Ok(())
    }

    async fn fast_forward(
        &self,
        repo: &Path,
        remote: &str,
        branch: &str,
    ) -> Result<String, NodError> {
        let current = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
        if current != branch {
            return Err(NodError::config(format!(
                "refusing to integrate: {} is on '{current}', not '{branch}'",
                repo.display()
            )));
        }

        let merge_ref = format!("{remote}/{branch}");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["merge", "--ff-only", &merge_ref])
            .output()
            .await
            .map_err(|e| NodError::internal(format!("cannot run git merge: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NodError::deployment(format!(
                "refusing non-fast-forward integration to {merge_ref} in {}: {}",
                repo.display(),
                stderr.trim()
            )));
        }

        git(repo, &["rev-parse", "HEAD"]).await
    }

    async fn is_ancestor(
        &self,
        repo: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, NodError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .output()
            .await
            .map_err(|e| NodError::internal(format!("cannot run git merge-base: {e}")))?;

        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(NodError::deployment(format!(
                "git merge-base --is-ancestor {ancestor} {descendant} failed in {}: {}",
                repo.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }

    async fn verify_signature(&self, repo: &Path, rev: &str) -> Result<bool, NodError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["verify-commit", rev])
            .output()
            .await
            .map_err(|e| NodError::internal(format!("cannot run git verify-commit: {e}")))?;
        Ok(output.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let status = StdCommand::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("a.nix"), "{}\n").unwrap();
        run(&["add", "a.nix"]);
        run(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"]);
        dir
    }

    #[tokio::test]
    async fn dirty_detection_ignores_untracked_files() {
        let dir = git_repo();
        let git = CliGit::new();
        assert!(!git.is_dirty(dir.path()).await.unwrap());

        std::fs::write(dir.path().join("untracked.nix"), "{}\n").unwrap();
        assert!(
            !git.is_dirty(dir.path()).await.unwrap(),
            "untracked files never deploy"
        );

        std::fs::write(dir.path().join("a.nix"), "{ x = 1; }\n").unwrap();
        assert!(git.is_dirty(dir.path()).await.unwrap());
    }

    #[tokio::test]
    async fn ancestry_is_reflexive_and_directional() {
        let dir = git_repo();
        let git = CliGit::new();
        let first = git.local_head(dir.path()).await.unwrap();

        std::fs::write(dir.path().join("b.nix"), "{}\n").unwrap();
        let status = StdCommand::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "b.nix"])
            .status()
            .unwrap();
        assert!(status.success());
        let status = StdCommand::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", "second"])
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .status()
            .unwrap();
        assert!(status.success());
        let second = git.local_head(dir.path()).await.unwrap();

        assert!(git.is_ancestor(dir.path(), &first, &second).await.unwrap());
        assert!(!git.is_ancestor(dir.path(), &second, &first).await.unwrap());
        assert!(git.is_ancestor(dir.path(), &first, &first).await.unwrap());
    }

    #[tokio::test]
    async fn fast_forward_refuses_a_different_checkout() {
        let dir = git_repo();
        let git = CliGit::new();
        let status = StdCommand::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["checkout", "-q", "-b", "other"])
            .status()
            .unwrap();
        assert!(status.success());

        let error = git
            .fast_forward(dir.path(), "origin", "main")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not 'main'"), "got: {error}");
    }
}
