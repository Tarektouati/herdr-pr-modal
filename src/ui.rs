//! Rendering, modelled on Herdr's keybinds help modal
//! (`client/shell/overlays.rs::render_help_overlay`, v0.9.1): title row with
//! an `esc close` badge, a `/` hint row, grouped two-column body, `▐`
//! scrollbar, footer of key hints. Herdr draws the popup border itself, so
//! the whole frame here is the modal's inner area.

use crate::app::{App, ErrRow, Row, View};
use crate::model::{Ci, Comments, PrDetails, PrMeta};
use crate::provider::ProviderError;
use crate::theme::Palette;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const HINT: &str = " / press / to filter by number, title, branch or author";
pub const FOOTER: &str = " search / · move j/k/↑↓ · preview p/→ · open enter · refresh r · close esc";
pub const FOOTER_DETAIL: &str = " scroll j/k/↑↓ · prev/next [ ] · open enter · refresh r · back esc";
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
    if matches!(app.tab().view, View::Detail { .. }) && app.fatal.is_none() {
        if app.tab().detail_pr().is_some() {
            draw_detail(frame, app, p);
            return;
        }
        app.tab_mut().view = View::List;
    }
    let (x, w) = (area.x, area.width);

    // Row 0: title · status, badge on the right.
    let title_style = base.add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0).bg(p.panel_bg);
    let mut title = vec![Span::styled("open PRs", title_style)];
    if let Some(repo) = &app.tab().repo_label {
        title.push(Span::styled(format!(" · {repo}"), title_style));
    }
    if app.tab().loading && app.tab().listing.is_some() {
        title.push(Span::styled(" · refreshing…", dim));
    } else if let Some(age) = app.tab().cached_age {
        title.push(Span::styled(format!(" · cached {age}s"), dim));
    }
    let title_w = w.saturating_sub(BADGE_WIDTH + 1);
    frame.buffer_mut().set_line(x, area.y, &Line::from(title), title_w);
    draw_badge(frame.buffer_mut(), area, if app.tab().filtering { " esc back " } else { " esc close " }, p);

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
    } else if app.tab().filtering {
        let q = format!(" / {}", app.tab().query);
        put(frame.buffer_mut(), x, hy, w, &q, base);
        let cx = (x + q.width() as u16).min(area.right() - 1);
        frame.set_cursor_position(Position::new(cx, hy));
    } else if !app.tab().query.is_empty() {
        put(frame.buffer_mut(), x, hy, w, &format!(" / {}", app.tab().query), dim);
    } else {
        put(frame.buffer_mut(), x, hy, w, HINT, dim);
    }

    // Body.
    let body = Rect::new(x, area.y + 3, w, area.height.saturating_sub(5));
    draw_body(frame.buffer_mut(), body, app, p);

    // Footer.
    let footer = if app.fatal.is_some() {
        FOOTER_FATAL
    } else if app.tab().filtering {
        FOOTER_FILTER
    } else {
        FOOTER
    };
    put(frame.buffer_mut(), x, area.bottom() - 1, w, footer, dim);
}

fn draw_badge(buf: &mut Buffer, area: Rect, badge: &str, p: &Palette) {
    let badge_style = Style::default().fg(p.contrast()).bg(p.accent).add_modifier(Modifier::BOLD);
    let badge_rect = Rect::new(area.right() - BADGE_WIDTH, area.y, BADGE_WIDTH, 1);
    buf.set_style(badge_rect, badge_style);
    let bw = badge.width() as u16;
    put(buf, badge_rect.x + (BADGE_WIDTH - bw) / 2, area.y, bw, badge, badge_style);
}

/// Error banner (failed refresh or open) at the top of `body`; returns the
/// area left below it.
fn draw_banner(buf: &mut Buffer, body: Rect, banner: Option<&ErrRow>, p: &Palette) -> Rect {
    let Some(banner) = banner else { return body };
    let line = err_line(banner, p);
    let wrapped = line.width().div_ceil(usize::from(body.width.max(1))).max(1);
    let lines = u16::try_from(wrapped).unwrap_or(1).min(body.height.saturating_sub(2).max(1));
    Paragraph::new(line).wrap(Wrap { trim: false }).render(Rect::new(body.x, body.y, body.width, lines), buf);
    Rect::new(body.x, body.y + lines + 1, body.width, body.height.saturating_sub(lines + 1))
}

