//! PR providers. All auth goes through the `gh` / `glab` CLIs; this plugin
//! never reads, stores or sends a token.

pub mod github;
pub mod gitlab;

use crate::cmd::{Output, RunError, Runner};
use crate::config::ProviderChoice;
use crate::model::{Listing, Pr, slug};
use std::path::Path;

/// A repository on a forge host, parsed from a git remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub host: String,
    /// `owner/repo`, or `group/sub/repo` on GitLab.
    pub path: String,
}

impl RepoRef {
    /// `host/path`, the `-R` form both CLIs accept for any host.
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.host, self.path)
    }
}

/// Parse `git@host:o/r.git`, `ssh://git@host:22/o/r.git`,
/// `https://host/o/r(.git)` and `git://host/o/r`.
pub fn parse_remote_url(url: &str) -> Option<RepoRef> {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(scheme, "ssh" | "https" | "http" | "git" | "git+ssh" | "ssh+git") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host, path)
    } else {
        // scp-like: [user@]host:path
        let (left, path) = url.split_once(':')?;
        if left.contains('/') {
            return None;
        }
        (left.rsplit('@').next()?, path)
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.split('/').filter(|s| !s.is_empty()).count() < 2 {
        return None;
    }
    Some(RepoRef { host: host.to_lowercase(), path: path.to_string() })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    GitHub,
    GitLab,
}

/// Config override wins; otherwise the host name decides. An unknown
/// self-hosted host needs the override.
pub fn detect(host: &str, choice: ProviderChoice) -> Option<ProviderKind> {
    match choice {
        ProviderChoice::Github => Some(ProviderKind::GitHub),
        ProviderChoice::Gitlab => Some(ProviderKind::GitLab),
        ProviderChoice::Auto if host.contains("gitlab") => Some(ProviderKind::GitLab),
        ProviderChoice::Auto if host.contains("github") => Some(ProviderKind::GitHub),
        ProviderChoice::Auto => None,
    }
}

/// One-row error for the modal: what went wrong and the command that fixes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub message: String,
    pub fix: Option<String>,
}

impl ProviderError {
    pub fn new(message: impl Into<String>, fix: Option<String>) -> Self {
        Self { message: message.into(), fix }
    }
}

/// Map a failed CLI call to a clear error with its fix.
pub(crate) fn cli_error(cli: &str, host: &str, result: Result<Output, RunError>) -> ProviderError {
    match result {
        Err(RunError::NotFound(_)) => ProviderError::new(
            format!("{cli} is not installed"),
            Some(format!("brew install {cli}   (or see {})", install_url(cli))),
        ),
        Err(RunError::Io(e)) => ProviderError::new(e, None),
        Ok(out) => {
            let text = format!("{}\n{}", out.stderr, out.stdout).to_lowercase();
            let auth = [
                "auth login",
                "not logged in",
                "authentication",
                "401",
                "unauthorized",
                "bad credentials",
                "no token found",
            ];
            let network = [
                "could not resolve host",
                "no such host",
                "dial tcp",
                "timeout",
                "timed out",
                "connection refused",
                "network is unreachable",
                "tls handshake",
            ];
            if network.iter().any(|n| text.contains(n)) {
                ProviderError::new(
                    format!("network error reaching {host}: {}", out.error_line()),
                    Some("check your connection or VPN, then press r".into()),
                )
            } else if auth.iter().any(|a| text.contains(a)) {
                ProviderError::new(
                    format!("{cli} is not authenticated for {host}"),
                    Some(format!("{cli} auth login --hostname {host}")),
                )
            } else {
                ProviderError::new(format!("{cli}: {}", out.error_line()), None)
            }
        }
    }
}

fn install_url(cli: &str) -> &'static str {
    if cli == "glab" { "https://gitlab.com/gitlab-org/cli" } else { "https://cli.github.com" }
}

/// How to bring a PR head into a local branch. Pure, so branch resolution is
/// testable without git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPlan {
    pub remote: String,
    /// Ref on the remote to fetch.
    pub source_ref: String,
    /// Local branch the worktree checks out.
    pub local_branch: String,
    /// Same-repo heads also update `refs/remotes/<remote>/<branch>` and get
    /// upstream tracking; fork heads have no upstream on this remote.
    pub tracking: Option<String>,
}

impl FetchPlan {
    /// Branches that count as "this PR already has a worktree": the planned
    /// local branch, plus any `pr/<n>-…` branch from an earlier run (the head
    /// branch may have been renamed since).
    pub fn is_pr_branch(&self, pr_number: u64, branch: &str) -> bool {
        if branch == self.local_branch {
            return true;
        }
        self.tracking.is_none()
            && (branch == format!("pr/{pr_number}") || branch.starts_with(&format!("pr/{pr_number}-")))
    }
}

pub(crate) fn plan_for(pr: &Pr, remote: &str, fork_ref: String) -> FetchPlan {
    if pr.is_fork {
        FetchPlan {
            remote: remote.to_string(),
            source_ref: fork_ref,
            local_branch: format!("pr/{}-{}", pr.number, slug(&pr.head_branch, 40)),
            tracking: None,
        }
    } else {
        FetchPlan {
            remote: remote.to_string(),
            source_ref: format!("refs/heads/{}", pr.head_branch),
            local_branch: pr.head_branch.clone(),
            tracking: Some(format!("refs/remotes/{remote}/{}", pr.head_branch)),
        }
    }
}

pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn cli(&self) -> &'static str;
    fn list_open_prs(&self, repo: &RepoRef, cwd: &Path) -> Result<Listing, ProviderError>;
    fn fetch_head(&self, pr: &Pr, remote: &str) -> FetchPlan;
}

pub fn for_kind<'a>(kind: ProviderKind, runner: &'a dyn Runner) -> Box<dyn Provider + 'a> {
    match kind {
        ProviderKind::GitHub => Box::new(github::GitHub { runner }),
        ProviderKind::GitLab => Box::new(gitlab::GitLab { runner }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Ci;

    #[test]
    fn remote_url_forms() {
        let cases = [
            ("git@github.com:cli/cli.git", "github.com", "cli/cli"),
            ("https://github.com/cli/cli", "github.com", "cli/cli"),
            ("https://github.com/cli/cli.git/", "github.com", "cli/cli"),
            ("ssh://git@ghe.corp.example:2222/team/app.git", "ghe.corp.example", "team/app"),
            ("https://user:pw@gitlab.com/group/sub/repo.git", "gitlab.com", "group/sub/repo"),
            ("git@GitLab.Example.com:a/b", "gitlab.example.com", "a/b"),
        ];
        for (url, host, path) in cases {
            let r = parse_remote_url(url).unwrap_or_else(|| panic!("{url}"));
            assert_eq!((r.host.as_str(), r.path.as_str()), (host, path), "{url}");
        }
        for bad in ["/local/path/repo", "file:///x/y", "github.com", "git@host:repo"] {
            assert_eq!(parse_remote_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn detection_and_override() {
        assert_eq!(detect("github.com", ProviderChoice::Auto), Some(ProviderKind::GitHub));
        assert_eq!(detect("gitlab.corp.io", ProviderChoice::Auto), Some(ProviderKind::GitLab));
        assert_eq!(detect("code.corp.io", ProviderChoice::Auto), None);
        assert_eq!(detect("code.corp.io", ProviderChoice::Gitlab), Some(ProviderKind::GitLab));
        assert_eq!(detect("github.com", ProviderChoice::Gitlab), Some(ProviderKind::GitLab));
    }

    fn pr(fork: bool) -> Pr {
        Pr {
            number: 42,
            title: "t".into(),
            author: "a".into(),
            head_branch: "Feat/Login Page".into(),
            is_fork: fork,
            is_draft: false,
            ci: Ci::None,
            review_requested: false,
            url: String::new(),
        }
    }

    #[test]
    fn same_repo_plan_tracks_remote_branch() {
        let plan = plan_for(&pr(false), "origin", "unused".into());
        assert_eq!(plan.source_ref, "refs/heads/Feat/Login Page");
        assert_eq!(plan.local_branch, "Feat/Login Page");
        assert_eq!(plan.tracking.as_deref(), Some("refs/remotes/origin/Feat/Login Page"));
        assert!(!plan.is_pr_branch(42, "pr/42-other"));
    }

    #[test]
    fn fork_plan_uses_pr_branch_and_matches_earlier_slugs() {
        let plan = plan_for(&pr(true), "origin", "refs/pull/42/head".into());
        assert_eq!(plan.local_branch, "pr/42-feat-login-page");
        assert_eq!(plan.tracking, None);
        assert!(plan.is_pr_branch(42, "pr/42-feat-login-page"));
        assert!(plan.is_pr_branch(42, "pr/42-renamed"));
        assert!(plan.is_pr_branch(42, "pr/42"));
        assert!(!plan.is_pr_branch(42, "pr/421-x"));
        assert!(!plan.is_pr_branch(42, "Feat/Login Page"));
    }

    #[test]
    fn cli_errors_carry_fix_commands() {
        let e = cli_error("gh", "ghe.io", Err(RunError::NotFound("gh".into())));
        assert_eq!(e.message, "gh is not installed");
        assert!(e.fix.unwrap().contains("brew install gh"));

        let e = cli_error(
            "gh",
            "ghe.io",
            Ok(Output::fail("To get started with GitHub CLI, please run:  gh auth login")),
        );
        assert_eq!(e.fix.as_deref(), Some("gh auth login --hostname ghe.io"));

        let e = cli_error(
            "glab",
            "gitlab.com",
            Ok(Output::fail("GET https://gitlab.com/api/v4/user: 401 {message: 401 Unauthorized}")),
        );
        assert_eq!(e.fix.as_deref(), Some("glab auth login --hostname gitlab.com"));

        let e = cli_error(
            "gh",
            "github.com",
            Ok(Output::fail(
                "error connecting to api.github.com: dial tcp: lookup api.github.com: no such host",
            )),
        );
        assert!(e.message.starts_with("network error reaching github.com"));

        let e =
            cli_error("gh", "github.com", Ok(Output::fail("\nGraphQL: Could not resolve to a Repository\n")));
        assert_eq!(e.message, "gh: GraphQL: Could not resolve to a Repository");
        assert_eq!(e.fix, None);
    }
}
