//! Herdr CLI calls. Plugins call Herdr through `HERDR_BIN_PATH`, which keeps
//! the socket transport out of the plugin.

use crate::cmd::{RunError, Runner};
use crate::config::Config;
use crate::context::Origin;
use crate::ids::{PANE_MODAL, PLUGIN_ID};
use std::path::{Path, PathBuf};

pub fn bin() -> String {
    std::env::var("HERDR_BIN_PATH").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "herdr".into())
}

/// argv (without the binary) that opens the modal popup for `origin`.
pub fn pane_open_args(origin: &Origin, cfg: &Config) -> Vec<String> {
    let mut args: Vec<String> = [
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        PANE_MODAL,
        "--placement",
        "popup",
        "--width",
        &cfg.width,
        "--height",
        &cfg.height,
        "--focus",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (k, v) in origin.to_env() {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    args
}

pub fn worktree_open_args(repo_root: &Path, checkout: &Path) -> Vec<String> {
    vec![
        "worktree".into(),
        "open".into(),
        "--cwd".into(),
        repo_root.display().to_string(),
        "--path".into(),
        checkout.display().to_string(),
        "--focus".into(),
    ]
}

pub fn worktree_create_args(repo_root: &Path, branch: &str, path: Option<&Path>, label: &str) -> Vec<String> {
    let mut args = vec![
        "worktree".into(),
        "create".into(),
        "--cwd".into(),
        repo_root.display().to_string(),
        "--branch".into(),
        branch.into(),
        "--label".into(),
        label.into(),
        "--focus".into(),
    ];
    if let Some(p) = path {
        args.push("--path".into());
        args.push(p.display().to_string());
    }
    args
}

/// Run a herdr command; on failure return the one line worth showing.
pub fn run(runner: &dyn Runner, bin: &str, args: &[String]) -> Result<String, String> {
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match runner.run(bin, &argv, None) {
        Ok(out) if out.success => Ok(out.stdout),
        Ok(out) => Err(format!("herdr {}: {}", args[..2.min(args.len())].join(" "), out.error_line())),
        Err(RunError::NotFound(_)) => Err(format!("herdr binary not found at {bin}")),
        Err(RunError::Io(e)) => Err(e),
    }
}

/// `result.root_pane.pane_id` from a `worktree create` response, if present.
pub fn created_root_pane(stdout: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    ["/result/root_pane/pane_id", "/root_pane/pane_id"]
        .iter()
        .find_map(|p| v.pointer(p).and_then(|x| x.as_str()).map(String::from))
}

pub fn workspace_list_args() -> Vec<String> {
    vec!["workspace".into(), "list".into()]
}

/// One repo open in Herdr: its main checkout, the name Herdr shows, and
/// every checkout (main or linked worktree) a workspace sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRepo {
    pub root: PathBuf,
    pub name: String,
    pub checkouts: Vec<PathBuf>,
}

impl WorkspaceRepo {
    /// `path` (a git top level) is this repo's root or one of its checkouts.
    pub fn contains(&self, path: &Path) -> bool {
        let path = normalize(path);
        self.root == path || self.checkouts.contains(&path)
    }
}

/// Herdr reports some roots as `/…/repo/.`; resolve that and symlinks so
/// paths compare equal to what git prints.
pub fn normalize(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let trimmed = s.strip_suffix("/.").map(Path::new).unwrap_or(path);
    std::fs::canonicalize(trimmed).unwrap_or_else(|_| trimmed.to_path_buf())
}

/// Repos from `workspace list`, deduplicated by repo root, in workspace
/// number order (where each repo first appears). Workspaces outside a git
/// repo have no `worktree` and are skipped.
pub fn parse_repo_roots(stdout: &str) -> Vec<WorkspaceRepo> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else { return Vec::new() };
    let Some(list) =
        v.pointer("/result/workspaces").or_else(|| v.get("workspaces")).and_then(|w| w.as_array())
    else {
        return Vec::new();
    };
    let mut workspaces: Vec<&serde_json::Value> = list.iter().collect();
    workspaces.sort_by_key(|w| w.get("number").and_then(|n| n.as_u64()).unwrap_or(u64::MAX));
    let mut repos: Vec<WorkspaceRepo> = Vec::new();
    for ws in workspaces {
        let Some(wt) = ws.get("worktree") else { continue };
        let Some(root) = wt.get("repo_root").and_then(|r| r.as_str()).filter(|r| !r.is_empty()) else {
            continue;
        };
        let root = normalize(Path::new(root));
        let checkout = wt.get("checkout_path").and_then(|c| c.as_str()).map(|c| normalize(Path::new(c)));
        let repo = match repos.iter_mut().find(|r| r.root == root) {
            Some(r) => r,
            None => {
                let name = wt
                    .get("repo_name")
                    .and_then(|n| n.as_str())
                    .filter(|n| !n.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| dir_name(&root));
                repos.push(WorkspaceRepo { root: root.clone(), name, checkouts: Vec::new() });
                repos.last_mut().unwrap()
            }
        };
        if let Some(c) = checkout
            && !repo.checkouts.contains(&c)
        {
            repo.checkouts.push(c);
        }
    }
    repos
}

/// Last path component, used as a label when Herdr gives no repo name.
pub fn dir_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[
      {"label":"web","number":1,"workspace_id":"w1","worktree":{"checkout_path":"/nx/web","is_linked_worktree":false,"repo_name":"web","repo_root":"/nx/web"}},
      {"label":"DLP","number":2,"workspace_id":"w2"},
      {"label":"feat-x","number":3,"workspace_id":"w3","worktree":{"checkout_path":"/nx/wt/web/feat-x","is_linked_worktree":true,"repo_name":"web","repo_root":"/nx/web"}},
      {"label":"made","number":5,"workspace_id":"w5","worktree":{"checkout_path":"/nx/made","repo_name":"made","repo_root":"/nx/made"}},
      {"label":"ci-1","number":4,"workspace_id":"w4","worktree":{"checkout_path":"/nx/wt/made/ci-1","is_linked_worktree":true,"repo_name":"made","repo_root":"/nx/made/."}}
    ]}}"#;

    #[test]
    fn repo_roots_are_deduplicated_in_workspace_order() {
        let repos = parse_repo_roots(LIST);
        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["web", "made"]);
        assert_eq!(repos[0].root, PathBuf::from("/nx/web"));
        assert_eq!(repos[0].checkouts, [PathBuf::from("/nx/web"), PathBuf::from("/nx/wt/web/feat-x")]);
        // Number 4 sorts before 5, and its `/.` root is the same repo.
        assert_eq!(repos[1].root, PathBuf::from("/nx/made"));
        assert_eq!(repos[1].checkouts, [PathBuf::from("/nx/wt/made/ci-1"), PathBuf::from("/nx/made")]);
    }

    #[test]
    fn contains_matches_root_and_linked_worktrees() {
        let repos = parse_repo_roots(LIST);
        assert!(repos[0].contains(Path::new("/nx/web")));
        assert!(repos[0].contains(Path::new("/nx/wt/web/feat-x")));
        assert!(repos[1].contains(Path::new("/nx/made/.")));
        assert!(!repos[0].contains(Path::new("/nx/made")));
    }

    #[test]
    fn unparsable_or_empty_output_gives_no_repos() {
        assert!(parse_repo_roots("").is_empty());
        assert!(parse_repo_roots("not json").is_empty());
        assert!(parse_repo_roots(r#"{"result":{"workspaces":[{"number":1}]}}"#).is_empty());
    }
}
