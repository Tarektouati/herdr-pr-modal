//! Provider parsing against recorded CLI output.
//!
//! - `gh_pr_list.json`: `gh pr list -R cli/cli --json <github::PR_FIELDS>`
//!   recorded 2026-09-23 (7 PRs: same-repo, fork, draft, failing CI, no CI).
//! - `glab_mr_list.json` / `glab_pipelines.json`: gitlab-org/cli merge
//!   requests and pipelines recorded 2026-09-23 from the REST API that
//!   `glab mr list -F json` and `glab api` print.
//! - `gh_pr_view.json`: `gh pr view 13013 -R cli/cli --json <github::VIEW_FIELDS>`
//!   recorded 2026-11-14 (labels, review requests, a failing check, bot comments,
//!   an HTML-comment PR template body).
//! - `glab_mr_view.json`, `glab_mr_pipelines.json`, `glab_pipeline_jobs.json`:
//!   gitlab-org/cli MR !3956, its pipelines and its latest pipeline's jobs,
//!   recorded 2026-11-14 from the REST API that `glab mr view -F json` and
//!   `glab api` print.
//! - `glab_mr_notes.json`: hand-written in the REST notes shape (newest first,
//!   system notes mixed in), because gitlab.com only serves MR notes to
//!   authenticated clients.

use herdr_pr_modal::cmd::{Output, RunError, ScriptedRunner};
use herdr_pr_modal::model::{Checks, Ci, Group, MAX_COMMENTS, grouped};
use herdr_pr_modal::provider::github::{self, GitHub, PR_FIELDS, VIEW_FIELDS};
use herdr_pr_modal::provider::gitlab::{self, GitLab};
use herdr_pr_modal::provider::{Provider, RepoRef};
use std::path::Path;

const GH: &str = include_str!("fixtures/gh_pr_list.json");
const GL_MRS: &str = include_str!("fixtures/glab_mr_list.json");
const GL_PIPES: &str = include_str!("fixtures/glab_pipelines.json");
const GH_VIEW: &str = include_str!("fixtures/gh_pr_view.json");
const GL_VIEW: &str = include_str!("fixtures/glab_mr_view.json");
const GL_NOTES: &str = include_str!("fixtures/glab_mr_notes.json");
const GL_MR_PIPES: &str = include_str!("fixtures/glab_mr_pipelines.json");
const GL_JOBS: &str = include_str!("fixtures/glab_pipeline_jobs.json");

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

// ---------------------------------------------------------------------------
// Preview details

#[test]
fn gh_view_fixture_parses_meta_checks_and_comments() {
    let (meta, checks, comments) = github::parse_pr_view(GH_VIEW).unwrap();
    assert_eq!(meta.author, "dsanders11");
    assert_eq!(
        (meta.head_branch.as_str(), meta.base_branch.as_str()),
        ("fix-non-zipped-artifact-download", "trunk")
    );
    assert!(!meta.is_draft);
    assert_eq!(meta.review_decision.as_deref(), Some("review required"));
    assert_eq!(meta.labels, ["external", "ready-for-review"]);
    assert_eq!(
        (meta.additions, meta.deletions, meta.changed_files.as_deref()),
        (Some(78), Some(8), Some("5"))
    );
    assert_eq!(meta.created_at, "2026-03-24T04:06:10Z");
    assert!(meta.body.contains("<!--") && meta.body.contains("Fixes #13012."));
    // Submitted reviews first, then outstanding requests as pending.
    let reviewers: Vec<(&str, Option<&str>)> =
        meta.reviewers.iter().map(|r| (r.login.as_str(), r.state.as_deref())).collect();
    assert_eq!(
        reviewers,
        [
            ("copilot-pull-request-reviewer", Some("commented")),
            ("ke66344060-ship-it", Some("commented")),
            ("williammartin", Some("pending")),
            ("BagToad", Some("pending")),
        ]
    );

    assert!(!checks.more_unknown);
    let checks = checks.items;
    assert_eq!(checks.len(), 18);
    assert_eq!(checks[0].name, "govulncheck");
    assert_eq!(checks[0].group.as_deref(), Some("Lint"));
    assert_eq!((checks[0].ci, checks[0].label.as_str()), (Ci::Fail, "failure"));
    assert!(checks[1..].iter().all(|c| c.ci != Ci::Fail));
    let skipped = checks.iter().filter(|c| c.label == "skipped").count();
    assert_eq!(skipped, 6);
    assert!(checks.iter().rev().take(skipped).all(|c| c.ci == Ci::None), "skipped checks sort last");
    // The `CodeQL` check has an empty workflow name.
    assert_eq!(checks.iter().find(|c| c.name == "CodeQL").unwrap().group, None);

    assert_eq!((comments.items.len(), comments.earlier), (5, 0));
    assert_eq!(comments.items[0].author, "github-actions");
    assert_eq!(comments.items[4].author, "iTrooz");
}