fn draw_scrollbar(buf: &mut Buffer, area: Rect, total: usize, viewport: usize, scroll: usize, p: &Palette) {
    let track = Rect::new(area.right() - 1, area.y, 1, area.height);
    let (top, len) = thumb(total, viewport, scroll);
    for yy in 0..track.height {
        let in_thumb = (usize::from(yy)) >= top && (usize::from(yy)) < top + len;
        let fg = if in_thumb { p.overlay1 } else { p.overlay0 };
        buf[(track.x, track.y + yy)].set_symbol("▐").set_style(Style::default().fg(fg).bg(p.panel_bg));
    }
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
    if let Some(e) =
        app.fatal.as_ref().or(app.tab().load_error.as_ref().filter(|_| app.tab().listing.is_none()))
    {
        wrap(vec![err_line(e, p)], buf);
        return;
    }
    if app.tab().listing.is_none() {
        let s = Style::default().fg(p.overlay1).bg(bg);
        wrap(vec![Line::styled(" loading open PRs…", s)], buf);
        return;
    }

    let area = draw_banner(buf, body, app.tab().banner.as_ref(), p);
    if area.height == 0 {
        return;
    }

    let rows = app.tab().rows();
    if rows.is_empty() {
        let msg = if app.tab().listing.as_ref().is_some_and(|l| l.prs.is_empty()) {
            " no open PRs".to_string()
        } else {
            format!(" no PRs match '{}'", app.tab().query)
        };
        put(buf, area.x, area.y, area.width, &msg, Style::default().fg(p.overlay1).bg(bg));
        return;
    }

    let viewport = usize::from(area.height);
    let needs_bar = rows.len() > viewport;
    let text_w = if needs_bar { area.width - 1 } else { area.width };
    app.tab_mut().ensure_visible(viewport);
    let selected = app.tab().selected_pr();
    let app = &*app;
    let Some(listing) = app.tab().listing.as_ref() else { return };
    let num_w = rows
        .iter()
        .filter_map(|r| {
            if let Row::Pr(i) = r { Some(listing.prs[*i].number.to_string().len() + 1) } else { None }
        })
        .max()
        .unwrap_or(2);

    for (line_no, row) in rows.iter().enumerate().skip(app.tab().scroll).take(viewport) {
        let y = area.y + (line_no - app.tab().scroll) as u16;
        match row {
            Row::Blank => {}
            Row::Header(g) => {
                let s = Style::default().fg(p.accent).bg(bg).add_modifier(Modifier::BOLD);
                put(buf, area.x, y, text_w, &format!(" {}", g.label()), s);
            }
            Row::Pr(i) => {
                let pr = &listing.prs[*i];
                let is_sel = selected == Some(*i);
                let line = pr_line(
                    pr,
                    num_w,
                    app.tab().worktree_prs.contains(&pr.number),
                    usize::from(text_w),
                    p,
                    is_sel,
                );
                if is_sel {
                    buf.set_style(Rect::new(area.x, y, text_w, 1), sel_style(p));
                }
                buf.set_line(area.x, y, &line, text_w);
            }
        }
    }

    if needs_bar {
        draw_scrollbar(buf, area, rows.len(), viewport, app.tab().scroll, p);
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

// ---------------------------------------------------------------------------
// Detail (preview) view

fn draw_detail(frame: &mut Frame, app: &mut App, p: &Palette) {
    let area = frame.area();
    let (x, w) = (area.x, area.width);
    let (Some(idx), View::Detail { number, scroll }) = (app.tab().detail_pr(), app.tab().view) else {
        return;
    };
    let Some(pr) = app.tab().listing.as_ref().map(|l| l.prs[idx].clone()) else { return };
    let base = Style::default().fg(p.text).bg(p.panel_bg);
    let dim = Style::default().fg(p.overlay0).bg(p.panel_bg);
    let loading = app.tab().details_loading.contains(&number);
    let details = app.tab().details.get(&number);

    // Row 0: `#123 title`, fetch status, badge.
    let title_style = base.add_modifier(Modifier::BOLD);
    let mut title = vec![Span::styled(format!("#{} {}", pr.number, pr.title), title_style)];
    if loading {
        title.push(Span::styled(if details.is_some() { " · refreshing…" } else { " · loading…" }, dim));
    }
    frame.buffer_mut().set_line(x, area.y, &Line::from(title), w.saturating_sub(BADGE_WIDTH + 1));
    draw_badge(frame.buffer_mut(), area, " esc back ", p);

    // Row 1: progress of an Enter, otherwise the PR's URL (author and
    // branches are the first body row).
    let hy = area.y + 1;
    if let Some(busy) = &app.busy {
        let s = Style::default().fg(p.accent).bg(p.panel_bg);
        put(frame.buffer_mut(), x, hy, w, &format!(" ⟳ {busy}"), s);
    } else if !pr.url.is_empty() {
        put(frame.buffer_mut(), x, hy, w, &format!(" {}", pr.url), dim);
    } else {
        put(frame.buffer_mut(), x, hy, w, &format!(" {} · {}", pr.author, pr.head_branch), dim);
    }

    // Body, pre-wrapped so scrolling counts real rows.
    let body = Rect::new(x, area.y + 3, w, area.height.saturating_sub(5));
    let banner = app.tab().banner.clone();
    let body = draw_banner(frame.buffer_mut(), body, banner.as_ref(), p);
    if body.height > 0 {
        let viewport = usize::from(body.height);
        let mut lines = detail_lines(pr.number, details, loading, usize::from(body.width), p);
        let needs_bar = lines.len() > viewport;
        if needs_bar {
            lines = detail_lines(pr.number, details, loading, usize::from(body.width - 1), p);
        }
        let text_w = if needs_bar { body.width - 1 } else { body.width };
        let scroll = scroll.min(lines.len().saturating_sub(viewport));
        app.tab_mut().view = View::Detail { number, scroll };
        let buf = frame.buffer_mut();
        for (row, line) in lines.iter().skip(scroll).take(viewport).enumerate() {
            buf.set_line(body.x, body.y + row as u16, line, text_w);
        }
        if needs_bar {
            draw_scrollbar(buf, body, lines.len(), viewport, scroll, p);
        }
    }

    put(frame.buffer_mut(), x, area.bottom() - 1, w, FOOTER_DETAIL, dim);
}

/// `2026-03-24T04:06:10Z` → `2026-03-24`.
fn date(ts: &str) -> &str {
    ts.get(..10).unwrap_or(ts)
}

fn check_symbol(ci: Ci, p: &Palette) -> (&'static str, ratatui::style::Color) {
    match ci {
        Ci::Pass => ("✓", p.green),
        Ci::Fail => ("✗", p.red),
        Ci::Pending => ("…", p.yellow),
        Ci::None => ("–", p.overlay0),
    }
}

/// Every row of the preview body, wrapped to `width`: meta, description,
/// checks and comments. A section that failed shows its error; an error
/// equal to the meta one (a single failed call) is shown once.
pub fn detail_lines(
    number: u64,
    details: Option<&PrDetails>,
    loading: bool,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    let bg = p.panel_bg;
    let st = |fg| Style::default().fg(fg).bg(bg);
    let Some(d) = details else {
        let msg = if loading { format!(" loading #{number}…") } else { " no details".into() };
        return vec![Line::styled(msg, st(p.overlay1))];
    };
    let mut out: Vec<Line<'static>> = Vec::new();
    let push = |line: Line<'static>, indent: usize, out: &mut Vec<Line<'static>>| {
        out.extend(wrap_line(line, width, indent));
    };
    let push_raw = |line: Line<'static>, out: &mut Vec<Line<'static>>| out.extend(wrap_line(line, width, 1));
    let header = |text: String| {
        Line::from(Span::styled(format!(" {text}"), st(p.accent).add_modifier(Modifier::BOLD)))
    };
    let err = |e: &ProviderError| err_line(&ErrRow::new(e.message.clone(), e.fix.clone()), p);
    let meta_err = d.meta.as_ref().err();
    let dup = |e: &ProviderError| meta_err == Some(e);

    // Meta.
    match &d.meta {
        Ok(m) => {
            for line in meta_lines(m, p) {
                push(line, META_KEY_W, &mut out);
            }
        }
        Err(e) => push(err(e), 3, &mut out),
    }

    // Description.
    if let Ok(m) = &d.meta {
        out.push(Line::default());
        push_raw(header("description".into()), &mut out);
        let md = md_lite(&m.body);
        if md.is_empty() {
            push(Line::styled(" no description", st(p.overlay0)), 1, &mut out);
        }
        for (kind, text) in md {
            let (line, indent) = match kind {
                MdKind::Text => (Line::from(Span::styled(format!(" {text}"), st(p.text))), 1),
                MdKind::Heading => {
                    (Line::from(Span::styled(format!(" {text}"), st(p.text).add_modifier(Modifier::BOLD))), 1)
                }
                MdKind::Code => (Line::from(Span::styled(format!("   {text}"), st(p.overlay1))), 3),
            };
            push(line, indent, &mut out);
        }
    }

    // Checks.
    match &d.checks {
        Err(e) if dup(e) => {}
        Err(e) => {
            out.push(Line::default());
            push_raw(header("checks".into()), &mut out);
            push(err(e), 3, &mut out);
        }
        Ok(checks) => {
            out.push(Line::default());
            let plus = if checks.more_unknown { "+" } else { "" };
            push_raw(header(format!("checks ({}{plus})", checks.items.len())), &mut out);
            if checks.items.is_empty() {
                push(Line::styled(" no checks reported", st(p.overlay0)), 1, &mut out);
            }
            for c in &checks.items {
                let (sym, color) = check_symbol(c.ci, p);
                let mut spans =
                    vec![Span::styled(format!(" {sym} "), st(color).add_modifier(Modifier::BOLD))];
                if let Some(g) = &c.group {
                    spans.push(Span::styled(format!("{g} / "), st(p.overlay0)));
                }
                spans.push(Span::styled(c.name.clone(), st(p.text)));
                spans.push(Span::styled(format!("  {}", c.label), st(p.overlay0)));
                push(Line::from(spans), 3, &mut out);
            }
        }
    }

    // Comments.
    match &d.comments {
        Err(e) if dup(e) => {}
        Err(e) => {
            out.push(Line::default());
            push_raw(header("comments".into()), &mut out);
            push(err(e), 3, &mut out);
        }
        Ok(c) => {
            out.push(Line::default());
            comment_lines(c, p, &mut |line, indent| push(line, indent, &mut out), &header);
        }
    }
    out
}

const META_KEY_W: usize = 10;

fn meta_lines(m: &PrMeta, p: &Palette) -> Vec<Line<'static>> {
    let bg = p.panel_bg;
    let st = |fg| Style::default().fg(fg).bg(bg);
    let key = |k: &str| Span::styled(format!(" {k:<w$}", w = META_KEY_W - 1), st(p.overlay0));
    let sep = || Span::styled(" · ", st(p.overlay0));
    let mut lines = Vec::new();

    let mut row = vec![
        key("author"),
        Span::styled(m.author.clone(), st(p.text).add_modifier(Modifier::BOLD)),
        sep(),
        Span::styled(format!("{} → {}", m.head_branch, m.base_branch), st(p.text)),
    ];
    if m.is_draft {
        row.push(sep());
        row.push(Span::styled("draft", st(p.yellow)));
    }
    lines.push(Line::from(row));

    if m.review_decision.is_some() || !m.reviewers.is_empty() {
        let mut row = vec![key("review")];
        if let Some(dec) = &m.review_decision {
            let color = match dec.as_str() {
                "approved" => p.green,
                "changes requested" => p.red,
                _ => p.yellow,
            };
            row.push(Span::styled(dec.clone(), st(color).add_modifier(Modifier::BOLD)));
            if !m.reviewers.is_empty() {
                row.push(sep());
            }
        }
        for (i, r) in m.reviewers.iter().enumerate() {
            if i > 0 {
                row.push(Span::styled(", ", st(p.overlay0)));
            }
            row.push(Span::styled(r.login.clone(), st(p.text)));
            if let Some(state) = &r.state {
                let color = match state.as_str() {
                    "approved" => p.green,
                    "changes requested" => p.red,
                    _ => p.overlay0,
                };
                row.push(Span::styled(format!(" ({state})"), st(color)));
            }
        }
        lines.push(Line::from(row));
    }

    if !m.labels.is_empty() {
        let mut row = vec![key("labels")];
        for (i, l) in m.labels.iter().enumerate() {
            if i > 0 {
                row.push(Span::styled(" ", st(p.text)));
            }
            row.push(Span::styled(format!("[{l}]"), st(p.teal)));
        }
        lines.push(Line::from(row));
    }

    let mut size = Vec::new();
    if let Some(a) = m.additions {
        size.push(Span::styled(format!("+{a}"), st(p.green)));
    }
    if let Some(d) = m.deletions {
        if !size.is_empty() {
            size.push(Span::styled(" ", st(p.text)));
        }
        size.push(Span::styled(format!("−{d}"), st(p.red)));
    }
    if let Some(f) = &m.changed_files {
        if !size.is_empty() {
            size.push(sep());
        }
        let noun = if f == "1" { "file" } else { "files" };
        size.push(Span::styled(format!("{f} {noun}"), st(p.text)));
    }
    if !size.is_empty() {
        let mut row = vec![key("size")];
        row.extend(size);
        lines.push(Line::from(row));
    }

    let mut dates = Vec::new();
    if !m.created_at.is_empty() {
        dates.push(format!("created {}", date(&m.created_at)));
    }
    if !m.updated_at.is_empty() {
        dates.push(format!("updated {}", date(&m.updated_at)));
    }
    if !dates.is_empty() {
        lines.push(Line::from(vec![key("dates"), Span::styled(dates.join(" · "), st(p.text))]));
    }
    lines
}

fn comment_lines(
    c: &Comments,
    p: &Palette,
    push: &mut dyn FnMut(Line<'static>, usize),
    header: &dyn Fn(String) -> Line<'static>,
) {
    let st = |fg| Style::default().fg(fg).bg(p.panel_bg);
    let plus = if c.more_unknown { "+" } else { "" };
    push(header(format!("comments ({}{plus})", c.earlier + c.items.len())), 1);
    if c.earlier > 0 {
        let noun = if c.earlier == 1 && !c.more_unknown { "comment" } else { "comments" };
        push(Line::styled(format!(" … {}{plus} earlier {noun}", c.earlier), st(p.overlay0)), 3);
    }
    if c.items.is_empty() {
        push(Line::styled(" no comments", st(p.overlay0)), 1);
    }
    for (i, comment) in c.items.iter().enumerate() {
        if i > 0 {
            push(Line::default(), 0);
        }
        push(
            Line::from(vec![
                Span::styled(format!(" {}", comment.author), st(p.text).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" · {}", date(&comment.created_at)), st(p.overlay0)),
            ]),
            3,
        );
        for (kind, text) in md_lite(&comment.body) {
            let style = match kind {
                MdKind::Text => st(p.text),
                MdKind::Heading => st(p.text).add_modifier(Modifier::BOLD),
                MdKind::Code => st(p.overlay1),
            };
            let pad = if kind == MdKind::Code { "     " } else { "   " };
            push(Line::from(Span::styled(format!("{pad}{text}"), style)), pad.len());
        }
    }
}

// ---------------------------------------------------------------------------
// Wrapping

/// Word-wrap a styled line to `width` columns, keeping span styles.
/// Continuation rows start with `indent` spaces; words wider than a row are
/// broken by character. Width-aware (CJK counts 2).
pub fn wrap_line(line: Line<'static>, width: usize, indent: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let indent = if indent * 2 >= width { 0 } else { indent };
    if line.width() <= width {
        return vec![line];
    }
    // Split every span into whitespace / non-whitespace chunks.
    let mut chunks: Vec<(String, Style, bool)> = Vec::new();
    for span in &line.spans {
        let mut cur = String::new();
        let mut cur_ws = None;
        for ch in span.content.chars() {
            let ws = ch.is_whitespace();
            if cur_ws.is_some_and(|w| w != ws) {
                chunks.push((std::mem::take(&mut cur), span.style, cur_ws.unwrap()));
            }
            cur.push(ch);
            cur_ws = Some(ws);
        }
        if let Some(ws) = cur_ws {
            chunks.push((cur, span.style, ws));
        }
    }
    let pad_style = line.spans.first().map(|s| s.style).unwrap_or_default();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    let new_row = |row: &mut Vec<Span<'static>>, used: &mut usize, out: &mut Vec<Line<'static>>| {
        while row.last().is_some_and(|s| s.content.trim().is_empty()) && row.len() > 1 {
            row.pop();
        }
        out.push(Line::from(std::mem::take(row)));
        if indent > 0 {
            row.push(Span::styled(" ".repeat(indent), pad_style));
        }
        *used = indent;
    };
    for (text, style, ws) in chunks {
        let cw = text.width();
        if ws && used == indent && !out.is_empty() {
            continue; // no leading blanks on a wrapped row
        }
        if used + cw <= width {
            row.push(Span::styled(text, style));
            used += cw;
            continue;
        }
        if ws {
            new_row(&mut row, &mut used, &mut out);
            continue;
        }
        if cw <= width - indent && used > indent {
            new_row(&mut row, &mut used, &mut out);
            row.push(Span::styled(text, style));
            used += cw;
            continue;
        }
        // Hard-break a word longer than a row.
        let mut piece = String::new();
        for ch in text.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > width && (used > indent || !piece.is_empty()) {
                row.push(Span::styled(std::mem::take(&mut piece), style));
                new_row(&mut row, &mut used, &mut out);
            }
            piece.push(ch);
            used += w;
        }
        if !piece.is_empty() {
            row.push(Span::styled(piece, style));
        }
    }
    if row.iter().any(|s| !s.content.trim().is_empty()) || out.is_empty() {
        out.push(Line::from(row));
    }
    out
}

