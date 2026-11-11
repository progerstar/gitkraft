//! Branch operations — list, create, delete, checkout, and merge branches.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use git2::{BranchType as Git2BranchType, Repository};
use tracing::debug;

use super::types::{BranchInfo, BranchType};
use crate::utils::process::output_with_timeout;

/// List all branches (local and remote) in the repository.
pub fn list_branches(repo: &Repository) -> Result<Vec<BranchInfo>> {
    let mut branches = Vec::new();

    let head_ref = repo.head().ok();
    let head_name = head_ref
        .as_ref()
        .and_then(|r| r.shorthand().ok().map(String::from));

    for branch_result in repo.branches(None)? {
        let (branch, bt) = branch_result?;

        let name = branch.name()?.unwrap_or("<invalid utf-8>").to_string();

        let branch_type = match bt {
            Git2BranchType::Local => BranchType::Local,
            Git2BranchType::Remote => BranchType::Remote,
        };

        let is_head = match branch_type {
            BranchType::Local => head_name.as_deref() == Some(name.as_str()),
            BranchType::Remote => false,
        };

        let target_oid = branch.get().target().map(|oid| oid.to_string());

        // Compute ahead/behind upstream for local branches.
        let (upstream_ahead, upstream_behind) = if branch_type == BranchType::Local {
            if let Some(ref local_oid_str) = target_oid {
                if let Ok(local_oid) = git2::Oid::from_str(local_oid_str) {
                    branch
                        .upstream()
                        .ok()
                        .and_then(|up| up.get().target())
                        .and_then(|up_oid| repo.graph_ahead_behind(local_oid, up_oid).ok())
                        .map(|(a, b)| (Some(a), Some(b)))
                        .unwrap_or((None, None))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        branches.push(BranchInfo {
            name,
            branch_type,
            is_head,
            target_oid,
            upstream_ahead,
            upstream_behind,
        });
    }

    debug!("listed {} branches", branches.len());
    Ok(branches)
}

/// Create a new local branch at HEAD with the given name.
///
/// Returns the newly created [`BranchInfo`].
pub fn create_branch(repo: &Repository, name: &str) -> Result<BranchInfo> {
    let head_ref = repo
        .head()
        .context("HEAD not found — is this an empty repository?")?;
    let commit = head_ref
        .peel_to_commit()
        .context("HEAD does not point to a commit")?;

    let branch = repo
        .branch(name, &commit, false)
        .with_context(|| format!("failed to create branch '{name}'"))?;

    let target_oid = branch.get().target().map(|oid| oid.to_string());

    debug!(name, "created branch");
    Ok(BranchInfo {
        name: name.to_string(),
        branch_type: BranchType::Local,
        is_head: false,
        target_oid,
        upstream_ahead: None,
        upstream_behind: None,
    })
}

/// Delete a local branch by name.
///
/// Refuses to delete the currently checked-out branch.
pub fn delete_branch(repo: &Repository, name: &str) -> Result<()> {
    let mut branch = repo
        .find_branch(name, Git2BranchType::Local)
        .with_context(|| format!("local branch '{name}' not found"))?;

    if branch.is_head() {
        bail!("cannot delete the currently checked-out branch '{name}'");
    }

    branch
        .delete()
        .with_context(|| format!("failed to delete branch '{name}'"))?;
    debug!(name, "deleted branch");
    Ok(())
}

/// Checkout an existing local branch by name.
///
/// Sets HEAD to the branch reference and updates the working directory.
pub fn checkout_branch(repo: &Repository, name: &str) -> Result<()> {
    let refname = format!("refs/heads/{name}");

    // Make sure the branch actually exists
    repo.find_branch(name, Git2BranchType::Local)
        .with_context(|| format!("local branch '{name}' not found"))?;

    repo.set_head(&refname)
        .with_context(|| format!("failed to set HEAD to '{refname}'"))?;

    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .with_context(|| format!("failed to checkout branch '{name}'"))?;

    debug!(name, "checked out branch");
    Ok(())
}

/// Merge a source branch into the current HEAD branch.
///
/// If the merge can be fast-forwarded, it does so. If it results in a normal
/// merge (no conflicts), an automatic merge commit is created. If there are
/// conflicts, an error is returned and the repository is left in a merging
/// state so the user can resolve conflicts manually.
pub fn merge_branch(repo: &Repository, source_branch: &str) -> Result<()> {
    // Look up the source branch reference and its annotated commit.
    let branch = repo
        .find_branch(source_branch, Git2BranchType::Local)
        .with_context(|| format!("local branch '{source_branch}' not found"))?;

    let source_ref = branch.get();
    let source_oid = source_ref
        .target()
        .with_context(|| format!("branch '{source_branch}' has no target OID"))?;

    let annotated_commit = repo
        .find_annotated_commit(source_oid)
        .context("failed to find annotated commit for source branch")?;

    // Perform merge analysis.
    let (analysis, _preference) = repo
        .merge_analysis(&[&annotated_commit])
        .context("merge analysis failed")?;

    if analysis.is_up_to_date() {
        debug!(source_branch, "already up to date");
        return Ok(());
    }

    if analysis.is_fast_forward() {
        debug!(source_branch, "fast-forwarding");
        // Fast-forward: just move the current branch reference.
        let refname = format!("refs/heads/{}", head_branch_name(repo)?);
        let msg = format!("Fast-forward merge of '{source_branch}'");
        repo.reference(&refname, source_oid, true, &msg)?;
        repo.set_head(&refname)?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
        return Ok(());
    }

    if analysis.is_normal() {
        debug!(source_branch, "performing normal merge");

        // Perform the actual merge (writes conflicts to index if any).
        repo.merge(&[&annotated_commit], None, None)
            .context("merge failed")?;

        // Check for conflicts.
        let index = repo.index().context("failed to read index after merge")?;
        if index.has_conflicts() {
            bail!(
                "merge of '{source_branch}' resulted in conflicts — resolve them and commit manually"
            );
        }

        // No conflicts — create the merge commit automatically.
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("GitKraft User", "user@gitkraft.local"))
            .context("failed to obtain signature")?;

        let mut index = repo.index().context("failed to read index")?;
        let tree_oid = index.write_tree().context("failed to write merged tree")?;
        let tree = repo
            .find_tree(tree_oid)
            .context("failed to find merged tree")?;

        let head_commit = repo
            .head()?
            .peel_to_commit()
            .context("HEAD does not point to a commit")?;

        let source_commit = repo
            .find_commit(source_oid)
            .context("failed to find source commit")?;

        let message = format!("Merge branch '{source_branch}'");
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &message,
            &tree,
            &[&head_commit, &source_commit],
        )
        .context("failed to create merge commit")?;

        // Clean up merge state.
        repo.cleanup_state()
            .context("failed to clean up merge state")?;

        debug!(source_branch, "merge commit created");
        return Ok(());
    }

    bail!("merge analysis returned an unexpected result for branch '{source_branch}'");
}

/// Helper: get the short name of the branch HEAD points to.
fn head_branch_name(repo: &Repository) -> Result<String> {
    let head = repo.head().context("HEAD not found")?;
    let name = head
        .shorthand()
        .context("HEAD is not a symbolic reference (detached HEAD?)")?
        .to_string();
    Ok(name)
}

// ── subprocess helper ─────────────────────────────────────────────────────────

fn run_git(workdir: &std::path::Path, args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new("git")
        .current_dir(workdir)
        .args(args)
        .output()
        .context("failed to spawn git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{}", stderr.trim());
    }
    Ok(())
}

