//! Reads the save-file locations Steam already knows about.
//!
//! Every game's entry in `appcache/appinfo.vdf` carries a `ufs/savefiles`
//! list: a root token (`WinAppDataLocal`, `WinMyDocuments`, …), a path under
//! it, and a filename pattern. That is the definition Steam Cloud itself syncs
//! from, so it is authoritative — and it is the piece the backup was missing.
//!
//! Before this, backups looked in two places only: Steam Cloud's own
//! `userdata/<id>/<appid>/remote`, and the Goldberg emulator's folder. Games
//! that save anywhere else were backed up as an achievements file and nothing
//! more. Crimson Desert is the example that exposed it — 8.9 MB of saves under
//! `%LOCALAPPDATA%\Pearl Abyss`, a folder named after the studio, which no
//! search by game title would ever have found. Steam had the answer on disk
//! the whole time:
//!
//! ```text
//! root=WinAppDataLocal  path=Pearl Abyss/CD/save/{Steam3AccountID}  pattern=*.save
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One declared save location.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveDefinition {
    pub root: String,
    pub path: String,
    pub pattern: String,
}

/// A parsed value from appinfo's binary VDF. Only what is needed to walk to
/// `ufs/savefiles` is kept; numbers are discarded.
#[derive(Clone)]
enum Node {
    Map(HashMap<String, Node>),
    Str(String),
    Other,
}

impl Node {
    fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(m) => m.get(key),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Node::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
    fn children(&self) -> Vec<(&String, &Node)> {
        match self {
            Node::Map(m) => m.iter().collect(),
            _ => Vec::new(),
        }
    }
}

