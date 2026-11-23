//! Work the modal runs off the UI thread: resolve the target repo, load PRs
//! (cache first), and find which PRs already have worktrees.

use crate::app::ErrRow;
use crate::cache;
use crate::cmd::Runner;
use crate::config::Config;
use crate::git;
use crate::herdr;
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

/// One repo tab before its remote is resolved. Resolution happens on the
/// worker thread with the tab's first load, so one bad remote doesn't hold
/// up the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabSeed {
    pub root: PathBuf,
    /// Repo name as Herdr shows it (or the directory name).
    pub label: String,
}

/// Target repo = the origin pane's working directory. Never falls back to
/// the popup's own cwd, which is the plugin checkout.
pub fn resolve_repo(
    runner: &dyn Runner,
    cwd: Option<&str>,
    cfg: &Config,
    config_path: &str,
) -> Result<RepoInfo, ErrRow> {
    let root = resolve_cwd_root(runner, cwd)?;
    resolve_root(runner, &root, cfg, config_path)
}

/// Git top level of the origin pane's working directory.
pub fn resolve_cwd_root(runner: &dyn Runner, cwd: Option<&str>) -> Result<PathBuf, ErrRow> {
    let Some(cwd) = cwd else {
        return Err(ErrRow::new("could not tell which directory the origin pane is in", None));
    };
    git::repo_root(runner, Path::new(cwd))
        .map_err(|e| ErrRow::new(e, None))?
        .ok_or_else(|| ErrRow::new(format!("not a git repository: {cwd}"), None))
}

/// Remote URL → forge repo → provider, for a known repo root.
pub fn resolve_root(
    runner: &dyn Runner,
    root: &Path,
    cfg: &Config,
    config_path: &str,
) -> Result<RepoInfo, ErrRow> {
    let url = git::remote_url(runner, root, &cfg.remote)
        .map_err(|e| ErrRow::new(e, Some(format!("set remote = \"<name>\" in {config_path}"))))?;
    let repo = provider::parse_remote_url(&url)
        .ok_or_else(|| ErrRow::new(format!("'{}' is not a GitHub/GitLab URL: {url}", cfg.remote), None))?;
    let kind = provider::detect(&repo.host, cfg.provider).ok_or_else(|| {
        ErrRow::new(
            format!("can't tell whether {} is GitHub or GitLab", repo.host),
            Some(format!("set provider = \"github\" or \"gitlab\" in {config_path}")),
        )
    })?;
    Ok(RepoInfo { root: root.to_path_buf(), repo, kind })
}

/// One tab per repo open in Herdr, plus the origin's repo if it isn't one
/// of them (prepended). Returns the tabs and the one to show first: the
/// origin's, else the first. If `workspace list` fails, only the origin's
/// repo is shown (single-repo mode). The origin not being in a repo is
/// only fatal when there is nothing else to show.
pub fn discover_tabs(
    runner: &dyn Runner,
    herdr_bin: &str,
    origin_cwd: Option<&str>,
) -> Result<(Vec<TabSeed>, usize), ErrRow> {
    let origin = resolve_cwd_root(runner, origin_cwd);
    let repos = herdr::run(runner, herdr_bin, &herdr::workspace_list_args())
        .map(|out| herdr::parse_repo_roots(&out))
        .unwrap_or_default();
    let mut tabs: Vec<TabSeed> =
        repos.iter().map(|r| TabSeed { root: r.root.clone(), label: r.name.clone() }).collect();
    match origin {
        Ok(root) => match repos.iter().position(|r| r.contains(&root)) {
            Some(i) => Ok((tabs, i)),
            None => {
                tabs.insert(0, TabSeed { label: herdr::dir_name(&root), root });
                Ok((tabs, 0))
            }
        },
        Err(e) if tabs.is_empty() => Err(e),
        Err(_) => Ok((tabs, 0)),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{Output, ScriptedRunner};

    const LIST: &str = r#"{"result":{"workspaces":[
      {"number":1,"worktree":{"checkout_path":"/nx/api","repo_name":"api","repo_root":"/nx/api"}},
      {"number":2,"label":"notes"},
      {"number":3,"worktree":{"checkout_path":"/nx/web","repo_name":"web","repo_root":"/nx/web"}},
      {"number":4,"worktree":{"checkout_path":"/nx/wt/web/feat","repo_name":"web","repo_root":"/nx/web"}}
    ]}}"#;

    fn runner(toplevel: Result<Output, crate::cmd::RunError>, list: Output) -> ScriptedRunner {
        ScriptedRunner::new()
            .on(&["git", "rev-parse", "--show-toplevel"], toplevel)
            .on(&["herdr", "workspace", "list"], Ok(list))
    }

    fn labels(tabs: &[TabSeed]) -> Vec<&str> {
        tabs.iter().map(|t| t.label.as_str()).collect()
    }

    #[test]
    fn origin_in_a_linked_worktree_selects_its_repo_tab() {
        let r = runner(Ok(Output::ok("/nx/wt/web/feat\n")), Output::ok(LIST));
        let (tabs, active) = discover_tabs(&r, "herdr", Some("/nx/wt/web/feat/src")).unwrap();
        assert_eq!(labels(&tabs), ["api", "web"]);
        assert_eq!(active, 1);
        assert_eq!(tabs[1].root, PathBuf::from("/nx/web"));
    }

    #[test]
    fn origin_repo_not_open_in_herdr_is_prepended() {
        let r = runner(Ok(Output::ok("/elsewhere/tool\n")), Output::ok(LIST));
        let (tabs, active) = discover_tabs(&r, "herdr", Some("/elsewhere/tool")).unwrap();
        assert_eq!(labels(&tabs), ["tool", "api", "web"]);
        assert_eq!(active, 0);
    }

    #[test]
    fn origin_outside_a_repo_opens_on_the_first_tab() {
        let r = runner(Ok(Output::fail("fatal: not a git repository")), Output::ok(LIST));
        let (tabs, active) = discover_tabs(&r, "herdr", Some("/nx")).unwrap();
        assert_eq!(labels(&tabs), ["api", "web"]);
        assert_eq!(active, 0);
    }

    #[test]
    fn failing_workspace_list_falls_back_to_the_origin_repo() {
        let r = runner(Ok(Output::ok("/nx/web\n")), Output::fail("no server"));
        let (tabs, active) = discover_tabs(&r, "herdr", Some("/nx/web")).unwrap();
        assert_eq!(labels(&tabs), ["web"]);
        assert_eq!(active, 0);
    }

    #[test]
    fn no_repo_anywhere_is_the_old_fatal_error() {
        let r = runner(Ok(Output::fail("fatal")), Output::fail("no server"));
        let err = discover_tabs(&r, "herdr", Some("/tmp/x")).unwrap_err();
        assert_eq!(err.message, "not a git repository: /tmp/x");
        let r = runner(Ok(Output::fail("fatal")), Output::ok(r#"{"result":{"workspaces":[]}}"#));
        assert!(discover_tabs(&r, "herdr", None).is_err());
    }
}