// ── new public functions ──────────────────────────────────────────────────────

/// Rename a local branch.
pub fn rename_branch(repo: &Repository, old_name: &str, new_name: &str) -> Result<()> {
    let mut branch = repo
        .find_branch(old_name, Git2BranchType::Local)
        .with_context(|| format!("branch '{old_name}' not found"))?;
    branch
        .rename(new_name, false)
        .with_context(|| format!("failed to rename '{old_name}' → '{new_name}'"))?;
    debug!(old_name, new_name, "renamed branch");
    Ok(())
}

/// Create a new local branch pointing at a specific commit OID.
pub fn create_branch_at_commit(repo: &Repository, name: &str, oid_str: &str) -> Result<()> {
    let oid = git2::Oid::from_str(oid_str).context("invalid OID")?;
    let commit = repo
        .find_commit(oid)
        .with_context(|| format!("commit {oid_str} not found"))?;
    repo.branch(name, &commit, false)
        .with_context(|| format!("failed to create branch '{name}' at {oid_str}"))?;
    debug!(name, oid_str, "created branch at commit");
    Ok(())
}

/// Push a local branch to a remote using `git push`.
///
/// Uses the system `git` binary so that the user's configured credential
/// helpers (SSH agent, git-credential-manager, etc.) are respected.
pub fn push_branch(workdir: &std::path::Path, branch: &str, remote: &str) -> Result<()> {
    run_git_remote(workdir, &["push", remote, branch], StartedBy::User)
}

