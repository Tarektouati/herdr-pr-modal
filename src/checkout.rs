//! Select a PR → switch to its worktree workspace, or fetch it and create one.

use crate::cmd::Runner;
use crate::config::Config;
use crate::git::{self, BranchState};
use crate::herdr;
use crate::model::{Pr, slug};
use crate::provider::FetchPlan;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// A worktree already had the branch; Herdr focused its workspace.
    Switched { path: PathBuf },
    /// A new worktree workspace was created and focused.
    Created { branch: String, state: BranchState },
}

/// Worktree path when `worktree_dir` is configured, mirroring Herdr's
/// `<dir>/<repo>/<branch-slug>` layout. `None` lets Herdr pick.
pub fn configured_path(cfg: &Config, repo_name: &str, branch: &str, home: Option<&str>) -> Option<PathBuf> {
    let dir = cfg.worktree_dir.as_deref()?;
    let dir = match (dir.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => Path::new(home).join(rest),
        _ if dir == "~" => PathBuf::from(home?),
        _ => PathBuf::from(dir),
    };
    Some(dir.join(repo_name).join(slug(branch, 80)))
}

pub fn workspace_label(pr: &Pr) -> String {
    let title: String = pr.title.chars().take(40).collect();
    let ellipsis = if pr.title.chars().count() > 40 { "…" } else { "" };
    format!("#{} {title}{ellipsis}", pr.number)
}

pub struct Request<'a> {
    pub runner: &'a dyn Runner,
    pub herdr_bin: &'a str,
    pub repo_root: &'a Path,
    pub cfg: &'a Config,
    pub pr: &'a Pr,
    pub plan: &'a FetchPlan,
}

pub fn open_pr(req: &Request) -> Result<Opened, String> {
    let worktrees = git::worktrees(req.runner, req.repo_root)?;
    if let Some(existing) = git::find_pr_worktree(&worktrees, req.plan, req.pr.number) {
        let args = herdr::worktree_open_args(req.repo_root, &existing.path);
        herdr::run(req.runner, req.herdr_bin, &args)?;
        return Ok(Opened::Switched { path: existing.path.clone() });
    }

    let state = git::fetch_into_branch(req.runner, req.repo_root, req.plan)?;
    let repo_name = req.repo_root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let home = std::env::var("HOME").ok();
    let path = configured_path(req.cfg, &repo_name, &req.plan.local_branch, home.as_deref());
    let args = herdr::worktree_create_args(
        req.repo_root,
        &req.plan.local_branch,
        path.as_deref(),
        &workspace_label(req.pr),
    );
    let stdout = herdr::run(req.runner, req.herdr_bin, &args)?;

    if let (Some(cmd), Some(pane)) = (&req.cfg.post_create_command, herdr::created_root_pane(&stdout)) {
        // Best effort: the workspace exists either way.
        let _ = herdr::run(req.runner, req.herdr_bin, &["pane".into(), "run".into(), pane, cmd.clone()]);
    }
    Ok(Opened::Created { branch: req.plan.local_branch.clone(), state })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_path_expands_home_and_slugs_branch() {
        let cfg = Config { worktree_dir: Some("~/wt".into()), ..Config::default() };
        assert_eq!(
            configured_path(&cfg, "app", "pr/12-Fix Bug", Some("/home/u")),
            Some(PathBuf::from("/home/u/wt/app/pr-12-fix-bug"))
        );
        assert_eq!(configured_path(&Config::default(), "app", "b", Some("/h")), None);
    }
}
