//! Select-a-PR flow against real git repositories in a temp dir. `git` runs
//! for real; `herdr` calls are recorded and answered, because creating the
//! worktree workspace is Herdr's job.

use herdr_pr_modal::checkout::{self, Opened};
use herdr_pr_modal::cmd::{Output, RunError, Runner, SystemRunner};
use herdr_pr_modal::config::Config;
use herdr_pr_modal::git::{self, BranchState};
use herdr_pr_modal::model::{Ci, Listing, Pr};
use herdr_pr_modal::provider::{Provider, ProviderKind, RepoRef, github::GitHub};
use herdr_pr_modal::session::{self, RepoInfo};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Real git, recorded herdr.
#[derive(Default)]
struct Hybrid {
    calls: Mutex<Vec<Vec<String>>>,
}

impl Hybrid {
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
    fn herdr_calls(&self) -> Vec<Vec<String>> {
        self.calls().into_iter().filter(|c| c[0] == "herdr").collect()
    }
}

impl Runner for Hybrid {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Output, RunError> {
        let argv = std::iter::once(program).chain(args.iter().copied()).map(String::from).collect();
        self.calls.lock().unwrap().push(argv);
        if program == "herdr" {
            return Ok(Output::ok(r#"{"id":"cli","result":{"root_pane":{"pane_id":"w9:p1"}}}"#));
        }
        SystemRunner.run(program, args, cwd)
    }
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    upstream: PathBuf,
    work: PathBuf,
}

/// upstream: main, `feature` (same-repo PR) and `refs/pull/7/head` (a fork
/// PR head that exists on no branch). work: a clone of upstream.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let upstream = root.join("upstream");
    std::fs::create_dir(&upstream).unwrap();
    sh(&upstream, &["init", "-q"]);
    sh(&upstream, &["commit", "-q", "--allow-empty", "-m", "base"]);
    sh(&upstream, &["checkout", "-q", "-b", "feature"]);
    sh(&upstream, &["commit", "-q", "--allow-empty", "-m", "feature 1"]);
    sh(&upstream, &["checkout", "-q", "-b", "fork-head", "main"]);
    sh(&upstream, &["commit", "-q", "--allow-empty", "-m", "fork work"]);
    let fork_sha = sh(&upstream, &["rev-parse", "HEAD"]);
    sh(&upstream, &["update-ref", "refs/pull/7/head", &fork_sha]);
    sh(&upstream, &["checkout", "-q", "main"]);
    sh(&upstream, &["branch", "-q", "-D", "fork-head"]);
    sh(&root, &["clone", "-q", upstream.to_str().unwrap(), "work"]);
    Fixture { _tmp: tmp, upstream, work: root.join("work") }
}

fn pr(number: u64, branch: &str, fork: bool) -> Pr {
    Pr {
        number,
        title: format!("PR {number}"),
        author: "a".into(),
        head_branch: branch.into(),
        is_fork: fork,
        is_draft: false,
        ci: Ci::None,
        review_requested: false,
        url: String::new(),
    }
}

fn open(f: &Fixture, runner: &Hybrid, pr: &Pr, cfg: &Config) -> Result<Opened, String> {
    let plan = GitHub { runner }.fetch_head(pr, &cfg.remote);
    checkout::open_pr(&checkout::Request {
        runner,
        herdr_bin: "herdr",
        repo_root: &f.work,
        cfg,
        pr,
        plan: &plan,
    })
}

#[test]
fn same_repo_pr_is_fetched_into_tracking_branch_then_created() {
    let f = fixture();
    let runner = Hybrid::default();
    let opened = open(&f, &runner, &pr(3, "feature", false), &Config::default()).unwrap();
    assert_eq!(opened, Opened::Created { branch: "feature".into(), state: BranchState::Created });

    let want = sh(&f.upstream, &["rev-parse", "feature"]);
    assert_eq!(sh(&f.work, &["rev-parse", "feature"]), want);
    assert_eq!(sh(&f.work, &["rev-parse", "--abbrev-ref", "feature@{upstream}"]), "origin/feature");

    let herdr = runner.herdr_calls();
    assert_eq!(herdr.len(), 1);
    assert_eq!(
        herdr[0],
        [
            "herdr",
            "worktree",
            "create",
            "--cwd",
            f.work.to_str().unwrap(),
            "--branch",
            "feature",
            "--label",
            "#3 PR 3",
            "--focus"
        ]
    );
}

#[test]
fn fork_pr_is_fetched_from_pull_ref_into_pr_branch() {
    let f = fixture();
    let runner = Hybrid::default();
    let opened = open(&f, &runner, &pr(7, "their/Branch", true), &Config::default()).unwrap();
    assert_eq!(opened, Opened::Created { branch: "pr/7-their-branch".into(), state: BranchState::Created });
    assert_eq!(
        sh(&f.work, &["rev-parse", "pr/7-their-branch"]),
        sh(&f.upstream, &["rev-parse", "refs/pull/7/head"])
    );
}

#[test]
fn existing_worktree_is_switched_to_without_fetching() {
    let f = fixture();
    sh(&f.work, &["branch", "-q", "--track", "feature", "origin/feature"]);
    let wt = f.work.parent().unwrap().join("wt-feature");
    sh(&f.work, &["worktree", "add", "-q", wt.to_str().unwrap(), "feature"]);

    let runner = Hybrid::default();
    let opened = open(&f, &runner, &pr(3, "feature", false), &Config::default()).unwrap();
    assert_eq!(opened, Opened::Switched { path: wt.clone() });
    assert!(!runner.calls().iter().any(|c| c.get(1).map(String::as_str) == Some("fetch")), "must not fetch");
    let herdr = runner.herdr_calls();
    assert_eq!(
        herdr,
        [[
            "herdr",
            "worktree",
            "open",
            "--cwd",
            f.work.to_str().unwrap(),
            "--path",
            wt.to_str().unwrap(),
            "--focus"
        ]]
    );
}

#[test]
fn fork_worktree_from_an_earlier_slug_still_counts_as_existing() {
    let f = fixture();
    sh(&f.work, &["fetch", "-q", "origin", "refs/pull/7/head:refs/heads/pr/7-old-name"]);
    let wt = f.work.parent().unwrap().join("wt-pr7");
    sh(&f.work, &["worktree", "add", "-q", wt.to_str().unwrap(), "pr/7-old-name"]);
    let runner = Hybrid::default();
    let opened = open(&f, &runner, &pr(7, "renamed", true), &Config::default()).unwrap();
    assert_eq!(opened, Opened::Switched { path: wt });
}

#[test]
fn diverged_local_branch_is_kept_and_behind_branch_fast_forwards() {
    let f = fixture();
    // Behind: local feature at main's commit, not checked out anywhere.
    sh(&f.work, &["branch", "-q", "feature", "main"]);
    let runner = Hybrid::default();
    let plan = GitHub { runner: &runner }.fetch_head(&pr(3, "feature", false), "origin");
    assert_eq!(git::fetch_into_branch(&runner, &f.work, &plan).unwrap(), BranchState::FastForwarded);
    assert_eq!(sh(&f.work, &["rev-parse", "feature"]), sh(&f.upstream, &["rev-parse", "feature"]));
    assert_eq!(git::fetch_into_branch(&runner, &f.work, &plan).unwrap(), BranchState::UpToDate);

    // Diverged: a local commit the PR head lacks is never rewritten.
    sh(&f.work, &["checkout", "-q", "feature"]);
    sh(&f.work, &["commit", "-q", "--allow-empty", "-m", "local only"]);
    let local = sh(&f.work, &["rev-parse", "HEAD"]);
    sh(&f.work, &["checkout", "-q", "main"]);
    sh(&f.upstream, &["checkout", "-q", "feature"]);
    sh(&f.upstream, &["commit", "-q", "--allow-empty", "-m", "feature 2"]);
    sh(&f.upstream, &["checkout", "-q", "main"]);
    assert_eq!(git::fetch_into_branch(&runner, &f.work, &plan).unwrap(), BranchState::KeptDiverged);
    assert_eq!(sh(&f.work, &["rev-parse", "feature"]), local);
}

#[test]
fn fetch_failure_surfaces_git_error_and_skips_herdr() {
    let f = fixture();
    let runner = Hybrid::default();
    let err = open(&f, &runner, &pr(4, "deleted-branch", false), &Config::default()).unwrap_err();
    assert!(err.starts_with("git fetch:"), "{err}");
    assert!(err.contains("deleted-branch"), "{err}");
    assert!(runner.herdr_calls().is_empty());
}

#[test]
fn post_create_command_runs_in_new_root_pane() {
    let f = fixture();
    let runner = Hybrid::default();
    let cfg = Config { post_create_command: Some("pnpm install".into()), ..Config::default() };
    open(&f, &runner, &pr(3, "feature", false), &cfg).unwrap();
    assert_eq!(runner.herdr_calls().last().unwrap(), &["herdr", "pane", "run", "w9:p1", "pnpm install"]);
}

#[test]
fn worktree_marks_and_repo_resolution() {
    let f = fixture();
    sh(&f.work, &["branch", "-q", "--track", "feature", "origin/feature"]);
    let wt = f.work.parent().unwrap().join("wt-feature");
    sh(&f.work, &["worktree", "add", "-q", wt.to_str().unwrap(), "feature"]);
    sh(&f.work, &["remote", "set-url", "origin", "git@github.com:acme/app.git"]);

    let runner = SystemRunner;
    let sub = f.work.join("nested");
    std::fs::create_dir(&sub).unwrap();
    let info = session::resolve_repo(&runner, sub.to_str(), &Config::default(), "cfg").unwrap();
    assert_eq!(
        info,
        RepoInfo {
            root: f.work.clone(),
            repo: RepoRef { host: "github.com".into(), path: "acme/app".into() },
            kind: ProviderKind::GitHub
        }
    );
    let listing = Listing { viewer: "a".into(), prs: vec![pr(3, "feature", false), pr(8, "other", false)] };
    let marks = session::worktree_marks(&runner, &info, &Config::default(), &listing);
    assert_eq!(marks.into_iter().collect::<Vec<_>>(), [3]);
}

#[test]
fn not_a_repo_and_unknown_host_are_clear_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let err = session::resolve_repo(&SystemRunner, dir.to_str(), &Config::default(), "cfg").unwrap_err();
    assert_eq!(err.message, format!("not a git repository: {}", dir.display()));

    let f = fixture();
    sh(&f.work, &["remote", "set-url", "origin", "https://code.corp.io/team/app.git"]);
    let err =
        session::resolve_repo(&SystemRunner, f.work.to_str(), &Config::default(), "/cfg.toml").unwrap_err();
    assert_eq!(err.fix.as_deref(), Some("set provider = \"github\" or \"gitlab\" in /cfg.toml"));

    let err = session::resolve_repo(&SystemRunner, None, &Config::default(), "cfg").unwrap_err();
    assert!(err.message.contains("origin pane"));
}