/// Plain-text wrap, for tests and callers without styles.
pub fn wrap_width(s: &str, width: usize) -> Vec<String> {
    wrap_line(Line::raw(s.to_string()), width, 0)
        .into_iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Markdown-lite

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdKind {
    Text,
    Heading,
    Code,
}

/// Light markdown cleanup for terminal display, no markdown crate:
/// HTML comments (PR templates) stripped outside code, `<details>` /
/// `<summary>` tags dropped, headings marked, code fences removed with their
/// contents marked verbatim, task checkboxes as ☐ / ☑, runs of blank lines
/// collapsed.
pub fn md_lite(body: &str) -> Vec<(MdKind, String)> {
    let text = body.replace("\r\n", "\n").replace('\r', "\n");
    let is_fence = |s: &str| {
        let t = s.trim();
        t.starts_with("```") || t.starts_with("~~~")
    };
    let mut out: Vec<(MdKind, String)> = Vec::new();
    let mut in_code = false;
    let mut in_comment = false;
    for raw in text.lines() {
        let mut rest = raw;
        if in_comment {
            // A comment hides everything up to `-->`, fences included.
            // `in_comment` is re-set from the rest of the line below.
            let Some(end) = raw.find("-->") else { continue };
            rest = &raw[end + 3..];
        } else if in_code {
            if is_fence(raw) {
                in_code = false;
            } else {
                out.push((MdKind::Code, raw.trim_end().to_string()));
            }
            continue;
        }
        let (stripped, open) = strip_html_comments(rest);
        in_comment = open;
        if is_fence(&stripped) {
            in_code = true;
            continue;
        }
        let (line, had_tag) = drop_details_tags(&stripped);
        if had_tag && line.trim().is_empty() {
            continue;
        }
        let line = line.trim_end();
        if let Some(h) = heading(line) {
            out.push((MdKind::Heading, h.to_string()));
        } else {
            out.push((MdKind::Text, checkbox(line)));
        }
    }
    // Collapse blank runs and trim the ends.
    let blank = |(k, t): &(MdKind, String)| *k != MdKind::Code && t.trim().is_empty();
    let mut collapsed: Vec<(MdKind, String)> = Vec::new();
    for item in out {
        if blank(&item) && collapsed.last().is_none_or(blank) {
            continue;
        }
        collapsed.push(item);
    }
    while collapsed.last().is_some_and(blank) {
        collapsed.pop();
    }
    collapsed
}

/// Remove `<!-- … -->` from one line. Reports whether a comment is still
/// open at the end of the line (it continues on the next ones).
fn strip_html_comments(line: &str) -> (String, bool) {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => return (out, true),
        }
    }
    out.push_str(rest);
    (out, false)
}