#[test]
fn glab_view_fixtures_parse() {
    let meta = gitlab::parse_mr_view(GL_VIEW).unwrap();
    assert_eq!(meta.author, "ranehebden");
    assert_eq!(
        (meta.head_branch.as_str(), meta.base_branch.as_str()),
        ("feat/theme-color-overrides", "main")
    );
    assert_eq!(meta.labels, ["type::feature"]);
    assert_eq!(meta.changed_files.as_deref(), Some("12"));
    assert_eq!((meta.additions, meta.deletions), (None, None));
    assert_eq!(meta.review_decision, None); // discussions_not_resolved
    let reviewers: Vec<&str> = meta.reviewers.iter().map(|r| r.login.as_str()).collect();
    assert_eq!(reviewers, ["gitlab-bot", "jay_mccure", "GitLabDuo"]);
    assert!(meta.body.starts_with("## What"));

    assert_eq!(gitlab::latest_pipeline(GL_MR_PIPES).unwrap(), Some((2997849276, Some(34675721))));
    assert_eq!(gitlab::latest_pipeline("[]").unwrap(), None);

    let jobs = gitlab::parse_jobs(GL_JOBS).unwrap();
    assert_eq!(jobs.len(), 18);
    // Pipeline order (oldest job first) within each severity.
    assert_eq!(jobs[0].name, "gitlab-advanced-sast");
    assert_eq!(jobs[0].group.as_deref(), Some("application-security-testing"));
    assert_eq!(jobs[0].ci, Ci::Pass);
    let manual: Vec<&str> = jobs.iter().filter(|j| j.label == "manual").map(|j| j.name.as_str()).collect();
    assert_eq!(manual, ["govulncheck", "review-docs-deploy", "review-docs-cleanup"]);
    assert!(jobs.iter().rev().take(3).all(|j| j.ci == Ci::None));

    let notes = gitlab::parse_notes(GL_NOTES).unwrap();
    let authors: Vec<&str> = notes.items.iter().map(|c| c.author.as_str()).collect();
    assert_eq!(authors, ["gitlab-bot", "jay_mccure"], "system and diff notes dropped, oldest first");
    assert_eq!((notes.earlier, notes.more_unknown), (0, false));
}

