//! The popup's event loop. Slow work runs on worker threads and reports back
//! over a channel, so the modal draws its loading row immediately.

use crate::app::{Action, App, ErrRow, Tab};
use crate::checkout::{self, Opened};
use crate::cmd::SystemRunner;
use crate::config::Config;
use crate::context::Origin;
use crate::herdr;
use crate::model::{Listing, Pr, PrDetails};
use crate::session::{self, RepoInfo, TabSeed};
use crate::{provider, theme, ui};
use crossterm::event::{self, Event, KeyEventKind};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

/// Per-repo messages carry their tab index, so results land in the right
/// tab even after the user switched away.
enum Msg {
    Tabs(Result<(Vec<TabSeed>, usize), ErrRow>),
    Resolved(usize, Result<RepoInfo, ErrRow>),
    Listing(usize, Result<(Listing, Option<u64>), ErrRow>),
    Marks(usize, HashSet<u64>),
    Opened(Result<Opened, String>),
    /// Preview details for a PR number. Kept in memory only.
    Details(usize, u64, Box<PrDetails>),
}

struct Env {
    cfg: Config,
    state_dir: Option<PathBuf>,
    config_path: String,
}

/// Load tab `idx`: resolve its remote first if that hasn't succeeded yet,
/// then PRs (cache first unless `force`), then worktree marks.
fn spawn_load(
    tx: &Sender<Msg>,
    env: &Arc<Env>,
    idx: usize,
    root: PathBuf,
    info: Option<RepoInfo>,
    force: bool,
) {
    let (tx, env) = (tx.clone(), env.clone());
    std::thread::spawn(move || {
        let runner = SystemRunner;
        let info = match info {
            Some(info) => info,
            None => {
                let resolved = session::resolve_root(&runner, &root, &env.cfg, &env.config_path);
                let _ = tx.send(Msg::Resolved(idx, resolved.clone()));
                match resolved {
                    Ok(info) => info,
                    Err(_) => return,
                }
            }
        };
        let result = session::load(&runner, &info, &env.cfg, env.state_dir.as_deref(), force);
        let listing = result.as_ref().ok().map(|(l, _)| l.clone());
        let _ = tx.send(Msg::Listing(idx, result));
        if let Some(listing) = listing {
            let _ = tx.send(Msg::Marks(idx, session::worktree_marks(&runner, &info, &env.cfg, &listing)));
        }
    });
}

/// Start the first load of the active tab, if it hasn't had one.
fn start_active(app: &mut App, infos: &[Option<RepoInfo>], tx: &Sender<Msg>, env: &Arc<Env>) {
    let idx = app.active;
    let tab = app.tab_mut();
    if tab.started {
        return;
    }
    tab.started = true;
    tab.loading = true;
    spawn_load(tx, env, idx, tab.root.clone(), infos.get(idx).cloned().flatten(), false);
}

fn spawn_open(tx: &Sender<Msg>, env: &Arc<Env>, info: RepoInfo, pr: Pr) {
    let (tx, env) = (tx.clone(), env.clone());
    std::thread::spawn(move || {
        let runner = SystemRunner;
        let plan = provider::for_kind(info.kind, &runner).fetch_head(&pr, &env.cfg.remote);
        let bin = herdr::bin();
        let req = checkout::Request {
            runner: &runner,
            herdr_bin: &bin,
            repo_root: &info.root,
            cfg: &env.cfg,
            pr: &pr,
            plan: &plan,
        };
        let _ = tx.send(Msg::Opened(checkout::open_pr(&req)));
    });
}

fn spawn_details(tx: &Sender<Msg>, idx: usize, info: RepoInfo, number: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let runner = SystemRunner;
        let details = provider::for_kind(info.kind, &runner).pr_details(&info.repo, &info.root, number);
        let _ = tx.send(Msg::Details(idx, number, Box::new(details)));
    });
}

