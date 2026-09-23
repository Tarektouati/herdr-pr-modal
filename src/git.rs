//! Git plumbing. Only read commands, `fetch`, `branch` (create) and a
//! compare-and-swap fast-forward of a branch that no worktree has checked out.
//! Nothing here resets, deletes, forces or touches a working tree.

use crate::cmd::{Output, RunError, Runner};
use crate::provider::FetchPlan;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Short branch name; `None` when detached or bare.
    pub branch: Option<String>,
}

/// Parse `git worktree list --porcelain`.
pub fn parse_worktree_porcelain(text: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.extend(current.take());
            current = Some(Worktree { path: PathBuf::from(path), branch: None });
        } else if let Some(r) = line.strip_prefix("branch ")
            && let Some(wt) = current.as_mut()
        {
            wt.branch = Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
        }
    }
    out.extend(current);
    out
}

/// The worktree that has this PR's branch checked out, if any.
pub fn find_pr_worktree<'a>(
    worktrees: &'a [Worktree],
    plan: &FetchPlan,
    pr_number: u64,
) -> Option<&'a Worktree> {
    worktrees.iter().find(|w| w.branch.as_deref().is_some_and(|b| plan.is_pr_branch(pr_number, b)))
}

fn git(runner: &dyn Runner, cwd: &Path, args: &[&str]) -> Result<Output, String> {
    match runner.run("git", args, Some(cwd)) {
        Ok(out) => Ok(out),
        Err(RunError::NotFound(_)) => Err("git is not installed".into()),
        Err(RunError::Io(e)) => Err(e),
    }
}

fn git_ok(runner: &dyn Runner, cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(runner, cwd, args)?;
    if out.success {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(format!("git {}: {}", args.first().unwrap_or(&""), out.error_line()))
    }
}

/// Top level of the checkout containing `cwd`, or `None` outside a repo.
pub fn repo_root(runner: &dyn Runner, cwd: &Path) -> Result<Option<PathBuf>, String> {
    let out = git(runner, cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(out.success.then(|| PathBuf::from(out.stdout.trim())).filter(|p| !p.as_os_str().is_empty()))
}

pub fn remote_url(runner: &dyn Runner, root: &Path, remote: &str) -> Result<String, String> {
    git_ok(runner, root, &["remote", "get-url", remote])
        .map_err(|_| format!("this repo has no '{remote}' remote"))
}

pub fn worktrees(runner: &dyn Runner, root: &Path) -> Result<Vec<Worktree>, String> {
    git_ok(runner, root, &["worktree", "list", "--porcelain"]).map(|t| parse_worktree_porcelain(&t))
}

fn local_branch_sha(runner: &dyn Runner, root: &Path, branch: &str) -> Result<Option<String>, String> {
    let out =
        git(runner, root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}^{{commit}}")])?;
    Ok(out.success.then(|| out.stdout.trim().to_string()))
}

/// What happened to the local branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchState {
    Created,
    UpToDate,
    FastForwarded,
    /// The local branch has commits the PR head does not; left untouched.
    KeptDiverged,
}

/// Fetch the PR head and make `plan.local_branch` point at it without ever
/// rewriting local work. Callers must first check that no worktree has the
/// branch checked out.
pub fn fetch_into_branch(runner: &dyn Runner, root: &Path, plan: &FetchPlan) -> Result<BranchState, String> {
    let head = match &plan.tracking {
        Some(tracking) => {
            let spec = format!("+{}:{tracking}", plan.source_ref);
            git_ok(runner, root, &["fetch", "--no-tags", &plan.remote, &spec])?;
            git_ok(runner, root, &["rev-parse", "--verify", &format!("{tracking}^{{commit}}")])?
        }
        None => {
            git_ok(runner, root, &["fetch", "--no-tags", &plan.remote, &plan.source_ref])?;
            git_ok(runner, root, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])?
        }
    };

    match local_branch_sha(runner, root, &plan.local_branch)? {
        None => {
            let mut args = vec!["branch"];
            if plan.tracking.is_some() {
                args.push("--track");
            }
            let start = plan.tracking.as_deref().unwrap_or(&head);
            args.extend([plan.local_branch.as_str(), start]);
            git_ok(runner, root, &args)?;
            Ok(BranchState::Created)
        }
        Some(old) if old == head => Ok(BranchState::UpToDate),
        Some(old) => {
            let ff = git(runner, root, &["merge-base", "--is-ancestor", &old, &head])?.success;
            if !ff {
                return Ok(BranchState::KeptDiverged);
            }
            // Compare-and-swap: fails if the branch moved since we read it.
            let reference = format!("refs/heads/{}", plan.local_branch);
            git_ok(
                runner,
                root,
                &["update-ref", "-m", "herdr-pr-modal: fast-forward to PR head", &reference, &head, &old],
            )?;
            Ok(BranchState::FastForwarded)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_with_detached_and_bare() {
        let text = "worktree /repo\nHEAD aaa\nbranch refs/heads/main\n\n\
                    worktree /wt/feat\nHEAD bbb\nbranch refs/heads/feat/x\n\n\
                    worktree /wt/detached\nHEAD ccc\ndetached\n\n\
                    worktree /bare\nbare\n";
        let wts = parse_worktree_porcelain(text);
        assert_eq!(wts.len(), 4);
        assert_eq!(wts[1], Worktree { path: "/wt/feat".into(), branch: Some("feat/x".into()) });
        assert_eq!(wts[2].branch, None);
        assert_eq!(wts[3].branch, None);
    }
}
