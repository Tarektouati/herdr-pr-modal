//! Per-repo PR listing cache under `$HERDR_PLUGIN_STATE_DIR/cache/`.
//! Holds PR metadata only, never credentials.

use crate::model::Listing;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    fetched_at: u64,
    listing: Listing,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn file_for(state_dir: &Path, repo_key: &str) -> PathBuf {
    let name: String = repo_key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    state_dir.join("cache").join(format!("{name}.json"))
}

/// Cached listing if younger than `ttl_secs`, with its age in seconds.
pub fn read(state_dir: &Path, repo_key: &str, ttl_secs: u64, now: u64) -> Option<(Listing, u64)> {
    let text = std::fs::read_to_string(file_for(state_dir, repo_key)).ok()?;
    let entry: Entry = serde_json::from_str(&text).ok()?;
    let age = now.saturating_sub(entry.fetched_at);
    (age < ttl_secs).then_some((entry.listing, age))
}

/// Best effort; a failed write only costs a refetch next time.
pub fn write(state_dir: &Path, repo_key: &str, listing: &Listing, now: u64) {
    let path = file_for(state_dir, repo_key);
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let entry = Entry { fetched_at: now, listing: listing.clone() };
    if let Ok(text) = serde_json::to_string(&entry) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_ttl_and_isolates_repos() {
        let dir = tempfile::tempdir().unwrap();
        let listing = Listing { viewer: "me".into(), prs: vec![] };
        write(dir.path(), "github.com/a/b", &listing, 1000);
        assert_eq!(read(dir.path(), "github.com/a/b", 60, 1030), Some((listing.clone(), 30)));
        assert_eq!(read(dir.path(), "github.com/a/b", 60, 1060), None);
        assert_eq!(read(dir.path(), "github.com/a/c", 60, 1030), None);
        assert_eq!(read(dir.path(), "github.com/a/b", 0, 1000), None);
    }
}
