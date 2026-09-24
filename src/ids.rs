//! Identifiers shared with `herdr-plugin.toml`.
//!
//! Herdr reads only the manifest; the code repeats these values as literals.
//! `tests/manifest.rs` reads the manifest back and pins it against these
//! constants and against the argv the code actually sends.

pub const PLUGIN_ID: &str = "tarektouati.pr-modal";
/// `[[actions]]` id of the keybinding entry point.
pub const ACTION_OPEN: &str = "open";
/// `[[panes]]` id of the popup that runs the TUI.
pub const PANE_MODAL: &str = "modal";
/// Where `[[build]]` (herdr/install.sh) leaves the binary; every manifest
/// command execs it.
pub const BINARY_PATH: &str = "bin/herdr-pr-modal";

/// Globally unique action id, as used by `[[keys.command]] command = …`.
pub fn qualified(action: &str) -> String {
    format!("{PLUGIN_ID}.{action}")
}
