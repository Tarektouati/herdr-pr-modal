//! `setup`: print the keybinding snippet. Herdr manifests cannot declare
//! keys, and this command never edits the user's config.

use crate::ids::{ACTION_OPEN, qualified};

/// Unbound in Herdr 0.9.1 defaults (`prefix+p` is previous_tab).
pub const DEFAULT_KEY: &str = "prefix+alt+p";

pub fn snippet(key: &str) -> String {
    format!(
        r#"# Add to ~/.config/herdr/config.toml, then run: herdr server reload-config
[[keys.command]]
key = "{key}"
type = "plugin_action"
command = "{action}"
description = "open PRs"
"#,
        action = qualified(ACTION_OPEN)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_is_valid_toml_with_plugin_action() {
        let v: toml::Value = toml::from_str(&snippet("prefix+x")).unwrap();
        let entry = &v["keys"]["command"][0];
        assert_eq!(entry["type"].as_str(), Some("plugin_action"));
        assert_eq!(entry["key"].as_str(), Some("prefix+x"));
        assert_eq!(entry["command"].as_str(), Some("tarektouati.pr-modal.open"));
    }
}
