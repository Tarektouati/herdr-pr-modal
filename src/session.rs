//! Work the modal runs off the UI thread: resolve the target repo, load PRs
//! (cache first), and find which PRs already have worktrees.

use crate::app::ErrRow;
use crate::cache;
use crate::cmd::Runner;
use crate::config::Config;
use crate::git;
use crate::model::Listing;
use crate::provider::{self, ProviderKind, RepoRef};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub root: PathBuf,
    pub repo: RepoRef,
    pub kind: ProviderKind,
}

/// Target repo = the origin pane's working directory. Never falls back to
/// the popup's own cwd, which is the plugin checkout.
pub fn resolve_repo(
    runner: &dyn Runner,
    cwd: Option<&str>,
    cfg: &Config,
    config_path: &str,
) -> Result<RepoInfo, ErrRow> {
    let Some(cwd) = cwd else {
        return Err(ErrRow::new("could not tell which directory the origin pane is in", None));
    };
    let root = git::repo_root(runner, Path::new(cwd))
        .map_err(|e| ErrRow::new(e, None))?
        .ok_or_else(|| ErrRow::new(format!("not a git repository: {cwd}"), None))?;
    let url = git::remote_url(runner, &root, &cfg.remote)
        .map_err(|e| ErrRow::new(e, Some(format!("set remote = \"<name>\" in {config_path}"))))?;
    let repo = provider::parse_remote_url(&url)
        .ok_or_else(|| ErrRow::new(format!("'{}' is not a GitHub/GitLab URL: {url}", cfg.remote), None))?;
    let kind = provider::detect(&repo.host, cfg.provider).ok_or_else(|| {
        ErrRow::new(
            format!("can't tell whether {} is GitHub or GitLab", repo.host),
            Some(format!("set provider = \"github\" or \"gitlab\" in {config_path}")),
        )
    })?;
    Ok(RepoInfo { root, repo, kind })
}

pub fn cache_key(info: &RepoInfo) -> String {
    info.repo.qualified()
}

/// PR listing plus its cache age when it came from cache. `force` skips the
/// cache (refresh key).
pub fn load(
    runner: &dyn Runner,
    info: &RepoInfo,
    cfg: &Config,
    state_dir: Option<&Path>,
    force: bool,
) -> Result<(Listing, Option<u64>), ErrRow> {
    let now = cache::now_secs();
    if let (false, Some(dir)) = (force, state_dir)
        && let Some((listing, age)) = cache::read(dir, &cache_key(info), cfg.cache_ttl_secs, now)
    {
        return Ok((listing, Some(age)));
    }
    let provider = provider::for_kind(info.kind, runner);
    let listing =
        provider.list_open_prs(&info.repo, &info.root).map_err(|e| ErrRow::new(e.message, e.fix))?;
    if let Some(dir) = state_dir {
        cache::write(dir, &cache_key(info), &listing, now);
    }
    Ok((listing, None))
}

/// Numbers of PRs whose branch is checked out in some worktree.
pub fn worktree_marks(runner: &dyn Runner, info: &RepoInfo, cfg: &Config, listing: &Listing) -> HashSet<u64> {
    let Ok(worktrees) = git::worktrees(runner, &info.root) else {
        return HashSet::new();
    };
    let provider = provider::for_kind(info.kind, runner);
    listing
        .prs
        .iter()
        .filter(|pr| {
            git::find_pr_worktree(&worktrees, &provider.fetch_head(pr, &cfg.remote), pr.number).is_some()
        })
        .map(|pr| pr.number)
        .collect()
}
