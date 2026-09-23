//! `herdr-plugin.toml` is the only thing Herdr reads; the code repeats its
//! ids as constants. These tests read the manifest back and pin it against
//! the constants *and* against the argv the code actually sends, so a
//! literal inlined at a call site cannot drift unnoticed.

use herdr_pr_modal::config::Config;
use herdr_pr_modal::context::Origin;
use herdr_pr_modal::herdr;
use herdr_pr_modal::ids::{ACTION_OPEN, BINARY_PATH, PANE_MODAL, PLUGIN_ID, qualified};
use herdr_pr_modal::setup;
use serde::Deserialize;

#[derive(Deserialize)]
struct Entry {
    id: String,
    command: Vec<String>,
    #[serde(default)]
    placement: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct Build {
    command: Vec<String>,
}

#[derive(Deserialize)]
struct Manifest {
    id: String,
    min_herdr_version: String,
    build: Vec<Build>,
    actions: Vec<Entry>,
    panes: Vec<Entry>,
}

fn manifest() -> Manifest {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    toml::from_str(&text).unwrap_or_else(|e| panic!("cannot parse {path}: {e}"))
}

fn flag<'a>(argv: &'a [String], name: &str) -> Vec<&'a str> {
    argv.windows(2).filter(|w| w[0] == name).map(|w| w[1].as_str()).collect()
}

#[test]
fn plugin_id_matches() {
    assert_eq!(manifest().id, PLUGIN_ID);
}

#[test]
fn open_action_is_declared_and_runs_open() {
    let m = manifest();
    let action = m.actions.iter().find(|a| a.id == ACTION_OPEN).expect("manifest lacks the open action");
    assert_eq!(action.command, [BINARY_PATH, "open"]);
}

#[test]
fn modal_pane_is_a_popup_that_runs_modal() {
    let m = manifest();
    let pane = m.panes.iter().find(|p| p.id == PANE_MODAL).expect("manifest lacks the modal pane");
    assert_eq!(pane.placement.as_deref(), Some("popup"));
    assert_eq!(pane.command, [BINARY_PATH, "modal"]);
    // Herdr rejects empty or whitespace titles; U+200B is not whitespace.
    assert!(!pane.title.as_deref().unwrap_or("").trim().is_empty());
}

#[test]
fn ids_are_local_and_unique() {
    let m = manifest();
    for e in m.actions.iter().chain(&m.panes) {
        assert!(!e.id.contains('.'), "local id {:?} must not contain dots", e.id);
    }
    let mut ids: Vec<_> = m.actions.iter().map(|a| &a.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), m.actions.len(), "duplicate action ids");
}

#[test]
fn build_output_is_what_commands_exec() {
    let m = manifest();
    assert!(m.build.iter().any(|b| b.command.starts_with(&[
        "cargo".into(),
        "build".into(),
        "--release".into()
    ])));
    let bin = concat!("target/release/", env!("CARGO_PKG_NAME"));
    assert_eq!(BINARY_PATH, bin);
    assert!(m.min_herdr_version.split('.').count() == 3);
}

#[test]
fn pane_open_sends_manifest_ids_and_forwards_origin() {
    let m = manifest();
    let origin = Origin {
        pane_id: Some("w1:p2".into()),
        tab_id: Some("w1:t1".into()),
        workspace_id: Some("w1".into()),
        cwd: Some("/repo".into()),
    };
    let argv = herdr::pane_open_args(&origin, &Config::default());
    assert_eq!(&argv[..3], ["plugin", "pane", "open"]);
    assert_eq!(flag(&argv, "--plugin"), [m.id.as_str()]);
    let entry = flag(&argv, "--entrypoint");
    assert_eq!(entry.len(), 1);
    assert!(m.panes.iter().any(|p| p.id == entry[0]), "entrypoint {entry:?} is not a manifest pane");
    assert_eq!(flag(&argv, "--placement"), ["popup"]);
    let env = flag(&argv, "--env");
    for want in [
        "PRM_ORIGIN_PANE_ID=w1:p2",
        "PRM_ORIGIN_TAB_ID=w1:t1",
        "PRM_ORIGIN_WORKSPACE_ID=w1",
        "PRM_ORIGIN_CWD=/repo",
    ] {
        assert!(env.contains(&want), "missing --env {want}: {env:?}");
    }
}

#[test]
fn setup_snippet_names_a_manifest_action() {
    let m = manifest();
    let v: toml::Value = toml::from_str(&setup::snippet(setup::DEFAULT_KEY)).unwrap();
    let command = v["keys"]["command"][0]["command"].as_str().unwrap();
    let (plugin, action) = command.rsplit_once('.').unwrap();
    assert_eq!(plugin, m.id);
    assert!(m.actions.iter().any(|a| a.id == action), "{command} is not a manifest action");
    assert_eq!(command, qualified(ACTION_OPEN));
}