#[test]
fn glab_notes_keep_latest_and_flag_a_full_page() {
    // Every 4th note is a system note, every 4th + 1 an inline diff note.
    let notes: Vec<String> = (0..100)
        .map(|i| {
            let kind = if i % 4 == 1 { r#""DiffNote""# } else { "null" };
            format!(
                r#"{{"body":"n{i}","author":{{"username":"u"}},"created_at":"","system":{},"type":{kind}}}"#,
                i % 4 == 3
            )
        })
        .collect();
    let c = gitlab::parse_notes(&format!("[{}]", notes.join(","))).unwrap();
    // 50 conversation notes, newest first in the page → the 20 newest, oldest first.
    assert_eq!((c.items.len(), c.earlier, c.more_unknown), (MAX_COMMENTS, 30, true));
    assert_eq!(c.items.last().unwrap().body, "n0");
    assert_eq!(c.items[0].body, "n38");
    // Only even notes (i % 4 == 0 or 2) are conversation notes.
    assert!(c.items.iter().all(|n| n.body[1..].parse::<usize>().unwrap().is_multiple_of(2)));
}

#[test]
fn github_details_use_host_qualified_repo() {
    let runner = ScriptedRunner::new()
        .on(&["gh", "pr", "view", "13013", "-R", "ghe.corp/cli/cli", "--json"], Ok(Output::ok(GH_VIEW)));
    let d = GitHub { runner: &runner }.pr_details(&repo("ghe.corp", "cli/cli"), Path::new("/r"), 13013);
    assert!(d.meta.is_ok() && d.checks.is_ok() && d.comments.is_ok());
    assert_eq!(runner.calls().len(), 1);
    assert_eq!(runner.calls()[0].last().map(String::as_str), Some(VIEW_FIELDS));
}

#[test]
fn github_details_failure_fails_every_section_with_its_fix() {
    let runner = ScriptedRunner::new().on(&["gh"], Err(RunError::NotFound("gh".into())));
    let d = GitHub { runner: &runner }.pr_details(&repo("github.com", "a/b"), Path::new("/r"), 1);
    assert_eq!(d.meta.as_ref().unwrap_err().message, "gh is not installed");
    assert_eq!(d.checks.as_ref().unwrap_err(), d.meta.as_ref().unwrap_err());
    assert_eq!(d.comments.as_ref().unwrap_err(), d.meta.as_ref().unwrap_err());
}

const PIPELINE: &str = "projects/34675721/pipelines/2997849276";
/// One failed downstream trigger job.
const GL_BRIDGES: &str =
    r#"[{"id":17686929999,"name":"trigger:docs","stage":"deploy","status":"failed","allow_failure":false}]"#;

fn glab_api(route: &str) -> [&str; 5] {
    ["glab", "api", "--hostname", "gitlab.corp", route]
}

fn job_json(id: u64, status: &str) -> String {
    format!(r#"{{"id":{id},"name":"job{id}","stage":"test","status":"{status}"}}"#)
}

/// A runner for the MR view, notes and pipelines, with no job routes.
fn gitlab_base(notes: Result<Output, RunError>, pipes: &str) -> ScriptedRunner {
    ScriptedRunner::new()
        .on(
            &["glab", "mr", "view", "3956", "-R", "https://gitlab.corp/grp/sub/cli", "-F", "json"],
            Ok(Output::ok(GL_VIEW)),
        )
        .on(
            &[
                "glab",
                "api",
                "--hostname",
                "gitlab.corp",
                "projects/grp%2Fsub%2Fcli/merge_requests/3956/notes?sort=desc&per_page=100",
            ],
            notes,
        )
        .on(
            &[
                "glab",
                "api",
                "--hostname",
                "gitlab.corp",
                "projects/grp%2Fsub%2Fcli/merge_requests/3956/pipelines",
            ],
            Ok(Output::ok(pipes)),
        )
}

fn gitlab_runner(notes: Result<Output, RunError>, pipes: &str) -> ScriptedRunner {
    // Jobs and bridges are read from the project the pipeline ran in.
    gitlab_base(notes, pipes)
        .on(&glab_api(&format!("{PIPELINE}/jobs?per_page=100&page=1")), Ok(Output::ok(GL_JOBS)))
        .on(&glab_api(&format!("{PIPELINE}/bridges?per_page=100")), Ok(Output::ok(GL_BRIDGES)))
}

#[test]
fn gitlab_details_on_self_hosted_host() {
    let runner = gitlab_runner(Ok(Output::ok(GL_NOTES)), GL_MR_PIPES);
    let d = GitLab { runner: &runner }.pr_details(&repo("gitlab.corp", "grp/sub/cli"), Path::new("/r"), 3956);
    assert_eq!(d.meta.unwrap().author, "ranehebden");
    let checks = d.checks.unwrap();
    assert_eq!((checks.items.len(), checks.more_unknown), (19, false), "18 jobs + 1 bridge");
    let bridge = &checks.items[0];
    assert_eq!(
        (bridge.name.as_str(), bridge.group.as_deref(), bridge.ci),
        ("trigger:docs", Some("deploy"), Ci::Fail)
    );
    assert_eq!(d.comments.unwrap().items.len(), 2);
    assert_eq!(runner.calls().len(), 5);
}

#[test]
fn gitlab_jobs_are_paged_so_a_late_failure_shows() {
    // Newest first: page 1 holds 100 passing jobs, page 2 the older failed one.
    let page1: Vec<String> = (0..100).map(|i| job_json(200 - i, "success")).collect();
    let runner = gitlab_base(Ok(Output::ok(GL_NOTES)), GL_MR_PIPES)
        .on(
            &glab_api(&format!("{PIPELINE}/jobs?per_page=100&page=1")),
            Ok(Output::ok(&format!("[{}]", page1.join(",")))),
        )
        .on(
            &glab_api(&format!("{PIPELINE}/jobs?per_page=100&page=2")),
            Ok(Output::ok(&format!("[{}]", job_json(50, "failed")))),
        )
        // A refused bridges call leaves the jobs intact.
        .on(&glab_api(&format!("{PIPELINE}/bridges?per_page=100")), Ok(Output::fail("403 Forbidden")));
    let d = GitLab { runner: &runner }.pr_details(&repo("gitlab.corp", "grp/sub/cli"), Path::new("/r"), 3956);
    let checks = d.checks.unwrap();
    assert_eq!((checks.items.len(), checks.more_unknown), (101, false));
    assert_eq!((checks.items[0].name.as_str(), checks.items[0].ci), ("job50", Ci::Fail));
    // Pipeline order (oldest first) among the passing jobs.
    assert_eq!(checks.items[1].name, "job101");
    assert_eq!(runner.calls().len(), 6);
}

#[test]
fn gitlab_jobs_stop_at_the_page_cap() {
    let mut runner = gitlab_base(Ok(Output::ok(GL_NOTES)), GL_MR_PIPES);
    for page in 1..=gitlab::MAX_JOB_PAGES as u64 {
        let jobs: Vec<String> = (0..100).map(|i| job_json(page * 1000 + i, "success")).collect();
        runner = runner.on(
            &glab_api(&format!("{PIPELINE}/jobs?per_page=100&page={page}")),
            Ok(Output::ok(&format!("[{}]", jobs.join(",")))),
        );
    }
    runner = runner.on(&glab_api(&format!("{PIPELINE}/bridges?per_page=100")), Ok(Output::ok("[]")));
    let d = GitLab { runner: &runner }.pr_details(&repo("gitlab.corp", "grp/sub/cli"), Path::new("/r"), 3956);
    let checks = d.checks.unwrap();
    assert_eq!((checks.items.len(), checks.more_unknown), (1000, true));
    assert!(!runner.calls().iter().any(|c| c.iter().any(|a| a.ends_with("page=11"))));
}

#[test]
fn gitlab_failed_jobs_call_fails_the_checks_section() {
    let runner = gitlab_base(Ok(Output::ok(GL_NOTES)), GL_MR_PIPES).on(
        &glab_api(&format!("{PIPELINE}/jobs?per_page=100&page=1")),
        Ok(Output::fail("500 Internal Server Error")),
    );
    let d = GitLab { runner: &runner }.pr_details(&repo("gitlab.corp", "grp/sub/cli"), Path::new("/r"), 3956);
    assert!(d.checks.is_err() && d.meta.is_ok() && d.comments.is_ok());
}

#[test]
fn gitlab_partial_failure_keeps_loaded_sections() {
    let unauth = Ok(Output::fail("GET https://gitlab.corp/api/v4/...: 401 {message: 401 Unauthorized}"));
    let runner = gitlab_runner(unauth, GL_MR_PIPES);
    let d = GitLab { runner: &runner }.pr_details(&repo("gitlab.corp", "grp/sub/cli"), Path::new("/r"), 3956);
    assert!(d.meta.is_ok() && d.checks.is_ok());
    let e = d.comments.unwrap_err();
    assert_eq!(e.message, "glab is not authenticated for gitlab.corp");
    assert_eq!(e.fix.as_deref(), Some("glab auth login --hostname gitlab.corp"));
}

#[test]
fn gitlab_mr_without_pipeline_has_no_checks() {
    let runner = gitlab_runner(Ok(Output::ok(GL_NOTES)), "[]");
    let d = GitLab { runner: &runner }.pr_details(&repo("gitlab.corp", "grp/sub/cli"), Path::new("/r"), 3956);
    assert_eq!(d.checks.unwrap(), Checks::default());
    assert_eq!(runner.calls().len(), 3, "no jobs or bridges call without a pipeline");
}
