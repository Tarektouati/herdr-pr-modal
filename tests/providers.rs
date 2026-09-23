//! Provider parsing against recorded CLI output.
//!
//! - `gh_pr_list.json`: `gh pr list -R cli/cli --json <github::PR_FIELDS>`
//!   recorded 2026-09-23 (7 PRs: same-repo, fork, draft, failing CI, no CI).
//! - `glab_mr_list.json` / `glab_pipelines.json`: gitlab-org/cli merge
//!   requests and pipelines recorded 2026-09-23 from the REST API that
//!   `glab mr list -F json` and `glab api` print.

use herdr_pr_modal::cmd::{Output, RunError, ScriptedRunner};
use herdr_pr_modal::model::{Ci, Group, grouped};
use herdr_pr_modal::provider::github::{self, GitHub, PR_FIELDS};
use herdr_pr_modal::provider::gitlab::{self, GitLab};
use herdr_pr_modal::provider::{Provider, RepoRef};
use std::path::Path;

const GH: &str = include_str!("fixtures/gh_pr_list.json");
const GL_MRS: &str = include_str!("fixtures/glab_mr_list.json");
const GL_PIPES: &str = include_str!("fixtures/glab_pipelines.json");

#[test]
fn gh_fixture_parses_every_field() {
    let prs = github::parse_pr_list(GH, "williammartin", &[13013]).unwrap();
    assert_eq!(prs.len(), 7);

    let same = prs.iter().find(|p| p.number == 14509).unwrap();
    assert_eq!(same.author, "williammartin");
    assert_eq!(same.head_branch, "williammartin-reviewer-facing-pr-guidance");
    assert!(!same.is_fork && !same.is_draft);
    assert_eq!(same.ci, Ci::Pass);
    assert_eq!(same.url, "https://github.com/cli/cli/pull/14509");

    let fork = prs.iter().find(|p| p.number == 14474).unwrap();
    assert!(fork.is_fork);

    let draft = prs.iter().find(|p| p.number == 14355).unwrap();
    assert!(draft.is_draft);

    let failing = prs.iter().find(|p| p.number == 14148).unwrap();
    assert_eq!(failing.ci, Ci::Fail);

    let no_checks = prs.iter().find(|p| p.number == 13982).unwrap();
    assert_eq!(no_checks.ci, Ci::None);

    // Direct user review request (13013 lists williammartin as reviewer).
    assert!(prs.iter().find(|p| p.number == 13013).unwrap().review_requested);
    assert!(!prs.iter().find(|p| p.number == 14474).unwrap().review_requested);
}

#[test]
fn gh_fixture_groups() {
    let prs = github::parse_pr_list(GH, "williammartin", &[14474]).unwrap();
    let listing = herdr_pr_modal::model::Listing { viewer: "williammartin".into(), prs };
    let groups: Vec<(Group, Vec<u64>)> = grouped(&listing, "")
        .into_iter()
        .map(|(g, ix)| (g, ix.into_iter().map(|i| listing.prs[i].number).collect()))
        .collect();
    assert_eq!(
        groups,
        vec![
            (Group::Mine, vec![14509, 14355]),
            (Group::ReviewRequested, vec![14474, 13013]),
            (Group::Others, vec![14373, 14148, 13982]),
        ]
    );
}

#[test]
fn glab_fixture_parses_forks_drafts_reviewers_and_ci() {
    let ci = gitlab::parse_pipelines(GL_PIPES);
    let mrs = gitlab::parse_mr_list(GL_MRS, "GitLabDuo", &ci).unwrap();
    assert_eq!(mrs.len(), 5);

    let same = mrs.iter().find(|m| m.number == 3956).unwrap();
    assert_eq!(same.author, "jhebden");
    assert_eq!(same.head_branch, "feat/theme-color-overrides");
    assert!(!same.is_fork);
    assert_eq!(same.ci, Ci::Pass);
    assert!(same.review_requested);
    assert_eq!(same.url, "https://gitlab.com/gitlab-org/cli/-/merge_requests/3956");

    let fork = mrs.iter().find(|m| m.number == 3957).unwrap();
    assert!(fork.is_fork);
    assert_eq!(fork.ci, Ci::None);

    let draft = mrs.iter().find(|m| m.number == 3911).unwrap();
    assert!(draft.is_draft);
    assert!(!draft.review_requested);

    // The fixture also holds an unrelated failed pipeline.
    assert!(ci.values().any(|c| *c == Ci::Fail));
}

fn repo(host: &str, path: &str) -> RepoRef {
    RepoRef { host: host.into(), path: path.into() }
}

