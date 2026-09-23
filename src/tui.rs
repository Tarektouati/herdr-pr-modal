//! The popup's event loop. Slow work runs on worker threads and reports back
//! over a channel, so the modal draws its loading row immediately.

use crate::app::{Action, App, ErrRow};
use crate::checkout::{self, Opened};
use crate::cmd::SystemRunner;
use crate::config::Config;
use crate::context::Origin;
use crate::herdr;
use crate::model::{Listing, Pr};
use crate::session::{self, RepoInfo};
use crate::{provider, theme, ui};
use crossterm::event::{self, Event, KeyEventKind};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

enum Msg {
    Repo(Result<RepoInfo, ErrRow>),
    Listing(Result<(Listing, Option<u64>), ErrRow>),
    Marks(HashSet<u64>),
    Opened(Result<Opened, String>),
}

struct Env {
    cfg: Config,
    state_dir: Option<PathBuf>,
}

fn spawn_load(tx: &Sender<Msg>, env: &std::sync::Arc<Env>, info: RepoInfo, force: bool) {
    let (tx, env) = (tx.clone(), env.clone());
    std::thread::spawn(move || {
        let runner = SystemRunner;
        let result = session::load(&runner, &info, &env.cfg, env.state_dir.as_deref(), force);
        let listing = result.as_ref().ok().map(|(l, _)| l.clone());
        let _ = tx.send(Msg::Listing(result));
        if let Some(listing) = listing {
            let _ = tx.send(Msg::Marks(session::worktree_marks(&runner, &info, &env.cfg, &listing)));
        }
    });
}

fn spawn_open(tx: &Sender<Msg>, env: &std::sync::Arc<Env>, info: RepoInfo, pr: Pr) {
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
    let env = std::sync::Arc::new(Env { cfg, state_dir: get("HERDR_PLUGIN_STATE_DIR").map(PathBuf::from) });
    let palette = theme::load();
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut info: Option<RepoInfo> = None;

    if app.fatal.is_none() {
        let (tx, env, cwd) = (tx.clone(), env.clone(), origin.cwd.clone());
        std::thread::spawn(move || {
            let runner = SystemRunner;
            let _ =
                tx.send(Msg::Repo(session::resolve_repo(&runner, cwd.as_deref(), &env.cfg, &config_path)));
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
                match app.handle_key(key, page) {
                    Action::Quit => return Ok(()),
                    Action::None => {}
                    Action::Refresh => {
                        if let Some(i) = &info {
                            app.loading = true;
                            spawn_load(&tx, &env, i.clone(), true);
                        }
                    }
                    Action::Open(idx) => {
                        if let (Some(i), Some(listing)) = (&info, &app.listing) {
                            let pr = listing.prs[idx].clone();
                            app.busy = Some(format!("opening #{} · fetching {}…", pr.number, pr.head_branch));
                            spawn_open(&tx, &env, i.clone(), pr);
                        }
                    }
                }
            }

            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::Repo(Ok(i)) => {
                        app.repo_label = Some(i.repo.path.clone());
                        spawn_load(&tx, &env, i.clone(), false);
                        info = Some(i);
                    }
                    Msg::Repo(Err(e)) => {
                        app.loading = false;
                        app.fatal = Some(e);
                    }
                    Msg::Listing(Ok((listing, age))) => {
                        app.loading = false;
                        app.set_listing(listing, age);
                    }
                    Msg::Listing(Err(e)) => {
                        app.loading = false;
                        if app.listing.is_some() {
                            app.banner = Some(e);
                        } else {
                            app.load_error = Some(e);
                        }
                    }
                    Msg::Marks(marks) => app.worktree_prs = marks,
                    // Herdr already focused the PR workspace; closing the
                    // popup reveals it.
                    Msg::Opened(Ok(_)) => return Ok(()),
                    Msg::Opened(Err(e)) => {
                        app.busy = None;
                        app.banner = Some(ErrRow::new(e, None));
                    }
                }
            }
        }
    })();
    ratatui::restore();
    result
}
