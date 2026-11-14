//! GitHub (and GitHub Enterprise) through `gh`.

use super::{FetchPlan, Provider, ProviderError, ProviderKind, RepoRef, cli_error, plan_for};
use crate::cmd::Runner;
use crate::model::{
    Check, Checks, Ci, Comment, Comments, Listing, Pr, PrDetails, PrMeta, Reviewer, latest_comments,
    sort_checks,
};
use serde::Deserialize;
use std::path::Path;

pub const PR_FIELDS: &str =
    "number,title,author,headRefName,isCrossRepository,isDraft,reviewRequests,url,statusCheckRollup";
/// Fields for the preview (`gh pr view --json`).
pub const VIEW_FIELDS: &str = "number,title,body,author,baseRefName,headRefName,isDraft,labels,\
reviewDecision,reviewRequests,latestReviews,additions,deletions,changedFiles,createdAt,updatedAt,\
statusCheckRollup,comments";
const LIMIT: &str = "200";

pub struct GitHub<'a> {
    pub runner: &'a dyn Runner,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    title: String,
    #[serde(default)]
    author: Option<GhLogin>,
    head_ref_name: String,
    #[serde(default)]
    is_cross_repository: bool,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    review_requests: Vec<GhReviewRequest>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    status_check_rollup: Option<Vec<GhCheck>>,
}

#[derive(Debug, Deserialize)]
struct GhLogin {
    login: String,
}

/// A user (`login`) or a team (`slug`, `name`) review request.
#[derive(Debug, Deserialize)]
struct GhReviewRequest {
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    slug: Option<String>,
}

