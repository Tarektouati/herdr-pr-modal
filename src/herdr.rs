//! Herdr CLI calls. Plugins call Herdr through `HERDR_BIN_PATH`, which keeps
//! the socket transport out of the plugin.

use crate::cmd::{RunError, Runner};
use crate::config::Config;
use crate::context::Origin;
use crate::ids::{PANE_MODAL, PLUGIN_ID};
use std::path::Path;

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
