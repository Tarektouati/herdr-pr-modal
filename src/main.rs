use herdr_pr_modal::cmd::{Runner, SystemRunner};
use herdr_pr_modal::config::Config;
use herdr_pr_modal::context::Origin;
use herdr_pr_modal::{herdr, setup, tui};
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "usage: herdr-pr-modal <open|modal|setup [--key KEY]>
  open   plugin action: open the PR popup over the focused pane
  modal  popup entrypoint: run the PR picker TUI
  setup  print the [[keys.command]] snippet for ~/.config/herdr/config.toml";

/// Action entry point. Runs on the Herdr server with no TTY: it only opens
/// the popup and forwards the origin pane context into it.
fn open() -> i32 {
    let get = |k: &str| std::env::var(k).ok();
    let origin = Origin::from_action_env(get);
    let config_dir = get("HERDR_PLUGIN_CONFIG_DIR").map(PathBuf::from);
    let cfg = match Config::load(config_dir.as_deref()) {
        Ok((cfg, warnings)) => {
            for w in warnings {
                eprintln!("herdr-pr-modal: {w}");
            }
            cfg
        }
        // The popup reports the config error itself.
        Err(_) => Config::default(),
    };
    let args = herdr::pane_open_args(&origin, &cfg);
    match herdr::run(&SystemRunner as &dyn Runner, &herdr::bin(), &args) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("herdr-pr-modal: {e}");
            1
        }
    }
}

/// Keep a failure visible instead of letting the popup vanish or go blank.
fn hold(message: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "\r\n herdr-pr-modal failed: {message}\r\n\r\n press any key to close\r\n");
    let _ = out.flush();
    let _ = crossterm::terminal::enable_raw_mode();
    let _ = crossterm::event::read();
    let _ = crossterm::terminal::disable_raw_mode();
}

fn modal() -> i32 {
    match std::panic::catch_unwind(tui::run_modal) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            hold(&e.to_string());
            1
        }
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "internal error".into());
            hold(&msg);
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("open") => open(),
        Some("modal") => modal(),
        Some("setup") => {
            let key = match args.get(1).map(String::as_str) {
                Some("--key") => match args.get(2) {
                    Some(k) => k.clone(),
                    None => {
                        eprintln!("{USAGE}");
                        std::process::exit(2);
                    }
                },
                _ => setup::DEFAULT_KEY.to_string(),
            };
            print!("{}", setup::snippet(&key));
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}
