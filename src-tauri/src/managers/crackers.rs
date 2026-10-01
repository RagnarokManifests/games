// Local achievement-tracking files written by various DRM-free/crack tools,
// independent of Steam's own client — useful for games that don't go
// through Ragnarok's live-Steam-client ownership method at all (e.g. pure
// DRM-free cracks added via Bypass, where Steam has no idea the game is
// "owned" and our SAM-style sync simply doesn't apply). Modeled directly on
// github.com/hydralauncher/hydra's own cracker detection (MIT licensed) —
// ported all 11 sources it actually has working parsers for: CODEX, RUNE,
// OnlineFix, RLD, CreamAPI, SKIDROW, EMPRESS, Razor1911, UserStats, 3DM, and
// Steam's own local achievements-panel cache (Goldberg/GSE itself is handled
// separately in main.rs, not here). Two more crackers Hydra *detects* files
// for (SmartSteamEmu, RLE) were deliberately skipped — Hydra's own parser has
// no case for them either, so those paths are dead code even upstream.
// Read-only — this never writes achievement state, only supplements what we
// can show as an additional fallback source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
enum Cracker {
    Codex,
    Rune,
    OnlineFix,
    Rld,
    CreamApi,
    Skidrow,
    Empress,
    Razor1911,
    /// `<game exe dir>/../SteamData/user_stats.ini` — found relative to the
    /// game's own executable, not a fixed %AppData% path like the others.
    UserStats,
    /// `<game exe dir>/../3DMGAME/Player/stats/achievements.ini` — same
    /// exe-relative discovery as UserStats.
    ThreeDm,
}

fn candidate_paths(cracker: Cracker, app_id: &str) -> Vec<PathBuf> {
    let roaming = dirs::data_dir();
    let local = dirs::data_local_dir();
    let documents = dirs::document_dir();
    let public_documents = PathBuf::from(r"C:\Users\Public\Documents");
    let program_data = PathBuf::from(r"C:\ProgramData");

    let mut out = Vec::new();
    match cracker {
        Cracker::Codex => {
            out.push(public_documents.join("Steam").join("CODEX").join(app_id).join("achievements.ini"));
            if let Some(r) = &roaming {
                out.push(r.join("Steam").join("CODEX").join(app_id).join("achievements.ini"));
            }
        }
        Cracker::Rune => {
            out.push(public_documents.join("Steam").join("RUNE").join(app_id).join("achievements.ini"));
        }
        Cracker::OnlineFix => {
            out.push(public_documents.join("OnlineFix").join(app_id).join("Stats").join("Achievements.ini"));
            out.push(public_documents.join("OnlineFix").join(app_id).join("Achievements.ini"));
        }
        Cracker::Rld => {
            out.push(program_data.join("RLD!").join(app_id).join("achievements.ini"));
            out.push(program_data.join("Steam").join("Player").join(app_id).join("stats").join("achievements.ini"));
            out.push(program_data.join("Steam").join("RLD!").join(app_id).join("stats").join("achievements.ini"));
            out.push(program_data.join("Steam").join("dodi").join(app_id).join("stats").join("achievements.ini"));
        }
        Cracker::CreamApi => {
            if let Some(r) = &roaming {
                out.push(r.join("CreamAPI").join(app_id).join("stats").join("CreamAPI.Achievements.cfg"));
            }
        }
        Cracker::Skidrow => {
            if let Some(d) = &documents {
                out.push(d.join("SKIDROW").join(app_id).join("SteamEmu").join("UserStats").join("achiev.ini"));
                out.push(d.join("Player").join(app_id).join("SteamEmu").join("UserStats").join("achiev.ini"));
            }
            if let Some(l) = &local {
                out.push(l.join("SKIDROW").join(app_id).join("SteamEmu").join("UserStats").join("achiev.ini"));
            }
        }
        Cracker::Empress => {
            if let Some(r) = &roaming {
                out.push(r.join("EMPRESS").join("remote").join(app_id).join("achievements.json"));
            }
            out.push(public_documents.join("EMPRESS").join(app_id).join("remote").join(app_id).join("achievements.json"));
        }
        Cracker::Razor1911 => {
            if let Some(r) = &roaming {
                out.push(r.join(".1911").join(app_id).join("achievement"));
            }
        }
        // Handled separately in exe_relative_paths() — needs the game's
        // actual install folder, not a fixed %AppData%-relative path.
        Cracker::UserStats | Cracker::ThreeDm => {}
    }
    out
}