pub fn run_modal() -> std::io::Result<()> {
    let get = |k: &str| std::env::var(k).ok();
    let origin = Origin::from_popup_env(get);
    let config_dir = get("HERDR_PLUGIN_CONFIG_DIR").map(PathBuf::from);
    let config_path = config_dir
        .as_deref()
        .map(|d| Config::path(d).display().to_string())
        .unwrap_or_else(|| "the plugin config.toml".into());
    let mut app = App::new();
    let cfg = match Config::load(config_dir.as_deref()) {
        Ok((cfg, _warnings)) => cfg,
        Err(e) => {
            app.fatal = Some(ErrRow::new(format!("invalid config: {e}"), None));
            Config::default()
        }
    };
    let env = Arc::new(Env { cfg, state_dir: get("HERDR_PLUGIN_STATE_DIR").map(PathBuf::from), config_path });
    let palette = theme::load();
    let (tx, rx) = mpsc::channel::<Msg>();
    // Resolved repo per tab; `None` until its first load resolves it.
    let mut infos: Vec<Option<RepoInfo>> = Vec::new();

    if app.fatal.is_none() {
        let (tx, cwd) = (tx.clone(), origin.cwd.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Tabs(session::discover_tabs(&SystemRunner, &herdr::bin(), cwd.as_deref())));
        });
    }

    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        loop {
            terminal.draw(|f| ui::draw(f, &mut app, &palette))?;
            let page = ui::body_height(terminal.size()?.into());

            if event::poll(Duration::from_millis(80))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                let idx = app.active;
                let info = infos.get(idx).cloned().flatten();
                match app.handle_key(key, page) {
                    Action::Quit => return Ok(()),
                    Action::None => {}
                    Action::SwitchTab(_) => start_active(&mut app, &infos, &tx, &env),
                    // Also retries a tab whose remote failed to resolve.
                    Action::Refresh => {
                        if idx < infos.len() {
                            let tab = app.tab_mut();
                            tab.begin_refresh();
                            spawn_load(&tx, &env, idx, tab.root.clone(), info, true);
                        }
                    }
                    Action::Open(i) => {
                        if let (Some(info), Some(listing)) = (info, &app.tab().listing) {
                            let pr = listing.prs[i].clone();
                            app.busy = Some(format!("opening #{} · fetching {}…", pr.number, pr.head_branch));
                            spawn_open(&tx, &env, info, pr);
                        }
                    }
                    // `App` already marked the PR as loading.
                    Action::Preview(i) | Action::RefreshDetail(i) => {
                        if let (Some(info), Some(listing)) = (info, &app.tab().listing) {
                            spawn_details(&tx, idx, info, listing.prs[i].number);
                        }
                    }
                }
            }

            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::Tabs(Ok((seeds, active))) => {
                        infos = vec![None; seeds.len()];
                        let tabs = seeds.into_iter().map(|s| Tab::new(s.label, s.root)).collect();
                        app.set_tabs(tabs, active);
                        start_active(&mut app, &infos, &tx, &env);
                    }
                    Msg::Tabs(Err(e)) => {
                        app.tab_mut().loading = false;
                        app.fatal = Some(e);
                    }
                    Msg::Resolved(idx, result) => {
                        let label = result.as_ref().map(|info| info.repo.path.clone()).map_err(Clone::clone);
                        if let (Some(slot), Ok(info)) = (infos.get_mut(idx), result) {
                            *slot = Some(info);
                        }
                        app.on_resolved(idx, label);
                    }
                    Msg::Listing(idx, result) => app.on_listing(idx, result),
                    Msg::Marks(idx, marks) => app.on_marks(idx, marks),
                    // Herdr already focused the PR workspace; closing the
                    // popup reveals it.
                    Msg::Opened(Ok(_)) => return Ok(()),
                    Msg::Opened(Err(e)) => {
                        app.busy = None;
                        app.tab_mut().banner = Some(ErrRow::new(e, None));
                    }
                    Msg::Details(idx, number, details) => app.on_detail(idx, number, *details),
                }
            }
        }
    })();
    ratatui::restore();
    result
}
