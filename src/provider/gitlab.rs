//! GitLab (gitlab.com and self-hosted) merge requests through `glab`.
//!
//! `glab mr list -F json` prints GitLab REST merge request objects, which do
//! not carry pipeline status. One `pipelines` call per listing fills the CI
//! marker by matching each MR head `sha`.

use super::{FetchPlan, Provider, ProviderError, ProviderKind, RepoRef, cli_error, plan_for};
use crate::cmd::Runner;
use crate::model::{Ci, Listing, Pr};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

pub struct GitLab<'a> {
    pub runner: &'a dyn Runner,
}

#[derive(Debug, Deserialize)]
struct GlMr {
    iid: u64,
    title: String,
    #[serde(default)]
    author: Option<GlUser>,
    source_branch: String,
    source_project_id: u64,
    target_project_id: u64,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    work_in_progress: bool,
    #[serde(default)]
    reviewers: Vec<GlUser>,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    web_url: String,
}

#[derive(Debug, Deserialize)]
struct GlUser {
    username: String,
}

#[derive(Debug, Deserialize)]
struct GlPipeline {
    sha: String,
    status: String,
}

fn pipeline_ci(status: &str) -> Ci {
    match status {
        "success" => Ci::Pass,
        "failed" | "canceled" => Ci::Fail,
        "created" | "waiting_for_resource" | "preparing" | "pending" | "running" | "scheduled" => Ci::Pending,
        _ => Ci::None, // skipped, manual
    }
}

/// Latest pipeline status per sha. The API lists newest first, so the first
/// entry for a sha wins.
pub fn parse_pipelines(json: &str) -> HashMap<String, Ci> {
    let mut out = HashMap::new();
    if let Ok(list) = serde_json::from_str::<Vec<GlPipeline>>(json) {
        for p in list {
            out.entry(p.sha).or_insert_with(|| pipeline_ci(&p.status));
        }
    }
    out
}

pub fn parse_mr_list(json: &str, viewer: &str, ci_by_sha: &HashMap<String, Ci>) -> Result<Vec<Pr>, String> {
    let raw: Vec<GlMr> = serde_json::from_str(json).map_err(|e| format!("unexpected glab output: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|m| Pr {
            number: m.iid,
            title: m.title,
            author: m.author.map(|a| a.username).unwrap_or_else(|| "ghost".into()),
            head_branch: m.source_branch,
            is_fork: m.source_project_id != m.target_project_id,
            is_draft: m.draft || m.work_in_progress,
            ci: m.sha.as_ref().and_then(|s| ci_by_sha.get(s)).copied().unwrap_or(Ci::None),
            review_requested: !viewer.is_empty()
                && m.reviewers.iter().any(|r| r.username.eq_ignore_ascii_case(viewer)),
            url: m.web_url,
        })
        .collect())
}

/// Percent-encode a project path for `projects/:id` API routes.
fn encode_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl Provider for GitLab<'_> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitLab
    }

    fn cli(&self) -> &'static str {
        "glab"
    }

    fn list_open_prs(&self, repo: &RepoRef, cwd: &Path) -> Result<Listing, ProviderError> {
        let url = format!("https://{}/{}", repo.host, repo.path);
        let pipelines = format!("projects/{}/pipelines?per_page=100", encode_path(&repo.path));
        let run = |args: &[&str]| self.runner.run("glab", args, Some(cwd));
        let (viewer, list, pipes) = std::thread::scope(|s| {
            let viewer = s.spawn(|| run(&["api", "--hostname", &repo.host, "user"]));
            let list = s.spawn(|| run(&["mr", "list", "-R", &url, "-F", "json", "-P", "100"]));
            let pipes = s.spawn(|| run(&["api", "--hostname", &repo.host, &pipelines]));
            (viewer.join().unwrap(), list.join().unwrap(), pipes.join().unwrap())
        });

        let list = match list {
            Ok(out) if out.success => out,
            other => return Err(cli_error("glab", &repo.host, other)),
        };
        let viewer = match viewer {
            Ok(out) if out.success => {
                serde_json::from_str::<GlUser>(&out.stdout).map(|u| u.username).unwrap_or_default()
            }
            other => return Err(cli_error("glab", &repo.host, other)),
        };
        // CI markers are optional; a failed pipelines call just hides them.
        let ci = pipes.ok().filter(|o| o.success).map(|o| parse_pipelines(&o.stdout)).unwrap_or_default();
        let prs = parse_mr_list(&list.stdout, &viewer, &ci).map_err(|e| ProviderError::new(e, None))?;
        Ok(Listing { viewer, prs })
    }

    fn fetch_head(&self, pr: &Pr, remote: &str) -> FetchPlan {
        plan_for(pr, remote, format!("refs/merge-requests/{}/head", pr.number))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_nested_group_paths() {
        assert_eq!(encode_path("group/sub/my.repo"), "group%2Fsub%2Fmy.repo");
    }

    #[test]
    fn pipeline_statuses() {
        assert_eq!(pipeline_ci("running"), Ci::Pending);
        assert_eq!(pipeline_ci("canceled"), Ci::Fail);
        assert_eq!(pipeline_ci("manual"), Ci::None);
    }

    #[test]
    fn newest_pipeline_per_sha_wins() {
        let ci = parse_pipelines(r#"[{"sha":"a","status":"running"},{"sha":"a","status":"failed"}]"#);
        assert_eq!(ci["a"], Ci::Pending);
    }
}