/// Reads `installdir` out of an AppID's ACF manifest, across every configured
/// Steam library, and returns the resulting `steamapps/common/<installdir>`
/// folder — the same folder Ragnarok's own installer/unlocker code treats as
/// "the game folder" elsewhere.
pub(crate) fn find_game_folder(steam_path: &str, app_id: &str) -> Option<PathBuf> {
    for lib in crate::managers::manifest::ManifestManager::get_library_folders(steam_path) {
        let acf = PathBuf::from(&lib).join("steamapps").join(format!("appmanifest_{}.acf", app_id));
        let Ok(content) = std::fs::read_to_string(&acf) else { continue };

        let read_field = |key: &str| -> Option<String> {
            content.lines().find_map(|l| {
                let l = l.trim();
                if !l.starts_with(&format!("\"{}\"", key)) {
                    return None;
                }
                let parts: Vec<&str> = l.splitn(4, '"').collect();
                parts.get(3).map(|s| s.trim_end_matches('"').to_string())
            })
        };

        let Some(dir) = read_field("installdir") else { continue };

        // This was a bare `flags == 4`, the third copy of a check that had
        // already drifted once with real consequences: a genuinely installed
        // game sitting at 1030 (UpdateRunning | UpdateRequired |
        // FullyInstalled) was reported as not installed, so every crack and
        // bypass action silently failed to find its folder. manifest.rs holds
        // the one correct implementation — a bit test — and this now defers to
        // it rather than keeping a copy that can drift again. A missing field
        // still counts as installed, since some ACFs Ragnarok itself writes
        // don't set one.
        let fully_installed = if content.contains("\"StateFlags\"") {
            crate::managers::manifest::ManifestManager::acf_reports_installed(&content)
        } else {
            true
        };

        let folder = PathBuf::from(&lib).join("steamapps").join("common").join(dir);
        if fully_installed && folder.exists() && folder_has_content(&folder) {
            return Some(folder);
        }
    }
    None
}

/// True if any currently-running process's executable lives inside `folder`.
/// Used to refuse writing into a game's install folder (bypass auto-apply,
/// unlockers) while it's actually running, mirroring the same exe-path match
/// `resolve_running_appid_via_process_match` uses elsewhere.
pub(crate) fn is_game_running(folder: &Path) -> bool {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_processes();
    sys.processes()
        .values()
        .any(|p| p.exe().map(|e| e.starts_with(folder)).unwrap_or(false))
}

/// True if `dir` contains at least one regular file within a few levels of
/// nesting. Used to reject "ghost" install folders — Steam sometimes leaves
/// an empty (or download-in-progress) folder behind while the appmanifest
/// still reports the app as installed.
fn folder_has_content(dir: &Path) -> bool {
    fn scan(dir: &Path, depth: u8) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                return true;
            }
            if path.is_dir() && depth > 0 && scan(&path, depth - 1) {
                return true;
            }
        }
        false
    }
    scan(dir, 4)
}

/// UserStats/3DM achievement files sit next to the game's own executable
/// (one level up from wherever the actual .exe lives), not under a fixed
/// %AppData% path — so this needs to locate the game folder first via its
/// ACF, then find the exe inside it, matching the reference implementation's
/// `executablePath/../...` resolution.
/// Resolved exe directories, remembered for the life of the process.
///
/// This is the expensive half of `load_all_cracker_achievements`: locating
/// the game's install folder and reading it to pick out the main .exe. The
/// achievements history recomputes state for every game the user has ever
/// opened the tab for, so without a memo that lookup runs once per game per
/// visit, for a path that has not moved since the app started. A game
/// installed or removed mid-session is the one case this goes stale, and the
/// cost of that is two rarely-used cracker formats not being picked up until
/// the next launch.
static EXE_PARENT_MEMO: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Option<PathBuf>>>> =
    std::sync::OnceLock::new();

