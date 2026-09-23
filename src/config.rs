//! Plugin config, read from `$HERDR_PLUGIN_CONFIG_DIR/config.toml`.
//! Every key is optional; a missing file means all defaults.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_SIZE: &str = "70%";
pub const DEFAULT_CACHE_TTL_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderChoice {
    #[default]
    Auto,
    Github,
    Gitlab,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Popup outer width: cells (`100`) or percent (`"70%"`).
    pub width: String,
    /// Popup outer height: cells (`30`) or percent (`"70%"`).
    pub height: String,
    /// Seconds a PR listing is reused before the next open refetches it.
    pub cache_ttl_secs: u64,
    /// Force the provider instead of detecting it from the remote host.
    pub provider: ProviderChoice,
    /// Git remote that PRs target and heads are fetched from.
    pub remote: String,
    /// Parent directory for new worktrees. Unset: Herdr's `[worktrees]
    /// directory` convention (`<dir>/<repo>/<branch-slug>`).
    pub worktree_dir: Option<String>,
    /// Command typed into the new workspace's first pane after creation.
    /// Off by default. Not run when switching to an existing worktree.
    pub post_create_command: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            width: DEFAULT_SIZE.into(),
            height: DEFAULT_SIZE.into(),
            cache_ttl_secs: DEFAULT_CACHE_TTL_SECS,
            provider: ProviderChoice::Auto,
            remote: "origin".into(),
            worktree_dir: None,
            post_create_command: None,
        }
    }
}

/// Herdr accepts `N` cells or `1%`..`100%` (schema `PopupSize`).
pub fn valid_popup_size(s: &str) -> bool {
    if let Some(pct) = s.strip_suffix('%') {
        return matches!(pct.parse::<u16>(), Ok(1..=100)) && !pct.starts_with('0');
    }
    s.parse::<u16>().is_ok()
}

impl Config {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("config.toml")
    }

    /// Parse config text. Invalid sizes fall back to the default with a
    /// warning instead of failing the whole modal.
    pub fn parse(text: &str) -> Result<(Config, Vec<String>), String> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| e.message().to_string())?;
        let mut warnings = Vec::new();
        for (name, value) in [("width", &mut cfg.width), ("height", &mut cfg.height)] {
            if !valid_popup_size(value) {
                warnings.push(format!("{name} = {value:?} is not N or N%; using {DEFAULT_SIZE}"));
                *value = DEFAULT_SIZE.into();
            }
        }
        cfg.post_create_command = cfg.post_create_command.filter(|c| !c.trim().is_empty());
        cfg.worktree_dir = cfg.worktree_dir.filter(|d| !d.trim().is_empty());
        Ok((cfg, warnings))
    }

    /// Load from the plugin config dir. A missing file is not an error.
    pub fn load(config_dir: Option<&Path>) -> Result<(Config, Vec<String>), String> {
        let Some(dir) = config_dir else {
            return Ok((Config::default(), Vec::new()));
        };
        let path = Self::path(dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Config::default(), Vec::new())),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_all_defaults() {
        let (cfg, warnings) = Config::parse("").unwrap();
        assert_eq!(cfg, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_every_key() {
        let (cfg, _) = Config::parse(
            r#"
width = "80%"
height = "30"
cache_ttl_secs = 5
provider = "gitlab"
remote = "upstream"
worktree_dir = "~/wt"
post_create_command = "pnpm install"
"#,
        )
        .unwrap();
        assert_eq!(cfg.width, "80%");
        assert_eq!(cfg.height, "30");
        assert_eq!(cfg.cache_ttl_secs, 5);
        assert_eq!(cfg.provider, ProviderChoice::Gitlab);
        assert_eq!(cfg.remote, "upstream");
        assert_eq!(cfg.worktree_dir.as_deref(), Some("~/wt"));
        assert_eq!(cfg.post_create_command.as_deref(), Some("pnpm install"));
    }

    #[test]
    fn bad_sizes_fall_back_with_warning() {
        let (cfg, warnings) = Config::parse("width = \"120%\"\nheight = \"big\"").unwrap();
        assert_eq!(cfg.width, DEFAULT_SIZE);
        assert_eq!(cfg.height, DEFAULT_SIZE);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("widht = \"70%\"").is_err());
    }

    #[test]
    fn popup_size_rules_match_herdr() {
        for ok in ["1%", "70%", "100%", "0", "120"] {
            assert!(valid_popup_size(ok), "{ok}");
        }
        for bad in ["0%", "07%", "101%", "%", "-5", "70 %"] {
            assert!(!valid_popup_size(bad), "{bad}");
        }
    }
}