pub fn force_push_branch(workdir: &std::path::Path, branch: &str, remote: &str) -> Result<()> {
    run_git_remote(
        workdir,
        &["push", "--force-with-lease", remote, branch],
        StartedBy::User,
    )
}

/// Delete a remote branch using `git push <remote> --delete <branch>`.
///
/// `full_name` is the remote-tracking branch name (e.g. `origin/feature-x`).
/// The function extracts the remote and branch parts automatically.
pub fn delete_remote_branch(workdir: &std::path::Path, full_name: &str) -> Result<()> {
    let (remote, branch) = full_name.split_once('/').with_context(|| {
        format!("invalid remote branch name '{full_name}' — expected 'remote/branch'")
    })?;
    run_git_remote(
        workdir,
        &["push", remote, "--delete", branch],
        StartedBy::User,
    )
}

/// Checkout a remote branch by creating a local tracking branch.
///
/// `full_name` is the remote-tracking branch name (e.g. `origin/feature-x`).
/// Creates a local branch named `feature-x` that tracks `origin/feature-x`.
pub fn checkout_remote_branch(workdir: &std::path::Path, full_name: &str) -> Result<()> {
    let (_remote, branch) = full_name.split_once('/').with_context(|| {
        format!("invalid remote branch name '{full_name}' — expected 'remote/branch'")
    })?;
    run_git(workdir, &["checkout", "-b", branch, "--track", full_name])
}

/// Pull the current branch from a remote with `--rebase`.
pub fn pull_rebase(workdir: &std::path::Path, remote: &str) -> Result<()> {
    run_git_remote(workdir, &["pull", "--rebase", remote], StartedBy::User)
}

/// Background `git fetch --all --prune` that never asks for credentials:
/// without a credential helper or SSH agent it simply fails.
pub fn fetch_all_quiet(workdir: &std::path::Path) -> Result<()> {
    run_git_remote(
        workdir,
        &["fetch", "--all", "--prune", "--quiet"],
        StartedBy::Background,
    )
}

/// `git fetch <remote>` with the system git, so credential helpers (e.g.
/// `gh auth git-credential`) and the SSH agent work — unlike libgit2's fetch.
/// Never asks in the terminal: there is nobody to answer there.
pub fn fetch_remote_cli(workdir: &std::path::Path, remote: &str) -> Result<()> {
    run_git_remote(workdir, &["fetch", "--prune", remote], StartedBy::User)
}

/// Pull the current branch from its upstream like GitHub Desktop: merge the
/// remote changes in (a merge commit when histories diverged), unless the
/// user's git config sets `pull.rebase`, which is then followed.
pub fn pull_merge(workdir: &std::path::Path) -> Result<()> {
    let rebase_configured = std::process::Command::new("git")
        .current_dir(workdir)
        .args(["config", "--get", "pull.rebase"])
        .output()
        .is_ok_and(|o| o.status.success());
    if rebase_configured {
        run_git_remote(workdir, &["pull"], StartedBy::User)
    } else {
        run_git_remote(workdir, &["pull", "--no-rebase"], StartedBy::User)
    }
}

/// Merge `refname` (e.g. a remote branch like `origin/main`) into the current
/// branch with the system git; the default merge message is used.
pub fn merge_ref_cli(workdir: &std::path::Path, refname: &str) -> Result<()> {
    run_git_no_prompt(workdir, &["merge", "--no-edit", refname])
}

/// Run git without any interaction: no credential prompts, no editor for
/// merge messages. On failure the error carries git's output.
fn run_git_no_prompt(workdir: &std::path::Path, args: &[&str]) -> Result<()> {
    let output = git_command(workdir, args)
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .output()
        .context("failed to spawn git")?;
    check_output(&output)
}