fn resolve_exe_parent(steam_path: &str, app_id: &str) -> Option<PathBuf> {
    let memo = EXE_PARENT_MEMO.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let key = format!("{}|{}", steam_path, app_id);

    if let Ok(guard) = memo.lock() {
        if let Some(hit) = guard.get(&key) {
            return hit.clone();
        }
    }

    // Computed outside the lock: this touches the disk, and holding the mutex
    // across it would serialise every caller behind the slowest one.
    let resolved = (|| {
        let game_folder = find_game_folder(steam_path, app_id)?;
        let exe_path = crate::managers::unlockers::RustUnlockerManager::detect_game_exe(
            &game_folder.to_string_lossy(),
        )
        .ok()?;
        PathBuf::from(exe_path).parent()?.parent().map(|p| p.to_path_buf())
    })();

    if let Ok(mut guard) = memo.lock() {
        guard.insert(key, resolved.clone());
    }
    resolved
}

fn exe_relative_paths(steam_path: &str, app_id: &str) -> Vec<(Cracker, PathBuf)> {
    let Some(parent) = resolve_exe_parent(steam_path, app_id) else { return Vec::new() };
    vec![
        (Cracker::UserStats, parent.join("SteamData").join("user_stats.ini")),
        (Cracker::ThreeDm, parent.join("3DMGAME").join("Player").join("stats").join("achievements.ini")),
    ]
}

/// Steam's own local UI cache for the achievements panel — not authoritative
/// (can lag behind real state, and only exists after the panel has been
/// opened at least once), so this is used as the lowest-priority fallback
/// only. Checked for every Steam user profile found on the machine.
/// Steam's user ids, read once. The same handful of profile folders serve
/// every game, so listing them per app_id meant one `read_dir` of userdata
/// for each game in the achievements history.
static STEAM_USER_IDS_MEMO: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<String>>>> =
    std::sync::OnceLock::new();

fn steam_user_ids(steam_path: &str) -> Vec<String> {
    let memo = STEAM_USER_IDS_MEMO.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(guard) = memo.lock() {
        if let Some(hit) = guard.get(steam_path) {
            return hit.clone();
        }
    }

    let userdata = PathBuf::from(steam_path).join("userdata");
    let ids: Vec<String> = std::fs::read_dir(&userdata)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()))
                .collect()
        })
        .unwrap_or_default();

    if let Ok(mut guard) = memo.lock() {
        guard.insert(steam_path.to_string(), ids.clone());
    }
    ids
}

fn steam_cache_paths(steam_path: &str, app_id: &str) -> Vec<PathBuf> {
    let userdata = PathBuf::from(steam_path).join("userdata");
    steam_user_ids(steam_path)
        .into_iter()
        .map(|name| {
            userdata
                .join(&name)
                .join("config")
                .join("librarycache")
                .join(format!("{}.json", app_id))
        })
        .collect()
}

/// Minimal INI parser matching the shape these crackers actually use:
/// `[SectionName]` headers followed by `key=value` lines. Strips a leading
/// BOM and `###` comment lines the same way the reference implementation
/// does.
fn ini_parse(content: &str) -> HashMap<String, HashMap<String, String>> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut result: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut section = String::new();
    for raw_line in content.split(['\r', '\n']) {
        let line = raw_line.trim();
        if line.starts_with("###") || line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            result.entry(section.clone()).or_default();
        } else if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().to_string();
            let value = line[eq + 1..].trim().to_string();
            result.entry(section.clone()).or_default().insert(key, value);
        }
    }
    result
}

/// Reads a hex string (as written by RLD/3DM-style crackers) as a
/// little-endian u32 — mirrors `DataView.getUint32(0, true)` over the
/// hex-decoded bytes in the reference implementation.
fn hex_to_u32_le(hex: &str) -> Option<u32> {
    let bytes: Vec<u8> = (0..hex.len().min(8))
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    if bytes.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Some crackers store a 7-digit unlock time that needs an extra ×1000 to
/// land in the same unit as the rest (observed empirically in the reference
/// implementation — not documented anywhere, just how these tools behave).
fn scale_unlock_time(raw: &str) -> u64 {
    let n: u64 = raw.parse().unwrap_or(0);
    if raw.len() == 7 {
        n * 1000
    } else {
        n
    }
}

fn process_default(parsed: &HashMap<String, HashMap<String, String>>) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    for (name, fields) in parsed {
        if fields.get("Achieved").map(|s| s.as_str()) == Some("1") {
            let t = fields.get("UnlockTime").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            out.insert(name.clone(), (true, t));
        }
    }
    out
}