fn read_u32(data: &[u8], pos: usize) -> Option<u32> {
    data.get(pos..pos + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_i64(data: &[u8], pos: usize) -> Option<i64> {
    data.get(pos..pos + 8).map(|b| {
        i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    })
}

fn read_cstr(data: &[u8], pos: usize) -> Option<(String, usize)> {
    let end = data[pos..].iter().position(|&c| c == 0)? + pos;
    let s = String::from_utf8_lossy(&data[pos..end]).into_owned();
    Some((s, end + 1))
}

/// One level of appinfo's binary VDF.
///
/// In format 29 the keys are indices into a string table at the end of the
/// file rather than inline strings, which is why the table has to be read
/// first.
fn read_kv(
    data: &[u8],
    mut pos: usize,
    table: Option<&[String]>,
    depth: u32,
) -> Option<(Node, usize)> {
    // Guards against a malformed file walking us off into the weeds.
    if depth > 32 {
        return None;
    }
    let mut map = HashMap::new();
    loop {
        let t = *data.get(pos)?;
        pos += 1;
        if t == 0x08 {
            return Some((Node::Map(map), pos));
        }
        let key = match table {
            Some(t) => {
                let idx = read_u32(data, pos)? as usize;
                pos += 4;
                t.get(idx)?.clone()
            }
            None => {
                let (s, next) = read_cstr(data, pos)?;
                pos = next;
                s
            }
        };
        match t {
            0x00 => {
                let (child, next) = read_kv(data, pos, table, depth + 1)?;
                pos = next;
                map.insert(key, child);
            }
            0x01 => {
                let (s, next) = read_cstr(data, pos)?;
                pos = next;
                map.insert(key, Node::Str(s));
            }
            0x02 => {
                pos += 4;
                map.insert(key, Node::Other);
            }
            0x07 => {
                pos += 8;
                map.insert(key, Node::Other);
            }
            _ => return None,
        }
    }
}

/// The `ufs/savefiles` entries Steam has recorded for this app.
///
/// Returns an empty list rather than an error whenever anything is missing or
/// unreadable: plenty of games declare no save files at all (Assassin's Creed
/// Black Flag Resynced is one), and a backup should carry on with whatever
/// other sources it has instead of failing.
/// The parsed appinfo tree for one app, or None if anything at all is off.
///
/// Split out of save_definitions once a second caller needed it: walking the
/// file, decoding the string table and skipping the per-app header is the same
/// work whichever section you are after, and having two copies of that offset
/// arithmetic is how one of them quietly drifts.
fn app_tree(steam_path: &str, app_id: &str) -> Option<Node> {
    let target = app_id.parse::<u32>().ok()?;
    let file = Path::new(steam_path).join("appcache").join("appinfo.vdf");
    let data = std::fs::read(&file).ok()?;

    let magic = read_u32(&data, 0)?;
    let mut pos = 8usize;

    // Format 29 and later put a string table at the end and reference it by
    // index; earlier formats spell every key out inline.
    let mut table: Option<Vec<String>> = None;
    // Valve increments the last byte in hex, so v29's successor is 0x0756442A
    // — not 0x07564430, which was written as if the digits were decimal and
    // skips six versions. When Valve does ship the next format this would stop
    // matching, the string table would go unread, and every key would be
    // decoded as an inline C-string over what are really binary indices. The
    // result is a silent empty list, and `save_definitions` documents an empty
    // list as normal — so backups would quietly go back to missing exactly the
    // saves this module was written to find.
    if (0x07564427..=0x0756442F).contains(&magic) {
        let off = read_i64(&data, pos)?;
        pos += 8;
        let mut p = usize::try_from(off).ok()?;
        let count = read_u32(&data, p)?;
        p += 4;
        // `count` comes straight out of the file, and a corrupt one can say
        // four billion. `with_capacity` would then ask the allocator for
        // ~100 GB in a single call, and an allocation failure is an abort, not
        // a catchable panic — the launcher would simply vanish while backing
        // up someone's saves. Each string needs at least a NUL byte, so the
        // remaining bytes are a hard ceiling on how many can exist.
        let remaining = data.len().saturating_sub(p);
        if count as usize > remaining {
            return None;
        }
        let mut strings = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let (str_value, next) = read_cstr(&data, p)?;
            strings.push(str_value);
            p = next;
        }
        table = Some(strings);
    }

    while pos + 8 <= data.len() {
        let Some(id) = read_u32(&data, pos) else { break };
        if id == 0 {
            break;
        }
        let Some(size) = read_u32(&data, pos + 4) else { break };
        let body = pos + 8;
        let next_app = body.saturating_add(size as usize);

        if id == target {
            // infoState, lastUpdated, picsToken, sha1, changeNumber, binary sha1
            let start = body + 4 + 4 + 8 + 20 + 4 + 20;
            let (tree, _) = read_kv(&data, start, table.as_deref(), 0)?;
            // The app's data sits under "appinfo" in some dumps and at the
            // root in others.
            return Some(match tree.get("appinfo") {
                Some(inner) => inner.clone(),
                None => tree,
            });
        }

        if next_app <= pos {
            break; // never move backwards on a corrupt size
        }
        pos = next_app;
    }

    None
}

pub fn save_definitions(steam_path: &str, app_id: &str) -> Vec<SaveDefinition> {
    let Some(root) = app_tree(steam_path, app_id) else { return Vec::new() };
    let Some(saves) = root.get("ufs").and_then(|u| u.get("savefiles")) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (_, entry) in saves.children() {
        let root_token = entry.get("root").and_then(|v| v.as_str()).unwrap_or("");
        if root_token.is_empty() {
            continue;
        }
        out.push(SaveDefinition {
            root: root_token.to_string(),
            path: entry.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            pattern: entry
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("*")
                .to_string(),
        });
    }
    out
}

/// The DLC app ids Steam itself records for a game, read from the local
/// appinfo.vdf.
///
/// The store API is the usual source, but it answers `success: false` for an
/// app that is unreleased or not sold in the user's region — Forza Horizon 6
/// (2483190) is one — and the DLC Manager then had nothing to show but an
/// error. Steam's own cache knows the DLC list for those apps anyway.
///
/// `extended/listofdlc` is a comma-separated string; each depot may also name
/// the DLC it belongs to. Numbers are not kept by this parser, so only the
/// string forms are read — which is how Valve writes both of these.
pub fn dlc_ids(steam_path: &str, app_id: &str) -> Vec<String> {
    let Some(root) = app_tree(steam_path, app_id) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        let id = id.trim();
        // `!= app_id` because a depot's dlcappid is the base game for its own
        // depots, and listing the game as its own DLC would offer to unlock it
        // twice.
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && id != app_id && !out.iter().any(|x| x == id) {
            out.push(id.to_string());
        }
    };

    if let Some(list) = root.get("extended").and_then(|e| e.get("listofdlc")).and_then(|v| v.as_str()) {
        for part in list.split(',') {
            push(part);
        }
    }
    if let Some(depots) = root.get("depots") {
        for (_, depot) in depots.children() {
            if let Some(id) = depot.get("dlcappid").and_then(|v| v.as_str()) {
                push(id);
            }
        }
    }
    out
}