/// `git <args>` in `workdir` that never waits for an answer in the terminal
/// (the TUI owns it, the GUI's is out of sight) and never opens an editor.
fn git_command(workdir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(workdir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_EDITOR", "true")
        .stdin(Stdio::null());
    cmd
}

/// `Ok` when git succeeded, otherwise an error with git's output (conflict
/// notices go to stdout, the rest to stderr).
fn check_output(output: &Output) -> Result<()> {
    if !output.status.success() {
        let text = [output.stdout.as_slice(), output.stderr.as_slice()]
            .iter()
            .map(|b| String::from_utf8_lossy(b).trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!("{text}");
    }
    Ok(())
}

// ── network commands ──────────────────────────────────────────────────────────

/// Who started a network command: decides what may ask for a password and
/// how long the command may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartedBy {
    /// A user action. A graphical askpass (`SSH_ASKPASS`, `core.askPass`,
    /// git-credential-manager) may ask, as when git runs without a terminal;
    /// the terminal does not (unless the user's own ssh command asks there).
    User,
    /// A background fetch: nothing may ask, it simply fails.
    Background,
}

impl StartedBy {
    /// Longest the command may run. A dead connection is dropped much sooner
    /// (see [`remote_command`]); this catches what slips through, such as a
    /// custom `core.sshCommand` that waits forever.
    fn timeout(self) -> Duration {
        match self {
            Self::User => Duration::from_secs(10 * 60),
            Self::Background => Duration::from_secs(2 * 60),
        }
    }
}

/// Run a git command that talks to a remote. Commands on the same repository
/// take turns: concurrent fetches and pushes fail on each other's ref locks.
fn run_git_remote(workdir: &Path, args: &[&str], by: StartedBy) -> Result<()> {
    let mut cmd = remote_command(workdir, args, by);
    let _turn = RepoTurn::take(workdir);
    let timeout = by.timeout();
    let output = output_with_timeout(&mut cmd, timeout)
        .context("failed to spawn git")?
        .with_context(|| {
            format!(
                "git {} did not finish within {} min and was stopped",
                args[0],
                timeout.as_secs() / 60
            )
        })?;
    check_output(&output)
}

/// The [`git_command`] for a network command started `by` someone.
fn remote_command(workdir: &Path, args: &[&str], by: StartedBy) -> Command {
    let mut cmd = git_command(workdir, args);
    // An HTTP(S) transfer that moved nothing for a minute is dead: git
    // servers send keep-alives while they work.
    for (var, value) in [
        ("GIT_HTTP_LOW_SPEED_LIMIT", "1"),
        ("GIT_HTTP_LOW_SPEED_TIME", "60"),
    ] {
        if std::env::var_os(var).is_none() {
            cmd.env(var, value);
        }
    }
    // ssh asks in the terminal when there is one; the askpass dialog instead.
    let askpass = by == StartedBy::User && std::env::var_os("SSH_ASKPASS").is_some();
    if askpass {
        cmd.env("SSH_ASKPASS_REQUIRE", "force");
    }
    if by == StartedBy::Background {
        // An empty GIT_ASKPASS also keeps git from falling back to SSH_ASKPASS.
        cmd.env("GIT_ASKPASS", "")
            .env("GCM_INTERACTIVE", "never")
            .env("SSH_ASKPASS_REQUIRE", "never");
    }
    if let Some(ssh) = ssh_command(workdir, !askpass) {
        cmd.env("GIT_SSH_COMMAND", ssh);
    }
    cmd
}

/// `GIT_SSH_COMMAND` that gives up on a server that does not answer or
/// stopped answering, and with `batch` never asks anything. `None` on Windows
/// (Git for Windows brings its own ssh) and when the user set up their own
/// ssh command, which is then left alone.
fn ssh_command(workdir: &Path, batch: bool) -> Option<String> {
    let own_env = ["GIT_SSH_COMMAND", "GIT_SSH", "GIT_SSH_VARIANT"]
        .iter()
        .any(|var| std::env::var_os(var).is_some());
    if cfg!(windows) || own_env {
        return None;
    }
    let own_config = Command::new("git")
        .current_dir(workdir)
        .args([
            "config",
            "--get-regexp",
            r"^(core\.sshcommand|ssh\.variant)$",
        ])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());
    if own_config {
        return None;
    }
    let mut ssh =
        String::from("ssh -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4");
    if batch {
        ssh.push_str(" -o BatchMode=yes");
    }
    Some(ssh)
}

/// Repositories (canonical workdirs) with a network command running.
static BUSY_REPOS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// Signalled whenever a repository leaves [`BUSY_REPOS`].
static REPO_FREED: Condvar = Condvar::new();