fn process_online_fix(parsed: &HashMap<String, HashMap<String, String>>) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    for (name, fields) in parsed {
        if fields.get("achieved").map(|s| s.as_str()) == Some("true") {
            let t = fields.get("timestamp").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            out.insert(name.clone(), (true, t));
        } else if fields.get("Achieved").map(|s| s.as_str()) == Some("true") {
            if let Some(raw) = fields.get("TimeUnlocked") {
                out.insert(name.clone(), (true, scale_unlock_time(raw)));
            }
        }
    }
    out
}

fn process_cream_api(parsed: &HashMap<String, HashMap<String, String>>) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    for (name, fields) in parsed {
        if fields.get("achieved").map(|s| s.as_str()) == Some("true") {
            if let Some(raw) = fields.get("unlocktime") {
                out.insert(name.clone(), (true, scale_unlock_time(raw)));
            }
        }
    }
    out
}

fn process_skidrow(parsed: &HashMap<String, HashMap<String, String>>) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    if let Some(section) = parsed.get("Achievements") {
        for (name, value) in section {
            let parts: Vec<&str> = value.split('@').collect();
            if parts.first() == Some(&"1") {
                let t = parts.last().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                out.insert(name.clone(), (true, t));
            }
        }
    }
    out
}

fn process_rld(parsed: &HashMap<String, HashMap<String, String>>) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    for (name, fields) in parsed {
        if name == "Steam" {
            continue;
        }
        if let Some(state_hex) = fields.get("State") {
            if hex_to_u32_le(state_hex) == Some(1) {
                let t = fields.get("Time").and_then(|s| hex_to_u32_le(s)).unwrap_or(0) as u64;
                out.insert(name.clone(), (true, t));
            }
        }
    }
    out
}

fn process_razor1911(content: &str) -> HashMap<String, (bool, u64)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut out = HashMap::new();
    for line in content.split(['\r', '\n']) {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(' ').collect();
        if parts.len() >= 3 && parts[1] == "1" {
            let t = parts[2].parse::<u64>().unwrap_or(0);
            out.insert(parts[0].to_string(), (true, t));
        }
    }
    out
}

/// UserStats' value format is a quoted, comma-separated pseudo-record:
/// `AchievementName="unlocked = true, time = 1234567890"`. Only entries
/// whose value actually matches the "unlocked = true" prefix parse to a
/// number — a locked achievement's differently-shaped value intentionally
/// fails to parse and is skipped, mirroring the reference's NaN-filter.
fn process_user_stats(parsed: &HashMap<String, HashMap<String, String>>) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    if let Some(section) = parsed.get("ACHIEVEMENTS") {
        for (name, value) in section {
            let clean_name = name.trim_matches('"').to_string();
            let inner = value.trim_matches('"');
            if let Some(rest) = inner.strip_prefix("unlocked = true, time = ") {
                if let Ok(t) = rest.trim().parse::<u64>() {
                    out.insert(clean_name, (true, t));
                }
            }
        }
    }
    out
}

/// 3DM stores two INI sections keyed by achievement name: "State" (a fixed
/// hex pattern, "0101" means unlocked) and "Time" (hex-encoded little-endian
/// u32 unix timestamp) — a different shape from RLD despite both using hex.
fn process_3dm(parsed: &HashMap<String, HashMap<String, String>>) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    if let Some(states) = parsed.get("State") {
        let times = parsed.get("Time");
        for (name, state) in states {
            if state == "0101" {
                let t = times.and_then(|t| t.get(name)).and_then(|s| hex_to_u32_le(s)).unwrap_or(0) as u64;
                out.insert(name.clone(), (true, t));
            }
        }
    }
    out
}