/// Remove `<details …>`, `</details>`, `<summary>` and `</summary>` tags,
/// keeping the text between them. Reports whether a tag was removed.
fn drop_details_tags(line: &str) -> (String, bool) {
    const TAGS: [&str; 4] = ["<details", "</details", "<summary", "</summary"];
    let mut out = String::with_capacity(line.len());
    let mut had = false;
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        let lower = rest.get(..10.min(rest.len())).unwrap_or(rest).to_ascii_lowercase();
        let tag = TAGS.iter().find(|t| {
            lower.starts_with(**t)
                && matches!(rest.as_bytes().get(t.len()), Some(b'>') | Some(b' ') | Some(b'\t'))
        });
        if let (Some(_), Some(close)) = (tag, rest.find('>')) {
            had = true;
            i += close + 1;
            continue;
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, had)
}

/// `## Title` → `Title`. `#12` (an issue ref) is not a heading.
fn heading(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let hashes = t.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &t[hashes..];
    if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
        Some(rest.trim().trim_end_matches('#').trim_end())
    } else {
        None
    }
}

/// `- [ ] x` → `☐ x`, `* [x] y` → `☑ y`, keeping the indent.
fn checkbox(line: &str) -> String {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = body.strip_prefix(bullet) {
            let (mark, tail) = match rest.get(..3) {
                Some("[ ]") => ("☐", &rest[3..]),
                Some("[x]") | Some("[X]") => ("☑", &rest[3..]),
                _ => continue,
            };
            return format!("{indent}{mark}{tail}");
        }
    }
    line.to_string()
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

    // --- Detail view -------------------------------------------------------

    use crate::model::{Check, Checks, Comment, Comments, PrDetails, PrMeta, Reviewer};
    use crate::provider::ProviderError;

    fn md(body: &str) -> Vec<(MdKind, &'static str)> {
        md_lite(body).into_iter().map(|(k, t)| (k, &*Box::leak(t.into_boxed_str()))).collect()
    }

    #[test]
    fn md_lite_strips_templates_and_marks_structure() {
        use MdKind::*;
        let body = "<!--\r\n  Thank you for contributing!\r\n-->\r\n\r\nFixes #13012.\r\n\r\n\r\n\
            ## Summary ##\n<details><summary>Logs</summary>\n\n```sh\n$ gh run\n\n  ok\n```\n</details>\n\
            - [ ] docs\n  * [x] tests\n- plain [ ] item\n#12 is a ref <!-- inline --> here\n\n\n";
        assert_eq!(
            md(body),
            vec![
                (Text, "Fixes #13012."),
                (Text, ""),
                (Heading, "Summary"),
                (Text, "Logs"),
                (Text, ""),
                (Code, "$ gh run"),
                (Code, ""),
                (Code, "  ok"),
                (Text, "☐ docs"),
                (Text, "  ☑ tests"),
                (Text, "- plain [ ] item"),
                (Text, "#12 is a ref  here"),
            ]
        );
        assert!(md_lite("<!-- only a template -->\n\n").is_empty());
        assert!(md_lite("<!-- unterminated\nstill hidden").is_empty());
        // A bare `#` is an empty heading, dropped like a blank line.
        assert_eq!(md("#\n###### six\n####### seven"), vec![(Heading, "six"), (Text, "####### seven")]);
    }

    #[test]
    fn md_lite_keeps_comments_inside_code_and_ignores_fences_inside_comments() {
        use MdKind::*;
        // `<!--` in a fenced block is code, closed or not.
        let body = "Example:\n```html\n<!-- x -->\n<!-- open\n<p>hi</p>\n```\nafter";
        assert_eq!(
            md(body),
            vec![
                (Text, "Example:"),
                (Code, "<!-- x -->"),
                (Code, "<!-- open"),
                (Code, "<p>hi</p>"),
                (Text, "after"),
            ]
        );
        // A multi-line comment hides a fence inside it; text after `-->` stays.
        let body = "a\n<!--\n```\nhidden\n--> b\nc";
        assert_eq!(md(body), vec![(Text, "a"), (Text, ""), (Text, " b"), (Text, "c")]);
    }

    #[test]
    fn wrapping_is_word_and_width_aware() {
        assert_eq!(wrap_width("hello world foo", 11), ["hello world", "foo"]);
        assert_eq!(wrap_width("short", 20), ["short"]);
        assert_eq!(wrap_width("日本語テキスト", 6), ["日本語", "テキス", "ト"]);
        assert_eq!(wrap_width("aaaaaaaaaa b", 4), ["aaaa", "aaaa", "aa b"]);
        // Continuation rows carry the indent and keep span styles.
        let p = Palette::catppuccin();
        let red = Style::default().fg(p.red);
        let line = Line::from(vec![Span::raw(" ✗ "), Span::styled("one two three four", red)]);
        let rows = wrap_line(line, 12, 3);
        let text: Vec<String> = rows.iter().map(line_text).collect();
        assert_eq!(text, [" ✗ one two", "   three", "   four"]);
        assert!(rows[1].spans.iter().any(|s| s.content == "three" && s.style == red));
    }

    fn meta() -> PrMeta {
        PrMeta {
            author: "alice".into(),
            head_branch: "feat/x".into(),
            base_branch: "main".into(),
            is_draft: true,
            review_decision: Some("changes requested".into()),
            reviewers: vec![Reviewer { login: "bob".into(), state: Some("changes requested".into()) }],
            labels: vec!["bug".into(), "ui".into()],
            additions: Some(12),
            deletions: Some(3),
            changed_files: Some("2".into()),
            created_at: "2026-09-01T10:00:00Z".into(),
            updated_at: "2026-09-02T10:00:00Z".into(),
            body: "<!-- template -->\n- [x] done".into(),
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(line_text).collect()
    }

    #[test]
    fn detail_lines_render_every_section() {
        let p = Palette::catppuccin();
        let d = PrDetails {
            meta: Ok(meta()),
            checks: Ok(Checks {
                items: vec![Check {
                    name: "lint".into(),
                    group: Some("CI".into()),
                    ci: Ci::Fail,
                    label: "failure".into(),
                }],
                more_unknown: false,
            }),
            comments: Ok(Comments {
                items: vec![Comment {
                    author: "carol".into(),
                    created_at: "2026-09-03T00:00:00Z".into(),
                    body: "LGTM".into(),
                }],
                earlier: 4,
                more_unknown: false,
            }),
        };
        let lines = detail_lines(7, Some(&d), false, 60, &p);
        let t = texts(&lines);
        for want in [
            " author   alice · feat/x → main · draft",
            " review   changes requested · bob (changes requested)",
            " labels   [bug] [ui]",
            " size     +12 −3 · 2 files",
            " dates    created 2026-09-01 · updated 2026-09-02",
            " description",
            " ☑ done",
            " checks (1)",
            " ✗ CI / lint  failure",
            " comments (5)",
            " … 4 earlier comments",
            " carol · 2026-09-03",
            "   LGTM",
        ] {
            assert!(t.iter().any(|l| l == want), "missing {want:?} in {t:#?}");
        }
        assert!(!t.iter().any(|l| l.contains("template")));
        for narrow in [24, 12] {
            let lines = detail_lines(7, Some(&d), false, narrow, &p);
            assert!(lines.iter().all(|l| l.width() <= narrow), "{:#?}", texts(&lines));
        }

        // A capped job list says there may be more.
        let mut d = d;
        if let Ok(c) = &mut d.checks {
            c.more_unknown = true;
        }
        let t = texts(&detail_lines(7, Some(&d), false, 60, &p));
        assert!(t.iter().any(|l| l == " checks (1+)"), "{t:#?}");
    }

    #[test]
    fn detail_errors_render_inline_and_once() {
        let p = Palette::catppuccin();
        let e = ProviderError::new("gh is not installed", Some("brew install gh".into()));
        let t = texts(&detail_lines(7, Some(&PrDetails::failed(e)), false, 80, &p));
        assert_eq!(t.iter().filter(|l| l.contains("gh is not installed")).count(), 1, "{t:#?}");
        assert!(t[0].contains("fix: brew install gh"));

        // Partial: meta loaded, comments failed.
        let d = PrDetails {
            meta: Ok(meta()),
            checks: Ok(Checks::default()),
            comments: Err(ProviderError::new("glab is not authenticated for gitlab.com", None)),
        };
        let t = texts(&detail_lines(7, Some(&d), false, 80, &p));
        let at = t.iter().position(|l| l == " comments").expect("comments header");
        assert!(t[at + 1].contains("glab is not authenticated"), "{t:#?}");
        assert!(t.iter().any(|l| l == " no checks reported"));
    }

    #[test]
    fn detail_loading_row() {
        let p = Palette::catppuccin();
        assert_eq!(texts(&detail_lines(42, None, true, 80, &p)), [" loading #42…"]);
    }
}