/// A repository's turn to run a network command, held until dropped.
struct RepoTurn(PathBuf);

impl RepoTurn {
    /// Wait until no other network command runs in `workdir`, then take the turn.
    fn take(workdir: &Path) -> Self {
        let repo = workdir
            .canonicalize()
            .unwrap_or_else(|_| workdir.to_path_buf());
        let mut busy = BUSY_REPOS.lock().unwrap_or_else(PoisonError::into_inner);
        while busy.contains(&repo) {
            busy = REPO_FREED
                .wait(busy)
                .unwrap_or_else(PoisonError::into_inner);
        }
        busy.push(repo.clone());
        Self(repo)
    }
}

impl Drop for RepoTurn {
    fn drop(&mut self) {
        BUSY_REPOS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|repo| *repo != self.0);
        REPO_FREED.notify_all();
    }
}

/// Rebase the current HEAD onto `target` (branch name or OID).
pub fn rebase_onto(workdir: &std::path::Path, target: &str) -> Result<()> {
    run_git(workdir, &["rebase", target])
}

/// Create a lightweight Git tag pointing at the given OID.
pub fn create_tag(repo: &Repository, name: &str, oid_str: &str) -> Result<()> {
    let oid = git2::Oid::from_str(oid_str).context("invalid OID")?;
    let object = repo
        .find_object(oid, None)
        .with_context(|| format!("object {oid_str} not found"))?;
    repo.tag_lightweight(name, &object, false)
        .with_context(|| format!("failed to create lightweight tag '{name}'"))?;
    debug!(name, oid_str, "created lightweight tag");
    Ok(())
}