/// Turns a root token into a real directory on this machine.
fn resolve_root(root: &str, game_dir: Option<&Path>) -> Option<PathBuf> {
    let home = dirs::home_dir();
    match root {
        "WinAppDataLocal" => dirs::data_local_dir(),
        "WinAppDataLocalLow" => dirs::data_local_dir().map(|p| {
            p.parent().map(|q| q.join("LocalLow")).unwrap_or(p)
        }),
        "WinAppDataRoaming" => dirs::data_dir(),
        "WinMyDocuments" => dirs::document_dir(),
        "WinSavedGames" => home.map(|h| h.join("Saved Games")),
        "gameinstall" => game_dir.map(|p| p.to_path_buf()),
        _ => None,
    }
}

/// True if this folder holds any file at all, a few levels down.
pub(crate) fn has_any_file(dir: &Path, depth: u32) -> bool {
    if depth > 4 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            return true;
        }
        if path.is_dir() && has_any_file(&path, depth + 1) {
            return true;
        }
    }
    false
}

/// Every declared save folder that actually holds something.
///
/// `{Steam3AccountID}` is the account id — the lower 32 bits of the SteamID64,
/// the same number Steam names its `userdata` folders after, so it is read
/// from there rather than asked for.
///
/// The parent of that folder is tried as well, and this is the part that
/// matters for the games this launcher deals with: they run under an emulated
/// Steam, so the account id the game writes under is the emulator's, not the
/// real one. Crimson Desert is the case in hand — Steam declares
/// `CD/save/{Steam3AccountID}`, which resolves to a folder holding one 53-byte
/// file, while the actual 8.9 MB of saves sit beside it under
/// `CD/save/19627`, `CD/save/764415490` and so on. Backing up the parent
/// catches every profile, which is what someone restoring wants anyway: they
/// do not know which of those numbers is theirs.
pub fn existing_save_dirs(steam_path: &str, app_id: &str, game_dir: Option<&Path>) -> Vec<PathBuf> {
    // Only folders with something in them, so an empty declared path never
    // shadows the one carrying the saves.
    let mut out: Vec<PathBuf> = declared_save_dirs(steam_path, app_id, game_dir)
        .into_iter()
        .map(|d| d.path)
        .filter(|c| c.is_dir() && has_any_file(c, 0))
        .collect();

    // The roomiest first: with both a profile folder and its parent present,
    // the parent is the one holding every save.
    out.sort_by_key(|p| std::cmp::Reverse(walk_size(p, 0)));
    out
}

/// One place Steam says a game's saves may live, whether or not it exists yet.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredSaveDir {
    pub root: String,
    /// The path exactly as appinfo.vdf declares it, placeholder included, so
    /// it can be resolved again on another PC under another account id.
    pub template: String,
    /// The folder above `{Steam3AccountID}`, holding every profile.
    pub holds_profiles: bool,
    pub path: PathBuf,
}

/// The folders one definition can resolve to under `base`: each account's
/// own folder, then the one holding every profile.
fn candidates_for(def: &SaveDefinition, base: &Path, account_ids: &[String]) -> Vec<(PathBuf, bool)> {
    let rel = def.path.replace('/', "\\");
    let mut out = Vec::new();
    if rel.contains("{Steam3AccountID}") {
        for id in account_ids {
            out.push((base.join(rel.replace("{Steam3AccountID}", id).trim_matches('\\')), false));
        }
        // Drop the placeholder segment entirely: the folder that holds every
        // profile, whatever id each one was written under.
        let parent_rel = rel
            .replace("{Steam3AccountID}", "")
            .trim_end_matches('\\')
            .to_string();
        if !parent_rel.is_empty() {
            out.push((base.join(parent_rel.trim_matches('\\')), true));
        }
    } else {
        out.push((base.join(rel.trim_matches('\\')), false));
    }
    out
}

/// Every save folder the game declares, **without** requiring it to exist.
///
/// This is the half restore was missing. Backup only ever needs folders that
/// already hold saves, but a restore after formatting — the case it exists for
/// — is precisely when the game's folder has not been created yet, and looking
/// only at existing folders sent those saves somewhere the game never reads.
pub fn declared_save_dirs(steam_path: &str, app_id: &str, game_dir: Option<&Path>) -> Vec<DeclaredSaveDir> {
    let account_ids = steam_account_ids(steam_path);
    let mut out: Vec<DeclaredSaveDir> = Vec::new();
    for def in save_definitions(steam_path, app_id) {
        let Some(base) = resolve_root(&def.root, game_dir) else { continue };
        for (path, holds_profiles) in candidates_for(&def, &base, &account_ids) {
            if !out.iter().any(|d| d.path == path) {
                out.push(DeclaredSaveDir {
                    root: def.root.clone(),
                    template: def.path.clone(),
                    holds_profiles,
                    path,
                });
            }
        }
    }
    out
}

