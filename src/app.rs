//! Modal state and key handling, independent of the terminal.

use crate::model::{Group, Listing, PrDetails, grouped};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};

/// One error line for the modal: what failed and the command that fixes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrRow {
    pub message: String,
    pub fix: Option<String>,
}

impl ErrRow {
    pub fn new(message: impl Into<String>, fix: Option<String>) -> Self {
        Self { message: message.into(), fix }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header(Group),
    /// Index into `listing.prs`.
    Pr(usize),
    Blank,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Refresh,
    /// Open the worktree for `listing.prs[i]`.
    Open(usize),
    /// Fetch preview details for `listing.prs[i]` (not cached, not in flight).
    Preview(usize),
    /// Refetch preview details for `listing.prs[i]`.
    RefreshDetail(usize),
}

/// Which screen the modal shows. The detail view is keyed by PR number, not
/// index, so it survives a list refresh that reorders `listing.prs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    List,
    Detail {
        number: u64,
        /// First visible body line; the renderer clamps it.
        scroll: usize,
    },
}

#[derive(Debug, Default)]
pub struct App {
    /// `owner/repo`, once resolved.
    pub repo_label: Option<String>,
    pub listing: Option<Listing>,
    /// A fetch is running (first load or refresh).
    pub loading: bool,
    /// Age of the cached listing being shown, if it came from cache.
    pub cached_age: Option<u64>,
    /// Unrecoverable (not a repo, no remote, unknown provider): any key closes.
    pub fatal: Option<ErrRow>,
    /// Load error with no data to show.
    pub load_error: Option<ErrRow>,
    /// Error shown above the list (failed refresh, failed open).
    pub banner: Option<ErrRow>,
    /// PR being opened; keys are ignored meanwhile.
    pub busy: Option<String>,
    pub query: String,
    pub filtering: bool,
    /// Position in `selectable()`.
    pub selected: usize,
    /// First visible body line.
    pub scroll: usize,
    /// PR numbers that already have a local worktree.
    pub worktree_prs: HashSet<u64>,
    pub view: View,
    /// Preview details by PR number, for the popup's lifetime only.
    pub details: HashMap<u64, PrDetails>,
    /// PR numbers whose details are being fetched.
    pub details_loading: HashSet<u64>,
}

impl App {
    pub fn new() -> Self {
        Self { loading: true, ..Self::default() }
    }

    pub fn rows(&self) -> Vec<Row> {
        let Some(listing) = &self.listing else { return Vec::new() };
        let mut rows = Vec::new();
        for (group, items) in grouped(listing, &self.query) {
            if !rows.is_empty() {
                rows.push(Row::Blank);
            }
            rows.push(Row::Header(group));
            rows.extend(items.into_iter().map(Row::Pr));
        }
        rows
    }

    /// PR indexes in display order.
    pub fn selectable(&self) -> Vec<usize> {
        self.rows().into_iter().filter_map(|r| if let Row::Pr(i) = r { Some(i) } else { None }).collect()
    }

    pub fn selected_pr(&self) -> Option<usize> {
        self.selectable().get(self.selected).copied()
    }