/// A `CheckRun` (name, status + conclusion) or a commit `StatusContext`
/// (context, state).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhCheck {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    workflow_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrView {
    #[serde(default)]
    body: String,
    #[serde(default)]
    author: Option<GhLogin>,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    review_requests: Vec<GhReviewRequest>,
    #[serde(default)]
    latest_reviews: Vec<GhReview>,
    #[serde(default)]
    additions: Option<u64>,
    #[serde(default)]
    deletions: Option<u64>,
    #[serde(default)]
    changed_files: Option<u64>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    status_check_rollup: Option<Vec<GhCheck>>,
    #[serde(default)]
    comments: Vec<GhComment>,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhReview {
    #[serde(default)]
    author: Option<GhLogin>,
    #[serde(default)]
    state: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhComment {
    #[serde(default)]
    author: Option<GhLogin>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct GhNumber {
    number: u64,
}

fn verdict_ci(verdict: &str) -> Ci {
    match verdict {
        "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => Ci::Fail,
        "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" => Ci::Pending,
        "SUCCESS" => Ci::Pass,
        _ => Ci::None, // SKIPPED, NEUTRAL, STALE
    }
}

/// One check's verdict and the raw status it came from. `None` when the
/// check reports nothing usable (a completed run without a conclusion).
fn check_status(c: &GhCheck) -> Option<(Ci, &str)> {
    match (&c.conclusion, &c.state, &c.status) {
        (Some(conc), _, _) if !conc.is_empty() => Some((verdict_ci(conc), conc)),
        (_, Some(state), _) => Some((verdict_ci(state), state)),
        (_, _, Some(status)) if status != "COMPLETED" => Some((Ci::Pending, status)),
        _ => None,
    }
}

/// Roll individual checks up to one marker: any failure fails, then any
/// pending is pending, then pass. Skipped / neutral checks do not count.
fn rollup(checks: &[GhCheck]) -> Ci {
    let mut any = false;
    let mut pending = false;
    for c in checks {
        match check_status(c).map(|(ci, _)| ci) {
            Some(Ci::Fail) => return Ci::Fail,
            Some(Ci::Pending) => {
                any = true;
                pending = true;
            }
            Some(Ci::Pass) => any = true,
            _ => {}
        }
    }
    match (any, pending) {
        (false, _) => Ci::None,
        (true, true) => Ci::Pending,
        (true, false) => Ci::Pass,
    }
}

/// Parse `gh pr list --json PR_FIELDS`. `review_requested_numbers` comes
/// from a `review-requested:@me` search, which also covers team requests.
pub fn parse_pr_list(json: &str, viewer: &str, review_requested_numbers: &[u64]) -> Result<Vec<Pr>, String> {
    let raw: Vec<GhPr> = serde_json::from_str(json).map_err(|e| format!("unexpected gh output: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|p| {
            let direct = p.review_requests.iter().any(|r| {
                r.login.as_deref().is_some_and(|l| !viewer.is_empty() && l.eq_ignore_ascii_case(viewer))
            });
            Pr {
                number: p.number,
                title: p.title,
                author: p.author.map(|a| a.login).unwrap_or_else(|| "ghost".into()),
                head_branch: p.head_ref_name,
                is_fork: p.is_cross_repository,
                is_draft: p.is_draft,
                ci: rollup(p.status_check_rollup.as_deref().unwrap_or_default()),
                review_requested: direct || review_requested_numbers.contains(&p.number),
                url: p.url,
            }
        })
        .collect())
}

/// `CHANGES_REQUESTED` → `changes requested`.
fn phrase(s: &str) -> String {
    s.to_lowercase().replace('_', " ")
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

fn login(a: Option<GhLogin>) -> String {
    a.map(|a| a.login).unwrap_or_else(|| "ghost".into())
}

/// Parse `gh pr view --json VIEW_FIELDS` into the preview sections.
pub fn parse_pr_view(json: &str) -> Result<(PrMeta, Checks, Comments), String> {
    let v: GhPrView = serde_json::from_str(json).map_err(|e| format!("unexpected gh output: {e}"))?;

    let mut reviewers: Vec<Reviewer> = v
        .latest_reviews
        .into_iter()
        .map(|r| Reviewer { login: login(r.author), state: non_empty(Some(phrase(&r.state))) })
        .collect();
    for r in v.review_requests {
        let Some(name) = non_empty(r.login).or(non_empty(r.slug)) else { continue };
        if !reviewers.iter().any(|x| x.login.eq_ignore_ascii_case(&name)) {
            reviewers.push(Reviewer { login: name, state: Some("pending".into()) });
        }
    }

    let meta = PrMeta {
        author: login(v.author),
        head_branch: v.head_ref_name,
        base_branch: v.base_ref_name,
        is_draft: v.is_draft,
        review_decision: non_empty(v.review_decision).map(|d| phrase(&d)),
        reviewers,
        labels: v.labels.into_iter().map(|l| l.name).collect(),
        additions: v.additions,
        deletions: v.deletions,
        changed_files: v.changed_files.map(|n| n.to_string()),
        created_at: v.created_at,
        updated_at: v.updated_at,
        body: v.body,
    };

    let mut checks: Vec<Check> = v
        .status_check_rollup
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let (ci, label) = check_status(c)
                .map(|(ci, l)| (ci, phrase(l)))
                .unwrap_or((Ci::None, c.status.as_deref().map(phrase).unwrap_or_else(|| "unknown".into())));
            Check {
                name: non_empty(c.name.clone())
                    .or(non_empty(c.context.clone()))
                    .unwrap_or_else(|| "check".into()),
                group: non_empty(c.workflow_name.clone()),
                ci,
                label,
            }
        })
        .collect();
    sort_checks(&mut checks);

    let comments = latest_comments(
        v.comments
            .into_iter()
            .map(|c| Comment { author: login(c.author), created_at: c.created_at, body: c.body })
            .collect(),
    );
    // `gh` preloads every check, so the list is complete.
    Ok((meta, Checks { items: checks, more_unknown: false }, comments))
}

