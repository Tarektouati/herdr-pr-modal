//! GitHub (and GitHub Enterprise) through `gh`.

use super::{FetchPlan, Provider, ProviderError, ProviderKind, RepoRef, cli_error, plan_for};
use crate::cmd::Runner;
use crate::model::{Ci, Listing, Pr};
use serde::Deserialize;
use std::path::Path;

pub const PR_FIELDS: &str =
    "number,title,author,headRefName,isCrossRepository,isDraft,reviewRequests,url,statusCheckRollup";
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

#[derive(Debug, Deserialize)]
struct GhReviewRequest {
    #[serde(default)]
    login: Option<String>,
}

/// A `CheckRun` (status + conclusion) or a commit `StatusContext` (state).
#[derive(Debug, Deserialize)]
struct GhCheck {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhNumber {
    number: u64,
}

/// Roll individual checks up to one marker: any failure fails, then any
/// pending is pending, then pass. Skipped / neutral checks do not count.
fn rollup(checks: &[GhCheck]) -> Ci {
    let mut any = false;
    let mut pending = false;
    for c in checks {
        let verdict = match (&c.conclusion, &c.state, &c.status) {
            (Some(conc), _, _) if !conc.is_empty() => conc.as_str(),
            (_, Some(state), _) => state.as_str(),
            (_, _, Some(status)) if status != "COMPLETED" => "PENDING",
            _ => continue,
        };
        match verdict {
            "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
                return Ci::Fail;
            }
            "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" => {
                any = true;
                pending = true;
            }
            "SUCCESS" => any = true,
            _ => {} // SKIPPED, NEUTRAL, STALE
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