#[test]
fn github_list_runs_gh_with_host_qualified_repo() {
    let runner = ScriptedRunner::new()
        .on(&["gh", "api"], Ok(Output::ok("williammartin\n")))
        .on(
            &["gh", "pr", "list", "-R", "ghe.corp/cli/cli", "--state", "open", "--limit", "200", "--json"],
            Ok(Output::ok(GH)),
        )
        .on(
            &["gh", "pr", "list", "-R", "ghe.corp/cli/cli", "--state", "open", "--limit", "200", "--search"],
            Ok(Output::ok(r#"[{"number":14474}]"#)),
        );
    let listing =
        GitHub { runner: &runner }.list_open_prs(&repo("ghe.corp", "cli/cli"), Path::new("/r")).unwrap();
    assert_eq!(listing.viewer, "williammartin");
    assert_eq!(listing.prs.len(), 7);
    assert!(listing.prs.iter().find(|p| p.number == 14474).unwrap().review_requested);
    let calls = runner.calls();
    assert!(calls.contains(&vec![
        "gh".into(),
        "api".into(),
        "--hostname".into(),
        "ghe.corp".into(),
        "user".into(),
        "--jq".into(),
        ".login".into()
    ]));
    assert!(calls.iter().any(|c| c.last().map(String::as_str) == Some(PR_FIELDS)));
}

#[test]
fn github_missing_cli_and_auth_errors_name_the_fix() {
    let runner = ScriptedRunner::new()
        .on(&["gh"], Err(RunError::NotFound("gh".into())))
        .on(&["gh"], Err(RunError::NotFound("gh".into())))
        .on(&["gh"], Err(RunError::NotFound("gh".into())));
    let err =
        GitHub { runner: &runner }.list_open_prs(&repo("github.com", "a/b"), Path::new("/r")).unwrap_err();
    assert_eq!(err.message, "gh is not installed");

    let unauth = || Ok(Output::fail("To get started with GitHub CLI, please run:  gh auth login"));
    let runner = ScriptedRunner::new().on(&["gh"], unauth()).on(&["gh"], unauth()).on(&["gh"], unauth());
    let err =
        GitHub { runner: &runner }.list_open_prs(&repo("github.com", "a/b"), Path::new("/r")).unwrap_err();
    assert_eq!(err.fix.as_deref(), Some("gh auth login --hostname github.com"));
}

#[test]
fn gitlab_list_uses_full_url_and_encoded_project_path() {
    let runner = ScriptedRunner::new()
        .on(
            &["glab", "api", "--hostname", "gitlab.corp", "user"],
            Ok(Output::ok(r#"{"username":"GitLabDuo"}"#)),
        )
        .on(&["glab", "mr", "list", "-R", "https://gitlab.corp/grp/sub/cli"], Ok(Output::ok(GL_MRS)))
        .on(
            &["glab", "api", "--hostname", "gitlab.corp", "projects/grp%2Fsub%2Fcli/pipelines?per_page=100"],
            Ok(Output::ok(GL_PIPES)),
        );
    let listing = GitLab { runner: &runner }
        .list_open_prs(&repo("gitlab.corp", "grp/sub/cli"), Path::new("/r"))
        .unwrap();
    assert_eq!(listing.viewer, "GitLabDuo");
    assert_eq!(listing.prs.len(), 5);
    assert_eq!(listing.prs.iter().find(|m| m.number == 3956).unwrap().ci, Ci::Pass);
}

#[test]
fn gitlab_unauthenticated_shows_glab_login() {
    let unauth = || Ok(Output::fail("GET https://gitlab.com/api/v4/user: 401 {message: 401 Unauthorized}"));
    let runner =
        ScriptedRunner::new().on(&["glab"], unauth()).on(&["glab"], unauth()).on(&["glab"], unauth());
    let err =
        GitLab { runner: &runner }.list_open_prs(&repo("gitlab.com", "a/b"), Path::new("/r")).unwrap_err();
    assert_eq!(err.message, "glab is not authenticated for gitlab.com");
    assert_eq!(err.fix.as_deref(), Some("glab auth login --hostname gitlab.com"));
}

#[test]
fn fetch_refs_per_provider() {
    let runner = ScriptedRunner::new();
    let prs = github::parse_pr_list(GH, "", &[]).unwrap();
    let fork = prs.iter().find(|p| p.number == 14474).unwrap();
    let plan = GitHub { runner: &runner }.fetch_head(fork, "origin");
    assert_eq!(plan.source_ref, "refs/pull/14474/head");
    assert_eq!(plan.local_branch, "pr/14474-remove-claude-md");

    let mrs = gitlab::parse_mr_list(GL_MRS, "", &Default::default()).unwrap();
    let fork = mrs.iter().find(|m| m.number == 3957).unwrap();
    let plan = GitLab { runner: &runner }.fetch_head(fork, "origin");
    assert_eq!(plan.source_ref, "refs/merge-requests/3957/head");
    // Slug capped at 40 chars.
    assert_eq!(plan.local_branch, "pr/3957-fix-issue-list-output-enum-and-changelog");
}
