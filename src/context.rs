//! The origin pane/tab/workspace the modal was opened from.
//!
//! The action process sees the origin through `HERDR_PLUGIN_CONTEXT_JSON` and
//! `HERDR_*_ID`. The popup process gets no `HERDR_PANE_ID`, so the action
//! forwards the origin explicitly as `PRM_ORIGIN_*` env vars on
//! `plugin pane open`. The values stay scoped to one invocation: two modals
//! opened at once cannot overwrite each other's origin.

use serde::Deserialize;

pub const ENV_PANE: &str = "PRM_ORIGIN_PANE_ID";
pub const ENV_TAB: &str = "PRM_ORIGIN_TAB_ID";
pub const ENV_WORKSPACE: &str = "PRM_ORIGIN_WORKSPACE_ID";
pub const ENV_CWD: &str = "PRM_ORIGIN_CWD";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Origin {
    pub pane_id: Option<String>,
    pub tab_id: Option<String>,
    pub workspace_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct InvocationContext {
    focused_pane_id: Option<String>,
    focused_pane_cwd: Option<String>,
    tab_id: Option<String>,
    workspace_id: Option<String>,
    workspace_cwd: Option<String>,
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

fn from_context_json(get: &dyn Fn(&str) -> Option<String>) -> Origin {
    let ctx: InvocationContext =
        get("HERDR_PLUGIN_CONTEXT_JSON").and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    Origin {
        pane_id: non_empty(ctx.focused_pane_id),
        tab_id: non_empty(ctx.tab_id),
        workspace_id: non_empty(ctx.workspace_id),
        cwd: non_empty(ctx.focused_pane_cwd).or(non_empty(ctx.workspace_cwd)),
    }
}

impl Origin {
    /// Origin as seen by the action process: context JSON first, then the
    /// plain `HERDR_*_ID` vars.
    pub fn from_action_env(get: impl Fn(&str) -> Option<String>) -> Origin {
        let ctx = from_context_json(&get);
        Origin {
            pane_id: ctx.pane_id.or_else(|| non_empty(get("HERDR_PANE_ID"))),
            tab_id: ctx.tab_id.or_else(|| non_empty(get("HERDR_TAB_ID"))),
            workspace_id: ctx.workspace_id.or_else(|| non_empty(get("HERDR_WORKSPACE_ID"))),
            cwd: ctx.cwd,
        }
    }

    /// Origin as seen by the popup: forwarded `PRM_ORIGIN_*` first, then the
    /// popup's own context JSON (it describes the underlying tiled pane), then
    /// `HERDR_ACTIVE_*` for a popup launched straight from a `type = "popup"`
    /// keybinding.
    pub fn from_popup_env(get: impl Fn(&str) -> Option<String>) -> Origin {
        let ctx = from_context_json(&get);
        let pick = |fwd: &str, ctx_val: Option<String>, active: &str| {
            non_empty(get(fwd)).or(ctx_val).or_else(|| non_empty(get(active)))
        };
        Origin {
            pane_id: pick(ENV_PANE, ctx.pane_id, "HERDR_ACTIVE_PANE_ID"),
            tab_id: pick(ENV_TAB, ctx.tab_id, "HERDR_ACTIVE_TAB_ID"),
            workspace_id: pick(ENV_WORKSPACE, ctx.workspace_id, "HERDR_ACTIVE_WORKSPACE_ID"),
            cwd: pick(ENV_CWD, ctx.cwd, "HERDR_ACTIVE_PANE_CWD"),
        }
    }

    /// `KEY=VALUE` pairs to pass as `--env` on `plugin pane open`.
    pub fn to_env(&self) -> Vec<(String, String)> {
        [
            (ENV_PANE, &self.pane_id),
            (ENV_TAB, &self.tab_id),
            (ENV_WORKSPACE, &self.workspace_id),
            (ENV_CWD, &self.cwd),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| (k.to_string(), v.clone())))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn action_reads_context_json_and_falls_back_to_ids() {
        let ctx = r#"{"focused_pane_id":"w1:p2","tab_id":"w1:t1","workspace_cwd":"/ws","focused_pane_cwd":"/repo"}"#;
        let o = Origin::from_action_env(env(&[
            ("HERDR_PLUGIN_CONTEXT_JSON", ctx),
            ("HERDR_WORKSPACE_ID", "w1"),
            ("HERDR_PANE_ID", "ignored"),
        ]));
        assert_eq!(o.pane_id.as_deref(), Some("w1:p2"));
        assert_eq!(o.tab_id.as_deref(), Some("w1:t1"));
        assert_eq!(o.workspace_id.as_deref(), Some("w1"));
        assert_eq!(o.cwd.as_deref(), Some("/repo"));
    }

    #[test]
    fn popup_prefers_forwarded_values_over_its_own_context() {
        let o = Origin::from_popup_env(env(&[
            (ENV_PANE, "w1:p2"),
            (ENV_CWD, "/repo"),
            ("HERDR_PLUGIN_CONTEXT_JSON", r#"{"focused_pane_id":"w9:p9","tab_id":"w9:t1"}"#),
            ("HERDR_ACTIVE_WORKSPACE_ID", "w7"),
        ]));
        assert_eq!(o.pane_id.as_deref(), Some("w1:p2"));
        assert_eq!(o.tab_id.as_deref(), Some("w9:t1"));
        assert_eq!(o.workspace_id.as_deref(), Some("w7"));
        assert_eq!(o.cwd.as_deref(), Some("/repo"));
    }

    #[test]
    fn to_env_round_trips_through_popup() {
        let o = Origin {
            pane_id: Some("w1:p1".into()),
            tab_id: None,
            workspace_id: Some("w1".into()),
            cwd: Some("/r".into()),
        };
        let pairs = o.to_env();
        let map: HashMap<String, String> = pairs.into_iter().collect();
        let back = Origin::from_popup_env(|k| map.get(k).cloned());
        assert_eq!(back, o);
    }
}