/// Steam's own achievements-panel cache: a JSON array of `[key, value]`
/// pairs (like a serialized Map) — find the "achievements" entry, then read
/// `data.vecHighlight`, an array of `{bAchieved, strID, rtUnlocked}`.
fn process_steam_cache(json: &serde_json::Value) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    let Some(arr) = json.as_array() else { return out };
    for entry in arr {
        if entry.get(0).and_then(|v| v.as_str()) != Some("achievements") {
            continue;
        }
        let Some(highlights) = entry
            .get(1)
            .and_then(|v| v.get("data"))
            .and_then(|v| v.get("vecHighlight"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        for a in highlights {
            if a.get("bAchieved").and_then(|v| v.as_bool()) == Some(true) {
                if let Some(id) = a.get("strID").and_then(|v| v.as_str()) {
                    let t = a.get("rtUnlocked").and_then(|v| v.as_u64()).unwrap_or(0);
                    out.insert(id.to_string(), (true, t));
                }
            }
        }
    }
    out
}

/// Shared by Empress (same JSON shape as Goldberg/GSE).
fn process_goldberg_style_json(json: &serde_json::Value) -> HashMap<String, (bool, u64)> {
    let mut out = HashMap::new();
    if let Some(arr) = json.as_array() {
        for item in arr {
            if item["earned"].as_bool() == Some(true) {
                if let Some(name) = item["name"].as_str() {
                    out.insert(name.to_string(), (true, item["earned_time"].as_u64().unwrap_or(0)));
                }
            }
        }
    } else if let Some(obj) = json.as_object() {
        for (name, item) in obj {
            if item["earned"].as_bool() == Some(true) {
                out.insert(name.clone(), (true, item["earned_time"].as_u64().unwrap_or(0)));
            }
        }
    }
    out
}

/// Scans every supported cracker's known file locations for this AppID and
/// merges whatever unlocked achievements it finds. Best-effort throughout —
/// a missing/unreadable/malformed file for one cracker just means it
/// contributes nothing, never an error.
///
/// Merge order matters: Steam's own local cache goes first since it's the
/// least trustworthy (can go stale) — every other source that finds the same
/// achievement name overwrites it. Everything else is equally "real" local

/// Human name for a cracker, for messages shown to the player.
fn cracker_label(cracker: Cracker) -> &'static str {
    match cracker {
        Cracker::Codex => "CODEX",
        Cracker::Rune => "RUNE",
        Cracker::OnlineFix => "OnlineFix",
        Cracker::Rld => "RLD!",
        Cracker::CreamApi => "CreamAPI",
        Cracker::Skidrow => "SKIDROW",
        Cracker::Empress => "EMPRESS",
        Cracker::Razor1911 => "Razor1911",
        Cracker::UserStats => "UserStats",
        Cracker::ThreeDm => "3DM",
    }
}

/// Where a game's achievements actually live, and in whose format.
///
/// Reading merges every cracker's file because a machine may have leftovers
/// from several. Writing cannot do that: an achievement written in Goldberg's
/// JSON is invisible to a CODEX game, which reads an INI somewhere else
/// entirely. So the store has to be identified first, and the only reliable
/// evidence is which file the game itself already created.
pub fn detect_achievement_store(app_id: &str) -> Option<(Cracker, PathBuf)> {
    const ORDER: [Cracker; 8] = [
        Cracker::OnlineFix,
        Cracker::Codex,
        Cracker::Rune,
        Cracker::Rld,
        Cracker::CreamApi,
        Cracker::Skidrow,
        Cracker::Empress,
        Cracker::Razor1911,
    ];
    for cracker in ORDER {
        for path in candidate_paths(cracker, app_id) {
            if path.is_file() {
                return Some((cracker, path));
            }
        }
    }
    None
}

/// Sets or clears one achievement in the game's own cracker store.
///
/// Returns the name of the store that was written, so the caller can tell the
/// user which one it was rather than claiming a generic success.
///
/// The existing file is read, modified and rewritten rather than regenerated:
/// it holds every other achievement the player has actually earned, and
/// rebuilding it from scratch would throw that away.
pub fn write_cracker_achievement(
    app_id: &str,
    achievement_name: &str,
    earned: bool,
    earned_time: u64,
) -> Result<Option<String>, String> {
    let Some((cracker, path)) = detect_achievement_store(app_id) else {
        return Ok(None);
    };

    let content = std::fs::read_to_string(&path).unwrap_or_default();

    let updated = match cracker {
        Cracker::Codex | Cracker::Rune => {
            write_ini_entry(&content, achievement_name, &[
                ("Achieved", if earned { "1" } else { "0" }),
                ("UnlockTime", &earned_time.to_string()),
            ])
        }
        // The legacy pair is dropped, not just left alone: the reader falls
        // through to it, so a stale `Achieved = true` would override the
        // `achieved=false` written here and the clear would do nothing.
        Cracker::OnlineFix => write_ini_entry_with_aliases(
            &content,
            achievement_name,
            &[
                ("achieved", if earned { "true" } else { "false" }),
                ("timestamp", &earned_time.to_string()),
            ],
            &["Achieved", "TimeUnlocked"],
        ),
        Cracker::CreamApi => write_ini_entry(&content, achievement_name, &[
            ("achieved", if earned { "true" } else { "false" }),
            ("unlocktime", &earned_time.to_string()),
        ]),
        Cracker::Rld => write_ini_entry(&content, achievement_name, &[
            ("State", &u32_to_hex_le(if earned { 1 } else { 0 })),
            ("Time", &u32_to_hex_le(earned_time as u32)),
        ]),
        Cracker::Skidrow => write_skidrow_entry(&content, achievement_name, earned, earned_time),
        Cracker::Empress => write_goldberg_json(&content, achievement_name, earned, earned_time)?,
        Cracker::Razor1911 => write_razor_entry(&content, achievement_name, earned, earned_time),
        Cracker::UserStats | Cracker::ThreeDm => return Ok(None),
    };

    // These files are the player's actual progress. A bad edit is not
    // recoverable from anywhere else, so the original is kept beside it.
    if path.is_file() {
        let backup = path.with_extension("ragnarok-bak");
        let _ = std::fs::copy(&path, &backup);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, updated)
        .map_err(|e| format!("No se pudo escribir {}: {}", path.display(), e))?;

    Ok(Some(cracker_label(cracker).to_string()))
}