/// Create an annotated Git tag with a tagger signature and message pointing at the given OID.
pub fn create_annotated_tag(
    repo: &Repository,
    name: &str,
    message: &str,
    oid_str: &str,
) -> Result<()> {
    let oid = git2::Oid::from_str(oid_str).context("invalid OID")?;
    let object = repo
        .find_object(oid, None)
        .with_context(|| format!("object {oid_str} not found"))?;
    let sig = repo
        .signature()
        .or_else(|_| git2::Signature::now("GitKraft User", "user@gitkraft.local"))
        .context("failed to obtain signature")?;
    repo.tag(name, &object, &sig, message, false)
        .with_context(|| format!("failed to create annotated tag '{name}'"))?;
    debug!(name, oid_str, "created annotated tag");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_repo_with_commit() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        std::fs::write(dir.path().join("file.txt"), "hello\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_oid).unwrap();
            let sig = repo.signature().unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }
        (dir, repo)
    }

    #[test]
    fn list_branches_shows_main() {
        let (_dir, repo) = setup_repo_with_commit();
        let branches = list_branches(&repo).unwrap();
        assert!(!branches.is_empty());
        assert!(branches.iter().any(|b| b.is_head));
    }

    #[test]
    fn create_and_delete_branch() {
        let (_dir, repo) = setup_repo_with_commit();
        let branch = create_branch(&repo, "feature-test").unwrap();
        assert_eq!(branch.name, "feature-test");
        assert!(!branch.is_head);

        delete_branch(&repo, "feature-test").unwrap();
        let branches = list_branches(&repo).unwrap();
        assert!(!branches.iter().any(|b| b.name == "feature-test"));
    }

    #[test]
    fn checkout_branch_switches_head() {
        let (_dir, repo) = setup_repo_with_commit();
        create_branch(&repo, "new-branch").unwrap();
        checkout_branch(&repo, "new-branch").unwrap();
        let branches = list_branches(&repo).unwrap();
        let head = branches.iter().find(|b| b.is_head).unwrap();
        assert_eq!(head.name, "new-branch");
    }

    #[test]
    fn delete_head_branch_fails() {
        let (_dir, repo) = setup_repo_with_commit();
        let branches = list_branches(&repo).unwrap();
        let head = branches.iter().find(|b| b.is_head).unwrap();
        let result = delete_branch(&repo, &head.name);
        assert!(result.is_err());
    }

    #[test]
    fn rename_branch_works() {
        let (_dir, repo) = setup_repo_with_commit();
        create_branch(&repo, "test-branch").unwrap();
        rename_branch(&repo, "test-branch", "renamed-branch").unwrap();
        let branches = list_branches(&repo).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"renamed-branch"));
        assert!(!names.contains(&"test-branch"));
    }

    #[test]
    fn rename_nonexistent_branch_fails() {
        let (_dir, repo) = setup_repo_with_commit();
        let result = rename_branch(&repo, "no-such-branch", "new-name");
        assert!(result.is_err());
    }

    #[test]
    fn create_branch_at_commit_works() {
        let (_dir, repo) = setup_repo_with_commit();
        let head_oid = repo.head().unwrap().target().unwrap().to_string();
        create_branch_at_commit(&repo, "feature-x", &head_oid).unwrap();
        let branches = list_branches(&repo).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"feature-x"));
    }

    #[test]
    fn create_branch_at_commit_invalid_oid_fails() {
        let (_dir, repo) = setup_repo_with_commit();
        let result = create_branch_at_commit(&repo, "bad", "not-a-valid-oid");
        assert!(result.is_err());
    }

    #[test]
    fn create_tag_works() {
        let (_dir, repo) = setup_repo_with_commit();
        let head_oid = repo.head().unwrap().target().unwrap().to_string();
        create_tag(&repo, "v1.0.0", &head_oid).unwrap();
        let tag = repo.find_reference("refs/tags/v1.0.0");
        assert!(tag.is_ok());
    }

    #[test]
    fn create_annotated_tag_works() {
        let (_dir, repo) = setup_repo_with_commit();
        let head_oid = repo.head().unwrap().target().unwrap().to_string();
        create_annotated_tag(&repo, "v2.0.0", "Release 2.0", &head_oid).unwrap();
        let tag = repo.find_reference("refs/tags/v2.0.0");
        assert!(tag.is_ok());
    }

    #[test]
    fn create_tag_duplicate_fails() {
        let (_dir, repo) = setup_repo_with_commit();
        let head_oid = repo.head().unwrap().target().unwrap().to_string();
        create_tag(&repo, "v1.0.0", &head_oid).unwrap();
        let result = create_tag(&repo, "v1.0.0", &head_oid);
        assert!(result.is_err());
    }

    #[test]
    fn merge_branch_fast_forward() {
        let (dir, repo) = setup_repo_with_commit();
        // Create a feature branch, switch to it, add a commit
        create_branch(&repo, "feature").unwrap();
        checkout_branch(&repo, "feature").unwrap();

        // Add a second commit on the feature branch
        std::fs::write(dir.path().join("feature.txt"), "feature work\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("feature.txt")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "feature commit",
            &tree,
            &[&head_commit],
        )
        .unwrap();

        let feature_oid = repo.head().unwrap().target().unwrap();

        // Switch back to master and merge — should fast-forward
        checkout_branch(&repo, "master").unwrap();
        merge_branch(&repo, "feature").unwrap();

        // master should now point at the same commit as feature
        let master_oid = repo.head().unwrap().target().unwrap();
        assert_eq!(master_oid, feature_oid);
    }

    #[test]
    fn merge_branch_already_up_to_date() {
        let (_dir, repo) = setup_repo_with_commit();
        create_branch(&repo, "same").unwrap();
        // Merging a branch that points at the same commit should be a no-op
        let result = merge_branch(&repo, "same");
        assert!(result.is_ok());
    }

    #[test]
    fn fetch_all_quiet_updates_behind_count() {
        let tmp = tempfile::tempdir().unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .current_dir(dir)
                .args([
                    "-c",
                    "user.name=T",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=main",
                ])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        let root = tmp.path();
        git(root, &["init", "-q", "--bare", "origin.git"]);
        git(root, &["clone", "-q", "origin.git", "me"]);
        git(root, &["clone", "-q", "origin.git", "other"]);
        let (me, other) = (root.join("me"), root.join("other"));
        std::fs::write(me.join("a.txt"), "1").unwrap();
        git(&me, &["add", "."]);
        git(&me, &["commit", "-qm", "one"]);
        git(&me, &["push", "-q", "-u", "origin", "HEAD:main"]);
        git(&other, &["pull", "-q", "origin", "main"]);
        std::fs::write(other.join("b.txt"), "2").unwrap();
        git(&other, &["add", "."]);
        git(&other, &["commit", "-qm", "two"]);
        git(&other, &["push", "-q", "origin", "HEAD:main"]);

        fetch_remote_cli(&me, "origin").unwrap();
        // A real git error comes back as the message.
        let err = fetch_remote_cli(&me, "nope").unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
        fetch_all_quiet(&me).unwrap();

        let repo = Repository::open(&me).unwrap();
        let branches = list_branches(&repo).unwrap();
        let head = branches.iter().find(|b| b.is_head).unwrap();
        assert_eq!(head.upstream_behind, Some(1));
        assert_eq!(head.upstream_ahead, Some(0));
    }

    /// `origin.git` with a `me` clone that is 1 ahead and 1 behind.
    fn diverged_clone(root: &std::path::Path, conflict: bool) -> std::path::PathBuf {
        let git = |dir: &std::path::Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args([
                    "-c",
                    "user.name=T",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=main",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(root, &["init", "-q", "--bare", "origin.git"]);
        git(root, &["clone", "-q", "origin.git", "me"]);
        let me = root.join("me");
        std::fs::write(me.join("a.txt"), "base\n").unwrap();
        git(&me, &["add", "."]);
        git(&me, &["commit", "-qm", "base"]);
        git(&me, &["push", "-q", "-u", "origin", "HEAD:main"]);
        git(root, &["clone", "-q", "-b", "main", "origin.git", "other"]);
        let other = root.join("other");
        let theirs = if conflict { "a.txt" } else { "b.txt" };
        std::fs::write(other.join(theirs), "theirs\n").unwrap();
        git(&other, &["add", "."]);
        git(&other, &["commit", "-qm", "theirs"]);
        git(&other, &["push", "-q", "origin", "HEAD:main"]);
        std::fs::write(me.join("a.txt"), "mine\n").unwrap();
        git(&me, &["commit", "-qam", "mine"]);
        // Identity for the merge commits the tests create.
        git(&me, &["config", "user.name", "T"]);
        git(&me, &["config", "user.email", "t@t"]);
        me
    }

    #[test]
    fn pull_merge_creates_a_merge_commit_on_diverged_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let me = diverged_clone(tmp.path(), false);
        pull_merge(&me).unwrap();
        let repo = Repository::open(&me).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_count(), 2, "expected a merge commit");
        assert!(me.join("b.txt").exists());
        let branches = list_branches(&repo).unwrap();
        let main = branches.iter().find(|b| b.is_head).unwrap();
        assert_eq!(main.upstream_behind, Some(0));
    }

    #[test]
    fn merge_ref_cli_merges_a_remote_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let me = diverged_clone(tmp.path(), false);
        fetch_remote_cli(&me, "origin").unwrap();
        merge_ref_cli(&me, "origin/main").unwrap();
        assert!(me.join("b.txt").exists());
    }

    #[test]
    fn pull_merge_reports_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let me = diverged_clone(tmp.path(), true);
        let err = pull_merge(&me).unwrap_err().to_string();
        // git's own text (localised) names the conflicting file.
        assert!(err.contains("a.txt"), "{err}");
        let repo = Repository::open(&me).unwrap();
        assert!(repo.index().unwrap().has_conflicts());
    }

    #[test]
    fn network_commands_on_one_repository_take_turns() {
        let repo = tempfile::tempdir().unwrap();
        let first = RepoTurn::take(repo.path());
        let (tx, rx) = std::sync::mpsc::channel();
        let path = repo.path().to_path_buf();
        let second = std::thread::spawn(move || {
            let _turn = RepoTurn::take(&path);
            tx.send(()).unwrap();
        });
        // The second command waits while the first one runs…
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        // …while another repository does not wait.
        let other = tempfile::tempdir().unwrap();
        drop(RepoTurn::take(other.path()));
        drop(first);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        second.join().unwrap();
    }

    #[test]
    fn network_commands_never_ask_in_the_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let var = |by, name: &str| {
            remote_command(dir.path(), &["fetch"], by)
                .get_envs()
                .find(|(k, _)| *k == name)
                .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        for by in [StartedBy::User, StartedBy::Background] {
            assert_eq!(var(by, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        }
        // A user action may still use a graphical askpass; a background fetch may not.
        assert_eq!(var(StartedBy::User, "GIT_ASKPASS"), None);
        assert_eq!(
            var(StartedBy::Background, "GIT_ASKPASS").as_deref(),
            Some("")
        );
        let gcm = var(StartedBy::Background, "GCM_INTERACTIVE");
        assert_eq!(gcm.as_deref(), Some("never"));
    }
}
