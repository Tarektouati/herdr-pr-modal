//! Rendering, modelled on Herdr's keybinds help modal
//! (`client/shell/overlays.rs::render_help_overlay`, v0.9.1): title row with
//! an `esc close` badge, a `/` hint row, grouped two-column body, `▐`
//! scrollbar, footer of key hints. Herdr draws the popup border itself, so
//! the whole frame here is the modal's inner area.

use crate::app::{App, ErrRow, Row};
use crate::model::Ci;
use crate::theme::Palette;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const HINT: &str = " / press / to filter by number, title, branch or author";
pub const FOOTER: &str = " search / · move j/k/↑↓ · open enter · refresh r · close esc";
pub const FOOTER_FILTER: &str = " type to filter · move ↑↓ · open enter · clear esc";
pub const FOOTER_FATAL: &str = " close esc";
const BADGE_WIDTH: u16 = 13;
pub const WORKTREE_MARK: &str = "●";

/// Body viewport height for a frame of `area` (rows 3 .. h-2).
pub fn body_height(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(5))
}

fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

fn put(buf: &mut Buffer, x: u16, y: u16, max: u16, text: &str, style: Style) {
    buf.set_stringn(x, y, text, usize::from(max), style);
}

pub fn draw(frame: &mut Frame, app: &mut App, p: &Palette) {
    let area = frame.area();
    let base = Style::default().fg(p.text).bg(p.panel_bg);
    frame.buffer_mut().set_style(area, base);
    if area.width < 24 || area.height < 6 {
        put(frame.buffer_mut(), area.x, area.y, area.width, "popup too small", base);
        return;
    }
    let (x, w) = (area.x, area.width);

    // Row 0: title · status, badge on the right.
    let title_style = base.add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0).bg(p.panel_bg);
    let mut title = vec![Span::styled("open PRs", title_style)];
    if let Some(repo) = &app.repo_label {
        title.push(Span::styled(format!(" · {repo}"), title_style));
    }
    if app.loading && app.listing.is_some() {
        title.push(Span::styled(" · refreshing…", dim));
    } else if let Some(age) = app.cached_age {
        title.push(Span::styled(format!(" · cached {age}s"), dim));
    }
    let title_w = w.saturating_sub(BADGE_WIDTH + 1);
    frame.buffer_mut().set_line(x, area.y, &Line::from(title), title_w);
    let badge = if app.filtering { " esc back " } else { " esc close " };
    let badge_style = Style::default().fg(p.contrast()).bg(p.accent).add_modifier(Modifier::BOLD);
    let badge_rect = Rect::new(area.right() - BADGE_WIDTH, area.y, BADGE_WIDTH, 1);
    frame.buffer_mut().set_style(badge_rect, badge_style);
    let bw = badge.width() as u16;
    put(frame.buffer_mut(), badge_rect.x + (BADGE_WIDTH - bw) / 2, area.y, bw, badge, badge_style);

    // Row 1: filter hint / live query / progress.
    let hy = area.y + 1;
    if let Some(busy) = &app.busy {
        put(
            frame.buffer_mut(),
            x,
            hy,
            w,
            &format!(" ⟳ {busy}"),
            Style::default().fg(p.accent).bg(p.panel_bg),
        );
    } else if app.filtering {
        let q = format!(" / {}", app.query);
        put(frame.buffer_mut(), x, hy, w, &q, base);
        let cx = (x + q.width() as u16).min(area.right() - 1);
        frame.set_cursor_position(Position::new(cx, hy));
    } else if !app.query.is_empty() {
        put(frame.buffer_mut(), x, hy, w, &format!(" / {}", app.query), dim);
    } else {
        put(frame.buffer_mut(), x, hy, w, HINT, dim);
    }

    // Body.
    let body = Rect::new(x, area.y + 3, w, area.height.saturating_sub(5));
    draw_body(frame.buffer_mut(), body, app, p);

    // Footer.
    let footer = if app.fatal.is_some() {
        FOOTER_FATAL
    } else if app.filtering {
        FOOTER_FILTER
    } else {
        FOOTER
    };
    put(frame.buffer_mut(), x, area.bottom() - 1, w, footer, dim);
}