/// Rewrites one `[section]` of an INI, leaving every other line byte for byte
/// as it was — comments and ordering included.
fn write_ini_entry(content: &str, section: &str, fields: &[(&str, &str)]) -> String {
    write_ini_entry_with_aliases(content, section, fields, &[])
}

/// Same, plus key spellings that must be removed even though nothing is
/// written in their place.
///
/// OnlineFix has shipped two formats over the years — `achieved`/`timestamp`
/// in current builds, `Achieved`/`TimeUnlocked` in older ones — and both are
/// live on real machines. Writing only the modern pair left the legacy one
/// sitting beside it, and since the reader falls through to it, clearing an
/// achievement silently did nothing: the new key said false, the old one
/// still said true, and the old one won.
fn write_ini_entry_with_aliases(
    content: &str,
    section: &str,
    fields: &[(&str, &str)],
    stale_keys: &[&str],
) -> String {
    let header = format!("[{}]", section);
    let mut out: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut written = false;

    for raw in content.split('\n') {
        let line = raw.trim_end_matches('\n');
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // Leaving the target section: emit the new values before moving on.
            if in_target && !written {
                for (k, v) in fields {
                    out.push(format!("{}={}", k, v));
                }
                written = true;
            }
            in_target = trimmed.eq_ignore_ascii_case(&header);
            out.push(line.to_string());
            continue;
        }

        // Inside the target section the old keys are dropped; everything the
        // cracker put there that isn't one of ours is preserved.
        if in_target {
            if let Some(eq) = trimmed.find('=') {
                let key = trimmed[..eq].trim();
                if fields.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
                    || stale_keys.iter().any(|k| k.eq_ignore_ascii_case(key))
                {
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }

    if in_target && !written {
        for (k, v) in fields {
            out.push(format!("{}={}", k, v));
        }
        written = true;
    }

    if !written {
        if !out.is_empty() && !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push(header);
        for (k, v) in fields {
            out.push(format!("{}={}", k, v));
        }
    }

    let mut text = out.join("
");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// RLD stores its numbers as little-endian hex, which is what `hex_to_u32_le`
/// reads back.
fn u32_to_hex_le(value: u32) -> String {
    value
        .to_le_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// SKIDROW keeps everything in one `[Achievements]` section, one line per
/// achievement, with `@`-separated fields whose first is the state and whose
/// last is the timestamp.
fn write_skidrow_entry(content: &str, name: &str, earned: bool, time: u64) -> String {
    let value = format!("{}@{}", if earned { 1 } else { 0 }, time);
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut written = false;

    for raw in content.split('\n') {
        let line = raw.trim_end_matches('\n');
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_section && !written {
                out.push(format!("{}={}", name, value));
                written = true;
            }
            in_section = trimmed.eq_ignore_ascii_case("[Achievements]");
            out.push(line.to_string());
            continue;
        }

        if in_section {
            if let Some(eq) = trimmed.find('=') {
                if trimmed[..eq].trim().eq_ignore_ascii_case(name) {
                    out.push(format!("{}={}", name, value));
                    written = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }

    if !written {
        if !in_section {
            out.push("[Achievements]".to_string());
        }
        out.push(format!("{}={}", name, value));
    }

    let mut text = out.join("
");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Razor1911's file is plain lines: `<name> <state> <time>`.
fn write_razor_entry(content: &str, name: &str, earned: bool, time: u64) -> String {
    let entry = format!("{} {} {}", name, if earned { 1 } else { 0 }, time);
    let mut out: Vec<String> = Vec::new();
    let mut written = false;

    for raw in content.split('\n') {
        let line = raw.trim_end_matches('\n');
        if line.split(' ').next() == Some(name) {
            if !written {
                out.push(entry.clone());
                written = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !written {
        out.push(entry);
    }

    let mut text: Vec<String> = out.into_iter().filter(|l| !l.trim().is_empty()).collect();
    text.push(String::new());
    text.join("
")
}

/// Goldberg/GSE-style JSON, the shape `process_goldberg_style_json` reads.
fn write_goldberg_json(
    content: &str,
    name: &str,
    earned: bool,
    time: u64,
) -> Result<String, String> {
    let mut root: serde_json::Value = serde_json::from_str(content)
        .unwrap_or_else(|_| serde_json::json!({}));
    root[name] = serde_json::json!({ "earned": earned, "earned_time": time });
    serde_json::to_string_pretty(&root).map_err(|e| e.to_string())
}

/// emulator state, so order between them doesn't matter.
pub fn load_all_cracker_achievements(steam_path: &str, app_id: &str) -> HashMap<String, (bool, u64)> {
    let mut merged = HashMap::new();

    for path in steam_cache_paths(steam_path, app_id) {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                merged.extend(process_steam_cache(&json));
            }
        }
    }

    let crackers = [
        Cracker::Codex,
        Cracker::Rune,
        Cracker::OnlineFix,
        Cracker::Rld,
        Cracker::CreamApi,
        Cracker::Skidrow,
        Cracker::Empress,
        Cracker::Razor1911,
    ];

    for cracker in crackers {
        for path in candidate_paths(cracker, app_id) {
            if !path.exists() {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let result = match cracker {
                Cracker::Codex | Cracker::Rune => process_default(&ini_parse(&content)),
                Cracker::OnlineFix => process_online_fix(&ini_parse(&content)),
                Cracker::Rld => process_rld(&ini_parse(&content)),
                Cracker::CreamApi => process_cream_api(&ini_parse(&content)),
                Cracker::Skidrow => process_skidrow(&ini_parse(&content)),
                Cracker::Empress => match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(json) => process_goldberg_style_json(&json),
                    Err(_) => continue,
                },
                Cracker::Razor1911 => process_razor1911(&content),
                Cracker::UserStats | Cracker::ThreeDm => continue,
            };
            merged.extend(result);
        }
    }

    for (cracker, path) in exe_relative_paths(steam_path, app_id) {
        if !path.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let result = match cracker {
            Cracker::UserStats => process_user_stats(&ini_parse(&content)),
            Cracker::ThreeDm => process_3dm(&ini_parse(&content)),
            _ => continue,
        };
        merged.extend(result);
    }

    merged
}

#[cfg(test)]
mod achievement_writer_tests {
    use super::*;

    /// The point of every test here: write with the new writer, then read the
    /// result back with the parser that already existed and that the rest of
    /// the app trusts. If the round trip holds, the format is right — which
    /// beats me reading a format by eye and hoping.

    #[test]
    fn codex_round_trips() {
        let out = write_ini_entry("", "ACH_WIN", &[("Achieved", "1"), ("UnlockTime", "1700000000")]);
        let back = process_default(&ini_parse(&out));
        assert_eq!(back.get("ACH_WIN"), Some(&(true, 1_700_000_000)));
    }

    #[test]
    fn online_fix_round_trips() {
        let out = write_ini_entry("", "ACH_A", &[("achieved", "true"), ("timestamp", "1700000000")]);
        let back = process_online_fix(&ini_parse(&out));
        assert_eq!(back.get("ACH_A"), Some(&(true, 1_700_000_000)));
    }

    #[test]
    fn cream_api_round_trips() {
        let out = write_ini_entry("", "ACH_B", &[("achieved", "true"), ("unlocktime", "1700000000")]);
        let back = process_cream_api(&ini_parse(&out));
        assert_eq!(back.get("ACH_B").map(|v| v.0), Some(true));
    }

    /// RLD stores numbers as little-endian hex, so this also checks the hex
    /// helper against the reader that consumes it.
    #[test]
    fn rld_round_trips() {
        let out = write_ini_entry(
            "",
            "ACH_C",
            &[("State", &u32_to_hex_le(1)), ("Time", &u32_to_hex_le(1_700_000_000))],
        );
        let back = process_rld(&ini_parse(&out));
        assert_eq!(back.get("ACH_C"), Some(&(true, 1_700_000_000)));
    }

    #[test]
    fn skidrow_round_trips() {
        let out = write_skidrow_entry("", "ACH_D", true, 1_700_000_000);
        let back = process_skidrow(&ini_parse(&out));
        assert_eq!(back.get("ACH_D"), Some(&(true, 1_700_000_000)));
    }

    #[test]
    fn razor_round_trips() {
        let out = write_razor_entry("", "ACH_E", true, 1_700_000_000);
        let back = process_razor1911(&out);
        assert_eq!(back.get("ACH_E"), Some(&(true, 1_700_000_000)));
    }

    /// The file holds progress the player actually earned. Setting one
    /// achievement must not disturb the others, and must not drop whatever
    /// else the cracker wrote in that section.
    #[test]
    fn preserves_other_achievements_and_unknown_keys() {
        let existing = "[ACH_ONE]\nAchieved=1\nUnlockTime=111\n\n[ACH_TWO]\nAchieved=1\nUnlockTime=222\nCustomField=keepme\n";
        let out = write_ini_entry(existing, "ACH_TWO", &[("Achieved", "1"), ("UnlockTime", "999")]);
        let back = process_default(&ini_parse(&out));

        assert_eq!(back.get("ACH_ONE"), Some(&(true, 111)), "borro otro logro:\n{}", out);
        assert_eq!(back.get("ACH_TWO"), Some(&(true, 999)), "no actualizo:\n{}", out);
        assert!(out.contains("CustomField=keepme"), "perdio un campo ajeno:\n{}", out);
    }

    /// Clearing has to actually clear: writing Achieved=0 must make the
    /// reader stop reporting it as earned.
    #[test]
    fn clearing_removes_it_from_the_reader() {
        let existing = "[ACH_ONE]\nAchieved=1\nUnlockTime=111\n";
        let out = write_ini_entry(existing, "ACH_ONE", &[("Achieved", "0"), ("UnlockTime", "0")]);
        let back = process_default(&ini_parse(&out));
        assert!(back.get("ACH_ONE").is_none(), "sigue marcado como conseguido:\n{}", out);
    }

    /// Writing the same achievement twice must not stack duplicate sections —
    /// the install flow can run this repeatedly.

    /// Real machines carry both OnlineFix formats. Clearing an achievement
    /// written in the old one has to actually clear it — leaving the legacy
    /// keys behind made the reader keep reporting it as earned.
    #[test]
    fn clearing_beats_the_legacy_online_fix_format() {
        let legacy = "[ACH_OLD]\nAchieved = true\nTimeUnlocked = 1777183\n";
        // Sanity: the reader really does consider it earned to begin with.
        assert!(process_online_fix(&ini_parse(legacy)).contains_key("ACH_OLD"));

        let out = write_ini_entry_with_aliases(
            legacy,
            "ACH_OLD",
            &[("achieved", "false"), ("timestamp", "0")],
            &["Achieved", "TimeUnlocked"],
        );
        let back = process_online_fix(&ini_parse(&out));
        assert!(
            back.get("ACH_OLD").is_none(),
            "sigue conseguido tras quitarlo:\n{}",
            out
        );
        assert!(!out.contains("TimeUnlocked"), "quedo la clave antigua:\n{}", out);
    }

    #[test]
    fn writing_twice_does_not_duplicate() {
        let once = write_ini_entry("", "ACH_X", &[("Achieved", "1"), ("UnlockTime", "5")]);
        let twice = write_ini_entry(&once, "ACH_X", &[("Achieved", "1"), ("UnlockTime", "7")]);
        assert_eq!(twice.matches("[ACH_X]").count(), 1, "seccion duplicada:\n{}", twice);
        let back = process_default(&ini_parse(&twice));
        assert_eq!(back.get("ACH_X"), Some(&(true, 7)));
    }
}
