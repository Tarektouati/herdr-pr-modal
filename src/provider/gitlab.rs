//! GitLab (gitlab.com and self-hosted) merge requests through `glab`.
//!
//! `glab mr list -F json` prints GitLab REST merge request objects, which do
//! not carry pipeline status. One `pipelines` call per listing fills the CI
//! marker by matching each MR head `sha`.

use super::{FetchPlan, Provider, ProviderError, ProviderKind, RepoRef, cli_error, plan_for};
use crate::cmd::Runner;
use crate::model::{
    Check, Checks, Ci, Comment, Comments, Listing, Pr, PrDetails, PrMeta, Reviewer, latest_comments,
    sort_checks,
};
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

#[derive(Debug, Deserialize)]
struct GlMrView {
    #[serde(default)]
    author: Option<GlUser>,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    work_in_progress: bool,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    reviewers: Vec<GlUser>,
    #[serde(default)]
    changes_count: Option<String>,
    #[serde(default)]
    detailed_merge_status: Option<String>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlNote {
    #[serde(default)]
    author: Option<GlUser>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    system: bool,
    /// `DiffNote` for an inline diff comment, `null` for a conversation one.
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlMrPipeline {
    id: u64,
    #[serde(default)]
    project_id: Option<u64>,
}

/// A pipeline job or a bridge (trigger job); both carry these fields.
#[derive(Debug, Deserialize)]
struct GlJob {
    #[serde(default)]
    id: u64,
    name: String,
    #[serde(default)]
    stage: Option<String>,
    status: String,
    #[serde(default)]
    allow_failure: bool,
}

/// Notes are fetched newest first, one page.
pub const NOTES_PAGE: usize = 100;
/// Jobs are fetched in pages of `JOBS_PAGE`, at most `MAX_JOB_PAGES` of them.
pub const JOBS_PAGE: usize = 100;
pub const MAX_JOB_PAGES: usize = 10;

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

/// A parse failure becomes a section error with no fix.
fn parsed<T>(r: Result<T, String>) -> Result<T, ProviderError> {
    r.map_err(|e| ProviderError::new(e, None))
}

fn username(u: Option<GlUser>) -> String {
    u.map(|u| u.username).unwrap_or_else(|| "ghost".into())
}

/// Parse `glab mr view -F json` (a REST merge request object).
pub fn parse_mr_view(json: &str) -> Result<PrMeta, String> {
    let m: GlMrView = serde_json::from_str(json).map_err(|e| format!("unexpected glab output: {e}"))?;
    let review_decision = match m.detailed_merge_status.as_deref() {
        Some("not_approved") => Some("review required".to_string()),
        Some("requested_changes") => Some("changes requested".to_string()),
        _ => None,
    };
    Ok(PrMeta {
        author: username(m.author),
        head_branch: m.source_branch,
        base_branch: m.target_branch,
        is_draft: m.draft || m.work_in_progress,
        review_decision,
        reviewers: m.reviewers.into_iter().map(|r| Reviewer { login: r.username, state: None }).collect(),
        labels: m.labels,
        additions: None,
        deletions: None,
        changed_files: m.changes_count.filter(|c| !c.is_empty()),
        created_at: m.created_at,
        updated_at: m.updated_at,
        body: m.description.unwrap_or_default(),
    })
}

/// Parse a newest-first page of MR notes: system notes and inline diff
/// comments dropped, the latest conversation comments kept oldest → newest.
pub fn parse_notes(json: &str) -> Result<Comments, String> {
    let notes: Vec<GlNote> =
        serde_json::from_str(json).map_err(|e| format!("unexpected glab output: {e}"))?;
    let full_page = notes.len() >= NOTES_PAGE;
    let mut comments: Vec<Comment> = notes
        .into_iter()
        .filter(|n| !n.system && n.kind.as_deref() != Some("DiffNote"))
        .map(|n| Comment { author: username(n.author), created_at: n.created_at, body: n.body })
        .collect();
    comments.reverse();
    let mut out = latest_comments(comments);
    out.more_unknown = full_page;
    Ok(out)
}

/// Newest pipeline of an MR (the API lists newest first) and the project it
/// ran in, which is the fork for fork MRs.
pub fn latest_pipeline(json: &str) -> Result<Option<(u64, Option<u64>)>, String> {
    let list: Vec<GlMrPipeline> =
        serde_json::from_str(json).map_err(|e| format!("unexpected glab output: {e}"))?;
    Ok(list.first().map(|p| (p.id, p.project_id)))
}

fn job_page(json: &str) -> Result<Vec<GlJob>, String> {
    serde_json::from_str(json).map_err(|e| format!("unexpected glab output: {e}"))
}

/// Parse one page of a pipeline's jobs (or bridges) into checks.
pub fn parse_jobs(json: &str) -> Result<Vec<Check>, String> {
    Ok(jobs_to_checks(job_page(json)?))
}

/// Jobs and bridges in pipeline order (by id), then sorted by severity.
fn jobs_to_checks(mut jobs: Vec<GlJob>) -> Vec<Check> {
    jobs.sort_by_key(|j| j.id);
    let mut checks: Vec<Check> = jobs
        .into_iter()
        .map(|j| {
            let (ci, label) = if j.allow_failure && j.status == "failed" {
                (Ci::None, "failed (allowed)".to_string())
            } else {
                (pipeline_ci(&j.status), j.status.replace('_', " "))
            };
            Check { name: j.name, group: j.stage.filter(|s| !s.is_empty()), ci, label }
        })
        .collect();
    sort_checks(&mut checks);
    checks
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

    /// MR view, notes and pipeline jobs are separate calls, run in parallel;
    /// each section reports its own failure. Jobs are paged (up to
    /// `MAX_JOB_PAGES`); bridges (trigger jobs) are best-effort.
    fn pr_details(&self, repo: &RepoRef, cwd: &Path, number: u64) -> PrDetails {
        let url = format!("https://{}/{}", repo.host, repo.path);
        let project = encode_path(&repo.path);
        let iid = number.to_string();
        let notes_route =
            format!("projects/{project}/merge_requests/{iid}/notes?sort=desc&per_page={NOTES_PAGE}");
        let pipes_route = format!("projects/{project}/merge_requests/{iid}/pipelines");
        let host = repo.host.as_str();
        let run = |args: &[&str]| self.runner.run("glab", args, Some(cwd));
        let section = |out: Result<crate::cmd::Output, crate::cmd::RunError>| match out {
            Ok(o) if o.success => Ok(o.stdout),
            other => Err(cli_error("glab", host, other)),
        };

        let (view, notes, checks) = std::thread::scope(|s| {
            let view = s.spawn(|| run(&["mr", "view", &iid, "-R", &url, "-F", "json"]));
            let notes = s.spawn(|| run(&["api", "--hostname", host, &notes_route]));
            let checks = s.spawn(|| {
                let pipes = section(run(&["api", "--hostname", host, &pipes_route]))?;
                let Some((pid, project_id)) = parsed(latest_pipeline(&pipes))? else {
                    return Ok(Checks::default());
                };
                let in_project = project_id.map(|id| id.to_string()).unwrap_or_else(|| project.clone());
                let pipeline = format!("projects/{in_project}/pipelines/{pid}");

                let mut jobs = Vec::new();
                let mut more_unknown = false;
                for page in 1..=MAX_JOB_PAGES {
                    let route = format!("{pipeline}/jobs?per_page={JOBS_PAGE}&page={page}");
                    let batch = parsed(job_page(&section(run(&["api", "--hostname", host, &route]))?))?;
                    let full = batch.len() >= JOBS_PAGE;
                    jobs.extend(batch);
                    if !full {
                        break;
                    }
                    more_unknown = page == MAX_JOB_PAGES;
                }

                // Downstream pipeline triggers; an older or restricted
                // instance refusing this route just leaves them out.
                let bridges_route = format!("{pipeline}/bridges?per_page={JOBS_PAGE}");
                if let Ok(o) = run(&["api", "--hostname", host, &bridges_route])
                    && o.success
                    && let Ok(bridges) = job_page(&o.stdout)
                {
                    jobs.extend(bridges);
                }

                Ok(Checks { items: jobs_to_checks(jobs), more_unknown })
            });
            (view.join().unwrap(), notes.join().unwrap(), checks.join().unwrap())
        });

        PrDetails {
            meta: section(view).and_then(|j| parsed(parse_mr_view(&j))),
            checks,
            comments: section(notes).and_then(|j| parsed(parse_notes(&j))),
        }
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
