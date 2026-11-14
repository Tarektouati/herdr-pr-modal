//! Provider-neutral PR model, grouping and filtering.

use crate::provider::ProviderError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ci {
    Pass,
    Fail,
    Pending,
    /// No checks reported.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    /// GitHub PR number or GitLab MR iid.
    pub number: u64,
    pub title: String,
    pub author: String,
    /// Head branch name in the head repository.
    pub head_branch: String,
    /// The head lives in another repository (a fork).
    pub is_fork: bool,
    pub is_draft: bool,
    pub ci: Ci,
    /// The viewer is a requested reviewer (directly or through a team).
    pub review_requested: bool,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// Login of the authenticated user, used for grouping.
    pub viewer: String,
    pub prs: Vec<Pr>,
}

/// Read-only details for the preview view. Each section loads (or fails)
/// on its own, so a partial fetch still renders what it got. Held in memory
/// only, never written to the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDetails {
    /// Meta and description come from the same call on both forges.
    pub meta: Result<PrMeta, ProviderError>,
    pub checks: Result<Checks, ProviderError>,
    pub comments: Result<Comments, ProviderError>,
}

impl PrDetails {
    /// Every section failed with the same error (one failed call).
    pub fn failed(e: ProviderError) -> Self {
        Self { meta: Err(e.clone()), checks: Err(e.clone()), comments: Err(e) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrMeta {
    pub author: String,
    pub head_branch: String,
    pub base_branch: String,
    pub is_draft: bool,
    /// `approved`, `changes requested` or `review required`.
    pub review_decision: Option<String>,
    pub reviewers: Vec<Reviewer>,
    pub labels: Vec<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    /// A string because GitLab reports e.g. `1000+`.
    pub changed_files: Option<String>,
    /// ISO 8601 timestamps as the forge sends them.
    pub created_at: String,
    pub updated_at: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewer {
    pub login: String,
    /// `approved`, `changes requested`, `commented`, `pending`…; `None` when
    /// the forge does not say.
    pub state: Option<String>,
}

/// One CI check run, status context or pipeline job. `Ci::None` means
/// skipped, neutral or manual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// Workflow (GitHub) or stage (GitLab).
    pub group: Option<String>,
    pub ci: Ci,
    /// Raw status, lowercased, e.g. `failure`, `skipped`, `manual`.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Checks {
    /// Failures first (see `sort_checks`).
    pub items: Vec<Check>,
    /// The forge has more checks than were fetched.
    pub more_unknown: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Comments {
    /// Oldest → newest, at most `MAX_COMMENTS`.
    pub items: Vec<Comment>,
    /// Comments before `items` that are not shown.
    pub earlier: usize,
    /// The forge returned a full page, so there may be more than `earlier`.
    pub more_unknown: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub author: String,
    pub created_at: String,
    pub body: String,
}

pub const MAX_COMMENTS: usize = 20;

/// Keep the latest `MAX_COMMENTS` of `all` (oldest → newest).
pub fn latest_comments(mut all: Vec<Comment>) -> Comments {
    let earlier = all.len().saturating_sub(MAX_COMMENTS);
    let items = all.split_off(earlier);
    Comments { items, earlier, more_unknown: false }
}

/// Failures first, then pending, pass and skipped; stable within each.
pub fn sort_checks(checks: &mut [Check]) {
    checks.sort_by_key(|c| match c.ci {
        Ci::Fail => 0,
        Ci::Pending => 1,
        Ci::Pass => 2,
        Ci::None => 3,
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Mine,
    ReviewRequested,
    Others,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Mine => "mine",
            Group::ReviewRequested => "review requested",
            Group::Others => "others",
        }
    }
}

pub fn group_of(pr: &Pr, viewer: &str) -> Group {
    if !viewer.is_empty() && pr.author.eq_ignore_ascii_case(viewer) {
        Group::Mine
    } else if pr.review_requested {
        Group::ReviewRequested
    } else {
        Group::Others
    }
}

/// Filter: every whitespace-separated term must match the number (`12`,
/// `#12`), title, head branch or author, case-insensitively.
pub fn matches(pr: &Pr, query: &str) -> bool {
    query.split_whitespace().all(|term| {
        let term = term.to_lowercase();
        if let Some(num) = term.strip_prefix('#') {
            return !num.is_empty() && pr.number.to_string().starts_with(num);
        }
        pr.number.to_string().starts_with(&term)
            || pr.title.to_lowercase().contains(&term)
            || pr.head_branch.to_lowercase().contains(&term)
            || pr.author.to_lowercase().contains(&term)
    })
}

/// PRs grouped for display, in `mine`, `review requested`, `others` order,
/// filtered by `query`, empty groups dropped. Within a group the provider
/// order (newest first) is kept. Items are indexes into `listing.prs`.
pub fn grouped(listing: &Listing, query: &str) -> Vec<(Group, Vec<usize>)> {
    let mut groups: Vec<(Group, Vec<usize>)> =
        [Group::Mine, Group::ReviewRequested, Group::Others].map(|g| (g, Vec::new())).into();
    for (i, pr) in listing.prs.iter().enumerate() {
        if !matches(pr, query) {
            continue;
        }
        let g = group_of(pr, &listing.viewer);
        groups.iter_mut().find(|(k, _)| *k == g).unwrap().1.push(i);
    }
    groups.retain(|(_, items)| !items.is_empty());
    groups
}

/// Lowercase, ASCII alphanumerics kept, everything else collapsed to `-`,
/// capped at `max` chars. Empty input gives `pr`.
pub fn slug(s: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_matches('-').chars().take(max).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "pr".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn pr(number: u64, author: &str, rr: bool) -> Pr {
        Pr {
            number,
            title: format!("Title {number}"),
            author: author.into(),
            head_branch: format!("feat/b{number}"),
            is_fork: false,
            is_draft: false,
            ci: Ci::None,
            review_requested: rr,
            url: String::new(),
        }
    }

    fn listing() -> Listing {
        Listing {
            viewer: "me".into(),
            prs: vec![pr(1, "alice", false), pr(2, "Me", true), pr(3, "bob", true), pr(4, "me", false)],
        }
    }

    #[test]
    fn groups_in_fixed_order_and_mine_wins_over_review_requested() {
        let g = grouped(&listing(), "");
        assert_eq!(
            g,
            vec![(Group::Mine, vec![1, 3]), (Group::ReviewRequested, vec![2]), (Group::Others, vec![0])]
        );
    }

    #[test]
    fn empty_groups_are_hidden() {
        let g = grouped(&listing(), "bob");
        assert_eq!(g, vec![(Group::ReviewRequested, vec![2])]);
        assert!(grouped(&listing(), "zzz").is_empty());
    }

    #[test]
    fn no_viewer_means_nothing_is_mine() {
        let mut l = listing();
        l.viewer.clear();
        assert!(grouped(&l, "").iter().all(|(g, _)| *g != Group::Mine));
    }

    #[test]
    fn filter_by_number_title_branch_author() {
        let mut p = pr(1234, "Alice", false);
        p.title = "Fix Crash on start".into();
        p.head_branch = "fix/crash".into();
        for q in ["123", "#12", "crash", "CRASH", "fix/cr", "alice", "alice crash", ""] {
            assert!(matches(&p, q), "{q:?} should match");
        }
        for q in ["#2", "#", "bob", "alice bob", "234"] {
            assert!(!matches(&p, q), "{q:?} should not match");
        }
    }

    #[test]
    fn latest_comments_caps_and_counts_earlier() {
        let c =
            |i: usize| Comment { author: format!("u{i}"), created_at: String::new(), body: String::new() };
        let few = latest_comments((0..3).map(c).collect());
        assert_eq!((few.items.len(), few.earlier), (3, 0));
        let many = latest_comments((0..25).map(c).collect());
        assert_eq!((many.items.len(), many.earlier), (MAX_COMMENTS, 5));
        assert_eq!(many.items[0].author, "u5");
        assert_eq!(many.items.last().unwrap().author, "u24");
    }

    #[test]
    fn checks_sort_by_severity_stably() {
        let c = |n: &str, ci| Check { name: n.into(), group: None, ci, label: String::new() };
        let mut v =
            vec![c("a", Ci::Pass), c("b", Ci::None), c("c", Ci::Fail), c("d", Ci::Pass), c("e", Ci::Pending)];
        sort_checks(&mut v);
        let names: Vec<&str> = v.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["c", "e", "a", "d", "b"]);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Feature/Add OAuth!!", 40), "feature-add-oauth");
        assert_eq!(slug("///", 40), "pr");
        assert_eq!(slug("abc-def-ghi", 5), "abc-d");
        assert_eq!(slug("abcd-efgh", 5), "abcd");
    }
}