impl Provider for GitHub<'_> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    fn cli(&self) -> &'static str {
        "gh"
    }

    fn list_open_prs(&self, repo: &RepoRef, cwd: &Path) -> Result<Listing, ProviderError> {
        let target = repo.qualified();
        let run = |args: &[&str]| self.runner.run("gh", args, Some(cwd));
        let (viewer, (list, requested)) = std::thread::scope(|s| {
            let viewer = s.spawn(|| run(&["api", "--hostname", &repo.host, "user", "--jq", ".login"]));
            let list = s.spawn(|| {
                run(&["pr", "list", "-R", &target, "--state", "open", "--limit", LIMIT, "--json", PR_FIELDS])
            });
            let requested = s.spawn(|| {
                run(&[
                    "pr",
                    "list",
                    "-R",
                    &target,
                    "--state",
                    "open",
                    "--limit",
                    LIMIT,
                    "--search",
                    "review-requested:@me",
                    "--json",
                    "number",
                ])
            });
            (viewer.join().unwrap(), (list.join().unwrap(), requested.join().unwrap()))
        });

        let list = match list {
            Ok(out) if out.success => out,
            other => return Err(cli_error("gh", &repo.host, other)),
        };
        let viewer = match viewer {
            Ok(out) if out.success => out.stdout.trim().to_string(),
            other => return Err(cli_error("gh", &repo.host, other)),
        };
        // The search is a nicety (team review requests); never fail on it.
        let requested: Vec<u64> = requested
            .ok()
            .filter(|o| o.success)
            .and_then(|o| serde_json::from_str::<Vec<GhNumber>>(&o.stdout).ok())
            .map(|v| v.into_iter().map(|n| n.number).collect())
            .unwrap_or_default();
        let prs =
            parse_pr_list(&list.stdout, &viewer, &requested).map_err(|e| ProviderError::new(e, None))?;
        Ok(Listing { viewer, prs })
    }

    fn fetch_head(&self, pr: &Pr, remote: &str) -> FetchPlan {
        plan_for(pr, remote, format!("refs/pull/{}/head", pr.number))
    }

    /// One `gh pr view` call holds every section, so a failure fails them all.
    fn pr_details(&self, repo: &RepoRef, cwd: &Path, number: u64) -> PrDetails {
        let n = number.to_string();
        let target = repo.qualified();
        let out = self.runner.run("gh", &["pr", "view", &n, "-R", &target, "--json", VIEW_FIELDS], Some(cwd));
        match out {
            Ok(o) if o.success => match parse_pr_view(&o.stdout) {
                Ok((meta, checks, comments)) => {
                    PrDetails { meta: Ok(meta), checks: Ok(checks), comments: Ok(comments) }
                }
                Err(e) => PrDetails::failed(ProviderError::new(e, None)),
            },
            other => PrDetails::failed(cli_error("gh", &repo.host, other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checks(json: &str) -> Ci {
        rollup(&serde_json::from_str::<Vec<GhCheck>>(json).unwrap())
    }

    #[test]
    fn rollup_rules() {
        assert_eq!(checks("[]"), Ci::None);
        assert_eq!(checks(r#"[{"status":"COMPLETED","conclusion":"SKIPPED"}]"#), Ci::None);
        assert_eq!(
            checks(
                r#"[{"status":"COMPLETED","conclusion":"SUCCESS"},{"status":"IN_PROGRESS","conclusion":""}]"#
            ),
            Ci::Pending
        );
        assert_eq!(
            checks(
                r#"[{"status":"IN_PROGRESS","conclusion":""},{"status":"COMPLETED","conclusion":"FAILURE"}]"#
            ),
            Ci::Fail
        );
        assert_eq!(checks(r#"[{"__typename":"StatusContext","state":"SUCCESS"}]"#), Ci::Pass);
        assert_eq!(checks(r#"[{"__typename":"StatusContext","state":"PENDING"}]"#), Ci::Pending);
    }

    #[test]
    fn team_review_requests_have_no_login() {
        let json = r#"[{"number":1,"title":"t","author":{"login":"a"},"headRefName":"b",
            "reviewRequests":[{"__typename":"Team","name":"core","slug":"core"}]}]"#;
        let prs = parse_pr_list(json, "me", &[]).unwrap();
        assert!(!prs[0].review_requested);
        let prs = parse_pr_list(json, "me", &[1]).unwrap();
        assert!(prs[0].review_requested);
    }
}
