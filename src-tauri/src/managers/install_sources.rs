//! Which service delivered each installed game's ticket — Ryuu or Hubcap.
//!
//! Recorded at the moment a ticket is actually installed, because that is the
//! only point where the answer is known for certain. With two catalogs and a
//! fallback between them, the source a game was *requested* from is not
//! necessarily the one that supplied it: Hubcap out of downloads for the day
//! hands the install to Ryuu, and Ryuu leaving a manifest missing hands it to
//! Hubcap.

use std::collections::HashMap;
use std::path::PathBuf;

fn record_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("com.ragnarok.launcher")
            .join("install_sources.json"),
    )
}

/// Every recorded app id → `"ryuu"` or `"hubcap"`.
pub fn load() -> HashMap<String, String> {
    let Some(path) = record_path() else { return HashMap::new() };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Notes which source installed `app_id`. A later install overwrites it,
/// which is what should happen: the label describes the ticket in place now.
pub fn record(app_id: &str, source: &str) {
    // The install path is exercised by tests against throwaway folders. Those
    // must never write into the user's real record — the same kind of leak that
    // once made test output look like real installs in ragnarok.log.
    if cfg!(test) {
        return;
    }
    let Some(path) = record_path() else { return };
    let mut map = load();
    map.insert(app_id.to_string(), source.trim().to_ascii_lowercase());

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Temp file plus rename, so an interrupted write cannot leave a truncated
    // file that would read back as "no labels at all".
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(&map) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