    fn move_by(&mut self, delta: isize) {
        let n = self.selectable().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    fn query_changed(&mut self) {
        self.selected = 0;
        self.scroll = 0;
    }

    /// Keep the selected line (and its group header when it is the first
    /// item) inside a viewport of `height` lines.
    pub fn ensure_visible(&mut self, height: usize) {
        let rows = self.rows();
        let height = height.max(1);
        let max_scroll = rows.len().saturating_sub(height);
        let Some(pr) = self.selected_pr() else {
            self.scroll = self.scroll.min(max_scroll);
            return;
        };
        let line = rows.iter().position(|r| *r == Row::Pr(pr)).unwrap_or(0);
        let top = if line > 0 && matches!(rows[line - 1], Row::Header(_)) { line - 1 } else { line };
        if top < self.scroll {
            self.scroll = top;
        } else if line >= self.scroll + height {
            self.scroll = line + 1 - height;
        }
        self.scroll = self.scroll.min(max_scroll);
    }

    pub fn handle_key(&mut self, key: KeyEvent, page: usize) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') && self.busy.is_none() {
            return Action::Quit;
        }
        if self.fatal.is_some() {
            return match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => Action::Quit,
                _ => Action::None,
            };
        }
        if self.busy.is_some() {
            return Action::None;
        }
        self.banner = None;
        let page = page.max(1) as isize;

        if matches!(self.view, View::Detail { .. }) {
            if self.detail_pr().is_some() {
                return self.handle_detail_key(key, page);
            }
            // The PR left the listing after a refresh.
            self.view = View::List;
        }

        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filtering = false;
                    if !self.query.is_empty() {
                        self.query.clear();
                        self.query_changed();
                    }
                }
                KeyCode::Enter => return self.open_selected(),
                KeyCode::Backspace => {
                    if self.query.pop().is_some() {
                        self.query_changed();
                    }
                }
                KeyCode::Char('u') if ctrl => {
                    self.query.clear();
                    self.query_changed();
                }
                KeyCode::Up => self.move_by(-1),
                KeyCode::Down => self.move_by(1),
                KeyCode::PageUp => self.move_by(-page),
                KeyCode::PageDown => self.move_by(page),
                KeyCode::Char(c) if !ctrl => {
                    self.query.push(c);
                    self.query_changed();
                }
                _ => {}
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Esc => {
                if self.query.is_empty() {
                    return Action::Quit;
                }
                self.query.clear();
                self.query_changed();
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Char('r') if !self.loading => return Action::Refresh,
            KeyCode::Enter => return self.open_selected(),
            KeyCode::Char('p') | KeyCode::Char('l') | KeyCode::Right => return self.preview_selected(),
            _ => {}
        }
        Action::None
    }

    fn handle_detail_key(&mut self, key: KeyEvent, page: isize) -> Action {
        let View::Detail { number, scroll } = &mut self.view else { return Action::None };
        let number = *number;
        let page = page as usize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
            KeyCode::PageDown => *scroll = scroll.saturating_add(page),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
            KeyCode::Char('G') | KeyCode::End => *scroll = usize::MAX,
            KeyCode::Char('[') => return self.step_detail(-1),
            KeyCode::Char(']') => return self.step_detail(1),
            KeyCode::Enter => return self.detail_pr().map_or(Action::None, Action::Open),
            KeyCode::Char('r') if !self.details_loading.contains(&number) => {
                if let Some(i) = self.detail_pr() {
                    self.details_loading.insert(number);
                    return Action::RefreshDetail(i);
                }
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.view = View::List,
            _ => {}
        }
        Action::None
    }

    /// Index into `listing.prs` of the PR shown in the detail view.
    pub fn detail_pr(&self) -> Option<usize> {
        let View::Detail { number, .. } = self.view else { return None };
        self.listing.as_ref()?.prs.iter().position(|p| p.number == number)
    }

    fn preview_selected(&mut self) -> Action {
        let Some(i) = self.selected_pr() else { return Action::None };
        self.show_detail(i)
    }

    /// Previous / next PR in the current (filtered) list order. The list
    /// selection follows, so going back lands on the PR last viewed.
    fn step_detail(&mut self, delta: isize) -> Action {
        self.move_by(delta);
        match self.selected_pr() {
            Some(i) if Some(i) != self.detail_pr() => self.show_detail(i),
            _ => Action::None,
        }
    }

    fn show_detail(&mut self, i: usize) -> Action {
        let Some(number) = self.listing.as_ref().map(|l| l.prs[i].number) else { return Action::None };
        self.view = View::Detail { number, scroll: 0 };
        if self.details.contains_key(&number) || !self.details_loading.insert(number) {
            return Action::None;
        }
        Action::Preview(i)
    }

    /// Store fetched details; reopening this PR's preview reuses them.
    pub fn set_detail(&mut self, number: u64, details: PrDetails) {
        self.details_loading.remove(&number);
        self.details.insert(number, details);
    }

    fn open_selected(&self) -> Action {
        match self.selected_pr() {
            Some(i) => Action::Open(i),
            None => Action::None,
        }
    }

    /// A fresh listing replaces the old one; keep the selection on the same
    /// PR number when it is still listed.
    pub fn set_listing(&mut self, listing: Listing, cached_age: Option<u64>) {
        let keep = self.selected_pr().and_then(|i| self.listing.as_ref().map(|l| l.prs[i].number));
        self.listing = Some(listing);
        self.cached_age = cached_age;
        self.load_error = None;
        self.selected = 0;
        if let (Some(number), Some(l)) = (keep, &self.listing) {
            let order = self.selectable();
            if let Some(pos) = order.iter().position(|&i| l.prs[i].number == number) {
                self.selected = pos;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Ci, Pr};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn pr(number: u64, author: &str, title: &str) -> Pr {
        Pr {
            number,
            title: title.into(),
            author: author.into(),
            head_branch: format!("b{number}"),
            is_fork: false,
            is_draft: false,
            ci: Ci::None,
            review_requested: false,
            url: String::new(),
        }
    }

    fn app() -> App {
        let mut a = App::new();
        a.loading = false;
        a.set_listing(
            Listing {
                viewer: "me".into(),
                prs: vec![pr(1, "x", "alpha"), pr(2, "me", "beta"), pr(3, "y", "gamma")],
            },
            None,
        );
        a
    }

    #[test]
    fn rows_group_with_blank_separators() {
        let a = app();
        assert_eq!(
            a.rows(),
            vec![
                Row::Header(Group::Mine),
                Row::Pr(1),
                Row::Blank,
                Row::Header(Group::Others),
                Row::Pr(0),
                Row::Pr(2)
            ]
        );
        assert_eq!(a.selectable(), vec![1, 0, 2]);
    }

    #[test]
    fn navigation_is_clamped() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('k')), 10);
        assert_eq!(a.selected, 0);
        a.handle_key(key(KeyCode::PageDown), 10);
        assert_eq!(a.selected, 2);
        assert_eq!(a.handle_key(key(KeyCode::Enter), 10), Action::Open(2));
    }

    #[test]
    fn filter_is_live_and_esc_clears_before_closing() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('/')), 10);
        for c in "gam".chars() {
            a.handle_key(key(KeyCode::Char(c)), 10);
        }
        assert_eq!(a.selectable(), vec![2]);
        // j is text while filtering, not movement.
        a.handle_key(key(KeyCode::Char('j')), 10);
        assert_eq!(a.query, "gamj");
        assert!(a.selectable().is_empty());
        assert_eq!(a.handle_key(key(KeyCode::Esc), 10), Action::None);
        assert!(!a.filtering);
        assert!(a.query.is_empty());
        assert_eq!(a.handle_key(key(KeyCode::Esc), 10), Action::Quit);
    }

    #[test]
    fn enter_in_filter_opens_match() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('/')), 10);
        a.handle_key(key(KeyCode::Char('#')), 10);
        a.handle_key(key(KeyCode::Char('3')), 10);
        assert_eq!(a.handle_key(key(KeyCode::Enter), 10), Action::Open(2));
    }

    #[test]
    fn busy_ignores_keys_and_fatal_closes_on_esc() {
        let mut a = app();
        a.busy = Some("opening".into());
        assert_eq!(a.handle_key(key(KeyCode::Esc), 10), Action::None);
        let mut f = App::new();
        f.fatal = Some(ErrRow::new("not a git repository", None));
        assert_eq!(f.handle_key(key(KeyCode::Char('j')), 10), Action::None);
        assert_eq!(f.handle_key(key(KeyCode::Esc), 10), Action::Quit);
    }

    #[test]
    fn refresh_keeps_selection_on_same_pr() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('j')), 10); // → #1
        assert_eq!(a.handle_key(key(KeyCode::Char('r')), 10), Action::Refresh);
        a.set_listing(
            Listing { viewer: "me".into(), prs: vec![pr(9, "z", "new"), pr(1, "x", "alpha")] },
            None,
        );
        assert_eq!(a.selected_pr().map(|i| a.listing.as_ref().unwrap().prs[i].number), Some(1));
    }

    #[test]
    fn scroll_follows_selection_and_shows_group_header() {
        let mut a = app();
        a.selected = 2; // #3, line 5
        a.ensure_visible(3);
        assert_eq!(a.scroll, 3);
        a.selected = 1; // #1, first in "others", header on line 3
        a.ensure_visible(3);
        assert_eq!(a.scroll, 3);
        a.selected = 0; // #2 under "mine"
        a.ensure_visible(3);
        assert_eq!(a.scroll, 0);
    }

    // --- Detail view -------------------------------------------------------

    fn details() -> PrDetails {
        PrDetails::failed(crate::provider::ProviderError::new("boom", None))
    }

    fn number_shown(a: &App) -> Option<u64> {
        if let View::Detail { number, .. } = a.view { Some(number) } else { None }
    }

    #[test]
    fn preview_keys_open_detail_and_fetch_once() {
        for code in [KeyCode::Char('p'), KeyCode::Char('l'), KeyCode::Right] {
            let mut a = app();
            assert_eq!(a.handle_key(key(code), 10), Action::Preview(1), "{code:?}"); // #2, first under "mine"
            assert_eq!(number_shown(&a), Some(2));
            assert!(a.details_loading.contains(&2));
            // Back and in again while in flight: no second fetch.
            a.handle_key(key(KeyCode::Esc), 10);
            assert_eq!(a.handle_key(key(code), 10), Action::None);
            // Once loaded, reopening uses the in-memory result.
            a.set_detail(2, details());
            a.handle_key(key(KeyCode::Esc), 10);
            assert_eq!(a.handle_key(key(code), 10), Action::None);
            assert!(a.details.contains_key(&2) && a.details_loading.is_empty());
        }
    }

    #[test]
    fn detail_scroll_keys() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('p')), 10);
        let scroll = |a: &App| if let View::Detail { scroll, .. } = a.view { scroll } else { panic!() };
        a.handle_key(key(KeyCode::Char('j')), 10);
        a.handle_key(key(KeyCode::Down), 10);
        assert_eq!(scroll(&a), 2);
        a.handle_key(key(KeyCode::Char('k')), 10);
        a.handle_key(key(KeyCode::Up), 10);
        a.handle_key(key(KeyCode::Up), 10);
        assert_eq!(scroll(&a), 0);
        a.handle_key(key(KeyCode::PageDown), 10);
        assert_eq!(scroll(&a), 10);
        a.handle_key(key(KeyCode::PageUp), 10);
        assert_eq!(scroll(&a), 0);
        a.handle_key(key(KeyCode::Char('G')), 10);
        assert_eq!(scroll(&a), usize::MAX); // clamped by the renderer
        a.handle_key(key(KeyCode::Char('g')), 10);
        assert_eq!(scroll(&a), 0);
    }

    #[test]
    fn brackets_walk_the_filtered_list_and_back_keeps_selection() {
        let mut a = app();
        a.query = "m".into(); // #2 (author "me") and #3 ("gamma")
        assert_eq!(a.selectable(), vec![1, 2]);
        a.handle_key(key(KeyCode::Char('p')), 10);
        a.handle_key(key(KeyCode::Char('j')), 10);
        assert_eq!(a.handle_key(key(KeyCode::Char(']')), 10), Action::Preview(2));
        assert_eq!(number_shown(&a), Some(3));
        assert_eq!(a.view, View::Detail { number: 3, scroll: 0 }, "scroll resets");
        assert_eq!(a.handle_key(key(KeyCode::Char(']')), 10), Action::None, "clamped at the end");
        assert_eq!(number_shown(&a), Some(3));
        assert_eq!(a.handle_key(key(KeyCode::Char('[')), 10), Action::None, "#2 already in flight");
        assert_eq!(number_shown(&a), Some(2));
        a.handle_key(key(KeyCode::Char(']')), 10);
        for back in [KeyCode::Esc, KeyCode::Left, KeyCode::Char('h')] {
            a.view = View::Detail { number: 3, scroll: 0 };
            assert_eq!(a.handle_key(key(back), 10), Action::None, "{back:?}");
            assert_eq!(a.view, View::List);
            assert_eq!(a.query, "m");
            assert_eq!(a.selected_pr(), Some(2), "selection stays on the PR last viewed");
        }
    }

    #[test]
    fn enter_opens_and_r_refetches_in_detail() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('p')), 10);
        assert_eq!(a.handle_key(key(KeyCode::Char('r')), 10), Action::None, "already fetching");
        a.set_detail(2, details()); // every section failed
        assert_eq!(a.handle_key(key(KeyCode::Enter), 10), Action::Open(1));
        assert_eq!(a.handle_key(key(KeyCode::Char('r')), 10), Action::RefreshDetail(1));
        assert_eq!(a.handle_key(key(KeyCode::Char('r')), 10), Action::None);
        // `r` in the detail view never refreshes the list.
        assert!(a.details_loading.contains(&2));
    }

    #[test]
    fn preview_keys_are_text_while_filtering() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('/')), 10);
        a.handle_key(key(KeyCode::Char('l')), 10);
        a.handle_key(key(KeyCode::Char('p')), 10);
        assert_eq!(a.query, "lp");
        assert_eq!(a.view, View::List);
    }

    #[test]
    fn detail_survives_listing_refresh() {
        let mut a = app();
        a.handle_key(key(KeyCode::Char('p')), 10); // #2
        a.set_listing(
            Listing {
                viewer: "me".into(),
                prs: vec![pr(9, "z", "new"), pr(3, "y", "gamma"), pr(2, "me", "beta")],
            },
            None,
        );
        assert_eq!(a.detail_pr(), Some(2));
        assert_eq!(a.handle_key(key(KeyCode::Enter), 10), Action::Open(2));
        // The PR was closed upstream: the next key falls back to the list.
        a.set_listing(Listing { viewer: "me".into(), prs: vec![pr(9, "z", "new")] }, None);
        assert_eq!(a.detail_pr(), None);
        a.handle_key(key(KeyCode::Char('j')), 10);
        assert_eq!(a.view, View::List);
    }
}