fn err_line(e: &ErrRow, p: &Palette) -> Line<'static> {
    let bg = p.panel_bg;
    let mut spans = vec![
        Span::styled(" ✗ ", Style::default().fg(p.red).bg(bg).add_modifier(Modifier::BOLD)),
        Span::styled(e.message.clone(), Style::default().fg(p.text).bg(bg)),
    ];
    if let Some(fix) = &e.fix {
        spans.push(Span::styled(" · fix: ", Style::default().fg(p.overlay0).bg(bg)));
        spans.push(Span::styled(
            fix.clone(),
            Style::default().fg(p.accent).bg(bg).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn draw_body(buf: &mut Buffer, body: Rect, app: &mut App, p: &Palette) {
    let bg = p.panel_bg;
    let wrap = |lines: Vec<Line<'static>>, buf: &mut Buffer| {
        Paragraph::new(lines).wrap(Wrap { trim: false }).render(body, buf);
    };
    if let Some(e) = app.fatal.as_ref().or(app.load_error.as_ref().filter(|_| app.listing.is_none())) {
        wrap(vec![err_line(e, p)], buf);
        return;
    }
    if app.listing.is_none() {
        let s = Style::default().fg(p.overlay1).bg(bg);
        wrap(vec![Line::styled(" loading open PRs…", s)], buf);
        return;
    }

    let mut area = body;
    if let Some(banner) = app.banner.clone() {
        let line = err_line(&banner, p);
        let wrapped = line.width().div_ceil(usize::from(body.width.max(1))).max(1);
        let lines = u16::try_from(wrapped).unwrap_or(1).min(body.height.saturating_sub(2).max(1));
        Paragraph::new(line)
            .wrap(Wrap { trim: false })
            .render(Rect::new(body.x, body.y, body.width, lines), buf);
        area = Rect::new(body.x, body.y + lines + 1, body.width, body.height.saturating_sub(lines + 1));
    }
    if area.height == 0 {
        return;
    }

    let rows = app.rows();
    if rows.is_empty() {
        let msg = if app.listing.as_ref().is_some_and(|l| l.prs.is_empty()) {
            " no open PRs".to_string()
        } else {
            format!(" no PRs match '{}'", app.query)
        };
        put(buf, area.x, area.y, area.width, &msg, Style::default().fg(p.overlay1).bg(bg));
        return;
    }

    let viewport = usize::from(area.height);
    let needs_bar = rows.len() > viewport;
    let text_w = if needs_bar { area.width - 1 } else { area.width };
    app.ensure_visible(viewport);
    let selected = app.selected_pr();
    let app = &*app;
    let Some(listing) = app.listing.as_ref() else { return };
    let num_w = rows
        .iter()
        .filter_map(|r| {
            if let Row::Pr(i) = r { Some(listing.prs[*i].number.to_string().len() + 1) } else { None }
        })
        .max()
        .unwrap_or(2);

    for (line_no, row) in rows.iter().enumerate().skip(app.scroll).take(viewport) {
        let y = area.y + (line_no - app.scroll) as u16;
        match row {
            Row::Blank => {}
            Row::Header(g) => {
                let s = Style::default().fg(p.accent).bg(bg).add_modifier(Modifier::BOLD);
                put(buf, area.x, y, text_w, &format!(" {}", g.label()), s);
            }
            Row::Pr(i) => {
                let pr = &listing.prs[*i];
                let is_sel = selected == Some(*i);
                let line =
                    pr_line(pr, num_w, app.worktree_prs.contains(&pr.number), usize::from(text_w), p, is_sel);
                if is_sel {
                    buf.set_style(Rect::new(area.x, y, text_w, 1), sel_style(p));
                }
                buf.set_line(area.x, y, &line, text_w);
            }
        }
    }

    if needs_bar {
        let track = Rect::new(area.right() - 1, area.y, 1, area.height);
        let (top, len) = thumb(rows.len(), viewport, app.scroll);
        for yy in 0..track.height {
            let in_thumb = (usize::from(yy)) >= top && (usize::from(yy)) < top + len;
            let fg = if in_thumb { p.overlay1 } else { p.overlay0 };
            buf[(track.x, track.y + yy)].set_symbol("▐").set_style(Style::default().fg(fg).bg(bg));
        }
    }
}

fn sel_style(p: &Palette) -> Style {
    Style::default().fg(p.contrast()).bg(p.accent).add_modifier(Modifier::BOLD)
}

/// Scrollbar thumb `(top, len)` in track cells.
pub fn thumb(total: usize, viewport: usize, scroll: usize) -> (usize, usize) {
    if total <= viewport || viewport == 0 {
        return (0, viewport);
    }
    let len = (viewport * viewport / total).max(1);
    let max_scroll = total - viewport;
    let top = (scroll.min(max_scroll) * (viewport - len) + max_scroll / 2) / max_scroll;
    (top, len)
}

pub fn ci_marker(ci: Ci) -> Option<&'static str> {
    match ci {
        Ci::Pass => Some("[CI ✓]"),
        Ci::Fail => Some("[CI ✗]"),
        Ci::Pending => Some("[CI …]"),
        Ci::None => None,
    }
}

/// ` #123 ● title  author · branch [draft] [CI ✓]`, fitted to `width`.
/// Markers that do not fit are dropped; author/branch are cut first.
pub fn pr_line(
    pr: &crate::model::Pr,
    num_w: usize,
    has_worktree: bool,
    width: usize,
    p: &Palette,
    selected: bool,
) -> Line<'static> {
    let bg = if selected { p.accent } else { p.panel_bg };
    let st = |fg| {
        if selected { sel_style(p) } else { Style::default().fg(fg).bg(bg) }
    };
    let num = format!(" {:<num_w$} ", format!("#{}", pr.number));
    let mark = if has_worktree { format!("{WORKTREE_MARK} ") } else { "  ".into() };
    let mut markers: Vec<(String, Style)> = Vec::new();
    if pr.is_draft {
        markers.push((" [draft]".into(), st(p.overlay0)));
    }
    if let Some(m) = ci_marker(pr.ci) {
        let fg = match pr.ci {
            Ci::Pass => p.green,
            Ci::Fail => p.red,
            _ => p.yellow,
        };
        markers.push((format!(" {m}"), st(fg)));
    }
    let meta = format!("  {} · {}", pr.author, pr.head_branch);
    let markers_w: usize = markers.iter().map(|(s, _)| s.width()).sum();
    let avail = width.saturating_sub(num.width() + mark.width());
    let title_w = pr.title.width();
    let tail_w = meta.width() + markers_w;
    // The title keeps up to 60% of the row before author/branch are cut.
    let title_room = title_w.min(avail.saturating_sub(tail_w).max(avail * 3 / 5));
    let title = truncate(&pr.title, title_room.max(1));
    let mut rest = avail.saturating_sub(title.width());

    let mut spans = vec![
        Span::styled(num, if selected { sel_style(p) } else { st(p.accent).add_modifier(Modifier::BOLD) }),
        Span::styled(mark, st(p.green)),
        Span::styled(title, if selected { sel_style(p) } else { st(p.text) }),
    ];
    let meta_fit = truncate(&meta, rest.saturating_sub(markers_w));
    rest = rest.saturating_sub(meta_fit.width());
    spans.push(Span::styled(meta_fit, st(p.overlay0)));
    for (text, style) in markers {
        if text.width() <= rest {
            rest -= text.width();
            spans.push(Span::styled(text, style));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_is_width_aware() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("日本語テキスト", 5), "日本…");
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn short_title_survives_long_branch() {
        let pr = crate::model::Pr {
            number: 38,
            title: "chore(main): release 1.0.2".into(),
            author: "app/github-actions".into(),
            head_branch: "release-please--branches--main--components--herdr-nvim".into(),
            is_fork: false,
            is_draft: true,
            ci: Ci::Pass,
            review_requested: false,
            url: String::new(),
        };
        let p = Palette::catppuccin();
        let text = line_text(&pr_line(&pr, 3, true, 80, &p, false));
        assert!(
            text.starts_with(" #38 ● chore(main): release 1.0.2  app/github-actions · release-"),
            "{text}"
        );
        assert!(text.ends_with(" [draft] [CI ✓]"), "{text}");
        assert!(text.width() <= 80, "{}", text.width());
        let narrow = line_text(&pr_line(&pr, 3, false, 30, &p, false));
        assert!(narrow.width() <= 30, "{narrow}");
        assert!(narrow.contains("chore"), "{narrow}");
    }

    #[test]
    fn thumb_spans_track() {
        assert_eq!(thumb(10, 10, 0), (0, 10));
        assert_eq!(thumb(100, 10, 0), (0, 1));
        assert_eq!(thumb(100, 10, 90), (9, 1));
        let (top, len) = thumb(20, 10, 5);
        assert_eq!(len, 5);
        assert!(top > 0 && top + len <= 10);
    }
}
