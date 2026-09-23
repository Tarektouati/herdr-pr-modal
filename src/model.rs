//! Provider-neutral PR model, grouping and filtering.

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
    fn slugs() {
        assert_eq!(slug("Feature/Add OAuth!!", 40), "feature-add-oauth");
        assert_eq!(slug("///", 40), "pr");
        assert_eq!(slug("abc-def-ghi", 5), "abc-d");
        assert_eq!(slug("abcd-efgh", 5), "abcd");
    }
}