/// A recorded origin, resolved on this machine. Works without appinfo.vdf
/// knowing the game, and without the folder existing.
pub fn resolve_template(
    steam_path: &str,
    root: &str,
    template: &str,
    holds_profiles: bool,
    game_dir: Option<&Path>,
) -> Option<PathBuf> {
    let base = resolve_root(root, game_dir)?;
    let def = SaveDefinition {
        root: root.to_string(),
        path: template.to_string(),
        pattern: "*".to_string(),
    };
    let candidates = candidates_for(&def, &base, &steam_account_ids(steam_path));
    candidates
        .iter()
        .find(|(_, holds)| *holds == holds_profiles)
        .or_else(|| candidates.first())
        .map(|(path, _)| path.clone())
}

/// Bytes held under a folder, bounded so a huge tree cannot stall a backup.
fn walk_size(dir: &Path, depth: u32) -> u64 {
    if depth > 4 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        } else if path.is_dir() {
            total += walk_size(&path, depth + 1);
        }
    }
    total
}

/// The account ids Steam has folders for under `userdata`.
fn steam_account_ids(steam_path: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let userdata = Path::new(steam_path).join("userdata");
    if let Ok(entries) = std::fs::read_dir(&userdata) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.chars().all(|c| c.is_ascii_digit()) && name != "0" {
                ids.push(name);
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEAM: &str = r"C:\Program Files (x86)\Steam";

    fn appinfo_present() -> bool {
        Path::new(STEAM).join("appcache").join("appinfo.vdf").is_file()
    }

    /// Reads the real file this machine has. Skipped where Steam is not
    /// installed, so the suite still passes on a build server.
    #[test]
    fn finds_a_declared_save_path() {
        if !appinfo_present() {
            return;
        }
        // Crimson Desert declares WinAppDataLocal + Pearl Abyss/CD/save/...
        let defs = save_definitions(STEAM, "3321460");
        if defs.is_empty() {
            return; // this machine may not have that app in its cache
        }
        assert!(
            defs.iter().any(|d| d.root == "WinAppDataLocal" && d.path.contains("Pearl Abyss")),
            "esperaba la ruta de Pearl Abyss, salio {:?}",
            defs
        );
    }

    /// A game that declares nothing must come back empty, not error.
    #[test]
    fn an_app_without_savefiles_is_empty_not_an_error() {
        if !appinfo_present() {
            return;
        }
        assert!(save_definitions(STEAM, "3751950").is_empty());
    }

    #[test]
    fn an_unknown_app_is_empty() {
        if !appinfo_present() {
            return;
        }
        assert!(save_definitions(STEAM, "999999999").is_empty());
    }

    #[test]
    fn a_missing_steam_folder_is_empty() {
        assert!(save_definitions(r"Z:\no\existe", "3321460").is_empty());
    }

    /// A restore target has to be computable before the folder exists — the
    /// whole point after formatting a PC.
    #[test]
    fn declared_candidates_do_not_depend_on_the_folder_existing() {
        let base = Path::new(r"Z:\no\existe");
        let profiles = SaveDefinition {
            root: "WinAppDataLocal".into(),
            path: "Pearl Abyss/CD/save/{Steam3AccountID}".into(),
            pattern: "*".into(),
        };
        assert_eq!(
            candidates_for(&profiles, base, &["19627".to_string()]),
            vec![
                (base.join(r"Pearl Abyss\CD\save\19627"), false),
                (base.join(r"Pearl Abyss\CD\save"), true),
            ]
        );
        // No Steam account signed in yet: only the folder holding every profile.
        assert_eq!(
            candidates_for(&profiles, base, &[]),
            vec![(base.join(r"Pearl Abyss\CD\save"), true)]
        );

        let plain = SaveDefinition {
            root: "WinMyDocuments".into(),
            path: "My Games/Juego".into(),
            pattern: "*".into(),
        };
        assert_eq!(candidates_for(&plain, base, &[]), vec![(base.join(r"My Games\Juego"), false)]);
    }

    #[test]
    fn an_unknown_app_has_no_dlc_and_a_game_is_never_its_own_dlc() {
        assert!(dlc_ids(STEAM, "999999999").is_empty());
        if !appinfo_present() {
            return;
        }
        for app in ["730", "227300", "2483190"] {
            assert!(!dlc_ids(STEAM, app).iter().any(|id| id == app), "{app} se listó como DLC de sí mismo");
        }
    }

    #[test]
    fn a_non_numeric_app_id_is_empty() {
        assert!(save_definitions(STEAM, "no-soy-un-id").is_empty());
    }
}
