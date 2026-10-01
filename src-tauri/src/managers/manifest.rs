use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ManifestManager;

impl ManifestManager {
    /// Parse libraryfolders.vdf to get all Steam library paths.
    pub fn get_library_folders(steam_path: &str) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        Self::push_library(&mut paths, steam_path.to_string());

        let vdf = PathBuf::from(steam_path)
            .join("steamapps")
            .join("libraryfolders.vdf");
        if let Ok(content) = fs::read_to_string(&vdf) {
            for p in Self::parse_library_folders(&content) {
                Self::push_library(&mut paths, p);
            }
        }
        paths
    }

    /// Adds a library unless it is already there under a different spelling.
    ///
    /// The Steam path arrives from the registry in whatever case Windows
    /// stored it — often all lowercase — while libraryfolders.vdf spells the
    /// same folder properly. Windows treats both as one path, so a
    /// case-sensitive comparison listed the Steam folder twice in the install
    /// drive picker: `c:\program files (x86)\steam` and
    /// `C:\Program Files (x86)\Steam`, same 19 GB, same disk.
    ///
    /// The first spelling seen is the one kept, so the list still shows what
    /// the rest of the app is actually using.
    fn push_library(paths: &mut Vec<String>, candidate: String) {
        let key = |s: &str| s.trim_end_matches(['\\', '/']).to_lowercase().replace('/', "\\");
        let candidate_key = key(&candidate);
        if candidate_key.is_empty() {
            return;
        }
        if paths.iter().any(|existing| key(existing) == candidate_key) {
            return;
        }
        paths.push(candidate);
    }

    /// Library paths out of a `libraryfolders.vdf`, in either layout Steam
    /// has used.
    ///
    /// Current Steam writes a block per library with a `"path"` key. Older
    /// installs — and installs upgraded from them — instead list the extra
    /// drives as numbered keys straight to the path:
    ///
    /// ```text
    ///     "1"     "D:\\SteamLibrary"
    /// ```
    ///
    /// Only `"path"` was read before, so on a machine still carrying the
    /// numbered layout every library except the Steam folder itself was
    /// invisible — and with it every game installed on that drive. A user
    /// reported exactly that: games on a second HDD registered in Steam,
    /// not detected by the launcher.
    ///
    /// Split out from the reading so both layouts can actually be tested.
    #[cfg(test)]
    #[allow(dead_code)]
    fn push_library_for_test(paths: &mut Vec<String>, candidate: String) {
        Self::push_library(paths, candidate)
    }

    pub(crate) fn parse_library_folders(content: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            // A value line has exactly two quoted fields: key then value.
            let fields: Vec<&str> = trimmed.split('"').filter(|s| !s.trim().is_empty()).collect();
            if fields.len() != 2 {
                continue;
            }
            let key = fields[0].trim();
            let value = fields[1].trim();

            // `"path"` (current) or a bare number (legacy). Anything else in
            // this file is metadata — TimeNextStatsReport, ContentStatsID,
            // per-app sizes — and must not be mistaken for a folder.
            let is_library = key.eq_ignore_ascii_case("path")
                || (!key.is_empty() && key.chars().all(|c| c.is_ascii_digit()));
            if !is_library {
                continue;
            }

            let path = value.replace("\\\\", "\\");
            // A numbered key whose value is a number is a size or an id, not
            // a path.
            if path.is_empty() || path.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if !out.contains(&path) {
                out.push(path);
            }
        }
        out
    }

    /// Single source of truth for "does this ACF mean the app's files are
    /// really on disk?" — used by BOTH is_properly_installed and
    /// acf_has_valid_state.
    ///
    /// These two used to carry their own copy of this logic and drifted:
    /// StateFlags 1 was dropped from one and left in the other, so
    /// list_installed_ids kept reporting owned-but-never-downloaded games as
    /// installed. That's what put a MATCH badge and an enabled Auto-apply on
    /// Bypass files for a game the user had never downloaded (confirmed on
    /// "Assassin's Creed Black Flag Resynced", appid 3751950, sitting at
    /// StateFlags 1), and the apply then failed with find_game_folder's own
    /// "no se encontró una instalación completa" — the backend correctly
    /// disagreeing with the badge the UI had just shown.
    ///
    /// Also switched from exact string equality to a bit test, because the
    /// old `val == "4"` missed every legitimate combination that merely has
    /// extra bits set — confirmed on a real install sitting at 1030
    /// (0x400 UpdateRunning | 0x2 UpdateRequired | 0x4 FullyInstalled),
    /// which is genuinely installed and was being reported as not installed.
    /// Public wrapper so other modules read StateFlags through this single
    /// implementation instead of keeping their own — crackers.rs had a copy
    /// that had already drifted out of sync with this one.
    pub(crate) fn acf_reports_installed(content: &str) -> bool {
        Self::state_flags_mean_installed(content)
    }

    fn state_flags_mean_installed(content: &str) -> bool {
        // Steam's own StateFlags bits (steamclient's EAppState).
        const FULLY_INSTALLED: u32 = 0x4;
        // 1026 = 0x400 UpdateRunning | 0x2 UpdateRequired: files exist but an
        // update is mid-download, so FullyInstalled isn't set yet. Kept as an
        // explicit value rather than a bit test — matching UpdateRunning on
        // its own would also catch a brand-new download that has nothing on
        // disk yet.
        const MID_UPDATE_WITH_FILES: u32 = 1026;

        content.lines().any(|l| {
            let l = l.trim();
            if !l.contains("\"StateFlags\"") {
                return false;
            }
            // Extract the value between the last pair of quotes: "StateFlags"  "4"
            // A line like `"StateFlags"\t\t"4"` has 4 quote chars, so a
            // plain .split('"') yields 5 segments — ["", "StateFlags",
            // "\t\t", "4", ""] — and .last() was grabbing that final
            // EMPTY segment (nothing follows the closing quote), never
            // the actual "4". This silently made every ACF-based real-
            // install check return false always; only ever masked
            // because callers unioned this with Ragnarok's own apps
            // sidecar, which doesn't need a real ACF to report "installed".
            let val = l.split('"').map(str::trim).filter(|s| !s.is_empty()).last().unwrap_or("");
            let flags: u32 = match val.parse() {
                Ok(f) => f,
                Err(_) => return false,
            };
            // StateFlags 1 ("Uninstalled") deliberately does NOT qualify: an
            // app merely owned/tracked in the library gets an ACF with that
            // value long before a single byte lands on disk.
            (flags & FULLY_INSTALLED) != 0 || flags == MID_UPDATE_WITH_FILES
        })
    }

    /// Reads one top-level `"key"		"value"` out of an ACF.
    pub(crate) fn acf_value(content: &str, key: &str) -> Option<String> {
        let needle = format!("\"{}\"", key);
        content.lines().find_map(|l| {
            let l = l.trim();
            if !l.starts_with(&needle) {
                return None;
            }
            let val = l[needle.len()..].trim().trim_matches('"').trim();
            (!val.is_empty()).then(|| val.to_string())
        })
    }

    /// Whether the ACF's own claim that the game is on disk actually holds.
    ///
    /// `StateFlags 4` asserts "Fully Installed". Nothing was checking that
    /// against reality, and a stale ACF making that claim over an empty folder
    /// is what drove the worst loop in this app:
    ///
    /// 1. install_game keeps the ACF, because its "is this already installed?"
    ///    guard only ever read StateFlags.
    /// 2. Steam reads it, looks in `common/<installdir>`, finds nothing, and
    ///    reacts the only sensible way — `Files Missing` → **uninstall**.
    /// 3. That uninstall sweeps the depot manifest out of depotcache, sixteen
    ///    seconds after the install had just written it there.
    /// 4. Steam then wants to download the game, has no manifest, asks its CDN,
    ///    and gets 401 because the account does not own it — surfaced as
    ///    "UNKNOWN ERROR" / "CONTENT SERVERS UNREACHABLE".
    ///
    /// Reinstalling could never break the loop, because every reinstall walked
    /// straight back into step 1. Traced end to end in content_log.txt.
    ///
    /// Deliberately scoped to the FullyInstalled claim. A mid-update ACF (1026)
    /// legitimately has an empty `common/` at the start of a fresh download —
    /// its files are still in `downloading/` — so treating that as stale would
    /// delete the manifest of a download in progress.
    fn install_dir_has_files(lib: &str, content: &str) -> bool {
        // No installdir to check means no verdict; trust the flags rather than
        // delete an ACF on a guess.
        let Some(dir) = Self::acf_value(content, "installdir") else {
            return true;
        };
        let path = PathBuf::from(lib).join("steamapps").join("common").join(dir);
        fs::read_dir(&path)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    }

    /// True only if the ACF reports the app as actually downloaded — NOT
    /// merely owned/tracked in the library, which has no files on disk at all,
    /// and not a stale "Fully Installed" over a folder Steam already emptied.
    pub fn is_properly_installed(library_paths: &[String], app_id: &str) -> bool {
        for lib in library_paths {
            let acf = PathBuf::from(lib)
                .join("steamapps")
                .join(format!("appmanifest_{}.acf", app_id));
            if let Ok(content) = fs::read_to_string(&acf) {
                if !Self::state_flags_mean_installed(&content) {
                    continue;
                }
                // Only the unambiguous claim gets verified; see the doc above.
                let claims_fully_installed = Self::acf_value(&content, "StateFlags")
                    .and_then(|v| v.parse::<u32>().ok())
                    .map(|f| (f & 0x4) != 0)
                    .unwrap_or(false);
                if claims_fully_installed && !Self::install_dir_has_files(lib, &content) {
                    continue;
                }
                return true;
            }
        }
        false
    }

    fn acf_has_valid_state(content: &str) -> bool {
        Self::state_flags_mean_installed(content)
    }

    /// Steam auto-installs these alongside real games (VC++/DirectX
    /// redistributables, the VR runtime, etc.) — each gets its own
    /// appmanifest_<id>.acf exactly like a real game, but none of them are
    /// games and none should ever show up as a "Steam" entry in the
    /// Library.
    const NON_GAME_APP_IDS: &'static [&'static str] = &[
        "228980",  // Steamworks Common Redistributables
        "250820",  // SteamVR
        "1070560", // Steam Linux Runtime
        "1391110", // Steam Linux Runtime - Soldier
        "1493710", // Steam Linux Runtime - Sniper
    ];

    /// Scans every Steam library folder's steamapps directory ONCE and
    /// returns every app id with a valid appmanifest_<id>.acf. Use this
    /// instead of calling is_properly_installed() per app id when checking
    /// against a large set of ids (e.g. an entire catalog) — one directory
    /// listing per library beats one file-exists check per catalog entry,
    /// which used to take a very long time (tens of thousands of individual
    /// file-system lookups) on catalogs with many thousands of games.
    pub fn list_installed_ids(steam_path: &str) -> std::collections::HashSet<String> {
        let mut result = std::collections::HashSet::new();
        for lib in Self::get_library_folders(steam_path) {
            let steamapps = PathBuf::from(&lib).join("steamapps");
            let entries = match fs::read_dir(&steamapps) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();
                let id = match file_name
                    .strip_prefix("appmanifest_")
                    .and_then(|s| s.strip_suffix(".acf"))
                {
                    Some(id) => id.to_string(),
                    None => continue,
                };
                if Self::NON_GAME_APP_IDS.contains(&id.as_str()) {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    if Self::acf_has_valid_state(&content) {
                        result.insert(id);
                    }
                }
            }
        }
        result
    }

    /// Create a fake ACF with StateFlags=4.
    pub fn create_fake_manifest(
        steam_path: &str,
        app_id: &str,
        app_name: &str,
    ) -> Result<(), String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Sanitize install dir (remove special chars)
        let install_dir: String = app_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let content = format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"Universe\"\t\t\"1\"\n\t\"name\"\t\t\"{name}\"\n\t\"StateFlags\"\t\t\"1\"\n\t\"installdir\"\t\t\"{dir}\"\n\t\"LastUpdated\"\t\t\"{ts}\"\n\t\"SizeOnDisk\"\t\t\"0\"\n\t\"buildid\"\t\t\"0\"\n\t\"LastOwner\"\t\t\"0\"\n\t\"BytesToDownload\"\t\t\"0\"\n\t\"BytesDownloaded\"\t\t\"0\"\n\t\"AutoUpdateBehavior\"\t\t\"0\"\n\t\"AllowOtherDownloadsWhileRunning\"\t\t\"0\"\n\t\"ScheduledAutoUpdate\"\t\t\"0\"\n\t\"UserConfig\"\n\t{{\n\t}}\n\t\"MountedDepots\"\n\t{{\n\t}}\n}}\n",
            appid = app_id,
            name = app_name,
            dir = install_dir,
            ts = ts
        );
        let steamapps = PathBuf::from(steam_path).join("steamapps");
        fs::create_dir_all(&steamapps).map_err(|e| e.to_string())?;
        let acf = steamapps.join(format!("appmanifest_{}.acf", app_id));
        fs::write(acf, content).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Delete the ACF manifest for a game across all library paths.
    pub fn delete_manifest(library_paths: &[String], app_id: &str) -> Result<(), String> {
        let mut found = false;
        
        for lib in library_paths {
            let acf = PathBuf::from(lib)
                .join("steamapps")
                .join(format!("appmanifest_{}.acf", app_id));
            if acf.exists() {
                fs::remove_file(&acf).map_err(|e| e.to_string())?;
                found = true;
            }
        }
        
        if found {
            Ok(())
        } else {
            Err(format!("Manifest for {} not found", app_id))
        }
    }

    /// Updates the SizeOnDisk (and BytesDownloaded) field in an existing ACF
    /// with the real size in bytes. Called after game files are placed on disk.
    ///
    /// Steam reads SizeOnDisk on every library refresh — setting it to the real
    /// value prevents "0 B" from showing in the library and makes the game appear
    /// as a proper installed title.
    pub fn update_size_on_disk(steam_path: &str, app_id: &str, size_bytes: u64) -> Result<(), String> {
        let acf_path = PathBuf::from(steam_path)
            .join("steamapps")
            .join(format!("appmanifest_{}.acf", app_id));

        if !acf_path.exists() {
            return Err(format!("ACF not found for app {}", app_id));
        }

        let content = fs::read_to_string(&acf_path).map_err(|e| e.to_string())?;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Replace key fields line by line (preserves all other fields intact)
        let updated: String = content
            .lines()
            .map(|line| {
                let t = line.trim();
                if t.starts_with("\"SizeOnDisk\"") {
                    format!("\t\"SizeOnDisk\"\t\t\"{}\"", size_bytes)
                } else if t.starts_with("\"BytesDownloaded\"") {
                    format!("\t\"BytesDownloaded\"\t\t\"{}\"", size_bytes)
                } else if t.starts_with("\"BytesToDownload\"") {
                    format!("\t\"BytesToDownload\"\t\t\"{}\"", size_bytes)
                } else if t.starts_with("\"LastUpdated\"") {
                    format!("\t\"LastUpdated\"\t\t\"{}\"", ts)
                } else if t.starts_with("\"StateFlags\"") {
                    // Only force "not installed" on a manifest Steam is not
                    // already busy with.
                    //
                    // This overwrote StateFlags to 1 unconditionally, and the
                    // caller is a background poll that fires every three
                    // seconds waiting for Steam to write its own ACF — so it
                    // caught the file mid-download. Steam writes 1026/1030
                    // there while fetching (see acf_reports_installed), and
                    // stamping 1 over that told Steam its own in-progress
                    // download was uninstalled: users saw the download cancel
                    // itself. It also left the file claiming "fully
                    // downloaded" and "not installed" at the same time.
                    //
                    // A file Steam is actively working on keeps whatever it
                    // says; the goal here is only to show Install for an entry
                    // that is genuinely idle.
                    let busy = Self::acf_reports_installed(&content)
                        || t.contains("1026")
                        || t.contains("1030")
                        || t.contains("1062");
                    if busy {
                        line.to_string()
                    } else {
                        "\t\"StateFlags\"\t\t\"1\"".to_string()
                    }
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Ensure trailing newline
        let updated = if updated.ends_with('\n') { updated } else { updated + "\n" };

        fs::write(&acf_path, updated).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod library_folder_tests {
    use super::ManifestManager;

    /// The layout current Steam writes, taken from a real machine.
    #[test]
    fn reads_the_current_path_layout() {
        let vdf = r#"
"libraryfolders"
{
    "0"
    {
        "path"        "C:\\Program Files (x86)\\Steam"
        "label"        ""
        "totalsize"        "0"
    }
    "1"
    {
        "path"        "D:\\SteamLibrary"
        "totalsize"        "1000202039296"
    }
}
"#;
        let out = ManifestManager::parse_library_folders(vdf);
        assert_eq!(
            out,
            vec![
                "C:\\Program Files (x86)\\Steam".to_string(),
                "D:\\SteamLibrary".to_string()
            ],
            "{:?}",
            out
        );
    }

    /// The older layout, where extra drives are numbered keys holding the
    /// path directly. This is the one that used to be invisible.
    #[test]
    fn reads_the_legacy_numbered_layout() {
        let vdf = r#"
"LibraryFolders"
{
    "TimeNextStatsReport"        "1620000000"
    "ContentStatsID"        "-1234567890123456789"
    "1"        "D:\\SteamLibrary"
    "2"        "F:\\Games\\Steam"
}
"#;
        let out = ManifestManager::parse_library_folders(vdf);
        assert_eq!(
            out,
            vec!["D:\\SteamLibrary".to_string(), "F:\\Games\\Steam".to_string()],
            "{:?}",
            out
        );
    }

    /// The metadata in that file must never be mistaken for a folder — a
    /// numeric value under a numeric key is a size, not a path.
    #[test]
    fn ignores_metadata_and_sizes() {
        let vdf = r#"
"libraryfolders"
{
    "TimeNextStatsReport"        "1620000000"
    "ContentStatsID"        "-1234567890123456789"
    "0"
    {
        "path"        "C:\\Steam"
        "totalsize"        "0"
        "apps"
        {
            "228980"        "1699280"
            "550"        "15000000000"
        }
    }
}
"#;
        let out = ManifestManager::parse_library_folders(vdf);
        assert_eq!(out, vec!["C:\\Steam".to_string()], "{:?}", out);
    }

    #[test]
    fn survives_an_empty_file() {
        assert!(ManifestManager::parse_library_folders("").is_empty());
    }
}


#[cfg(test)]
mod library_dedupe_tests {
    use super::ManifestManager;

    /// What the picker actually showed: the Steam folder twice, once as the
    /// registry spells it and once as libraryfolders.vdf does.
    #[test]
    fn the_same_folder_in_two_cases_is_listed_once() {
        let mut paths = Vec::new();
        ManifestManager::push_library_for_test(&mut paths, r"c:\program files (x86)\steam".into());
        ManifestManager::push_library_for_test(&mut paths, r"C:\Program Files (x86)\Steam".into());
        assert_eq!(paths, vec![r"c:\program files (x86)\steam".to_string()]);
    }

    /// A trailing separator is not a different library either.
    #[test]
    fn a_trailing_slash_is_not_a_second_library() {
        let mut paths = Vec::new();
        ManifestManager::push_library_for_test(&mut paths, r"D:\SteamLibrary".into());
        ManifestManager::push_library_for_test(&mut paths, r"D:\SteamLibrary\".into());
        assert_eq!(paths.len(), 1);
    }

    /// Genuinely different drives must all survive.
    #[test]
    fn different_libraries_are_all_kept() {
        let mut paths = Vec::new();
        for p in [r"C:\Steam", r"D:\SteamLibrary", r"E:\SteamLibrary", r"F:\SteamLibrary"] {
            ManifestManager::push_library_for_test(&mut paths, p.into());
        }
        assert_eq!(paths.len(), 4);
    }
}

#[cfg(test)]
mod stale_acf_tests {
    use super::*;

    /// Builds a throwaway library with one ACF, and optionally the game folder.
    fn library(name: &str, state_flags: &str, install_dir: &str, with_files: bool) -> String {
        let root = std::env::temp_dir().join(format!("ragnarok_acf_{name}"));
        let _ = fs::remove_dir_all(&root);
        let steamapps = root.join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        fs::write(
            steamapps.join("appmanifest_4001890.acf"),
            format!(
                "\"AppState\"\n{{\n\t\"appid\"\t\t\"4001890\"\n\t\"name\"\t\t\"How to Fish\"\n\
                 \t\"StateFlags\"\t\t\"{state_flags}\"\n\t\"installdir\"\t\t\"{install_dir}\"\n}}\n"
            ),
        )
        .unwrap();
        let common = steamapps.join("common").join(install_dir);
        fs::create_dir_all(&common).unwrap();
        if with_files {
            fs::write(common.join("game.exe"), b"contenido").unwrap();
        }
        root.to_string_lossy().to_string()
    }

    /// The bug this exists for. An ACF claiming "Fully Installed" over an empty
    /// folder made install_game keep it, Steam then answered `Files Missing`
    /// with an uninstall, and that uninstall took the depot manifest with it.
    #[test]
    fn a_fully_installed_claim_over_an_empty_folder_is_not_trusted() {
        let lib = library("stale", "4", "How to Fish", false);
        assert!(
            !ManifestManager::is_properly_installed(&[lib.clone()], "4001890"),
            "un ACF que miente sobre archivos en disco tiene que descartarse"
        );
        let _ = fs::remove_dir_all(&lib);
    }

    /// And the same claim with the files actually there stays trusted, so a
    /// real install is never wiped and re-downloaded.
    #[test]
    fn a_fully_installed_claim_with_files_is_trusted() {
        let lib = library("real", "4", "How to Fish", true);
        assert!(
            ManifestManager::is_properly_installed(&[lib.clone()], "4001890"),
            "una instalación real no puede marcarse como obsoleta"
        );
        let _ = fs::remove_dir_all(&lib);
    }

    /// A download that just started has an empty `common/` — its bytes are in
    /// `downloading/`. Treating that as stale would delete the ACF and the
    /// manifest of a transfer in progress, so 1026 is deliberately exempt.
    #[test]
    fn a_download_in_progress_is_left_alone() {
        let lib = library("midupdate", "1026", "How to Fish", false);
        assert!(
            ManifestManager::is_properly_installed(&[lib.clone()], "4001890"),
            "una descarga en curso no es un ACF obsoleto"
        );
        let _ = fs::remove_dir_all(&lib);
    }

    /// StateFlags 1 is "owned, nothing downloaded" — it never claimed files, so
    /// the disk check does not even apply and the answer is unchanged.
    #[test]
    fn merely_owned_still_does_not_count_as_installed() {
        let lib = library("owned", "1", "How to Fish", false);
        assert!(!ManifestManager::is_properly_installed(&[lib.clone()], "4001890"));
        let _ = fs::remove_dir_all(&lib);
    }

    /// Without an installdir there is nothing to verify against. Trust the
    /// flags rather than delete someone's ACF on a guess.
    #[test]
    fn an_acf_with_no_installdir_is_trusted_rather_than_guessed_at() {
        let root = std::env::temp_dir().join("ragnarok_acf_noinstalldir");
        let _ = fs::remove_dir_all(&root);
        let steamapps = root.join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        fs::write(
            steamapps.join("appmanifest_4001890.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"4001890\"\n\t\"StateFlags\"\t\t\"4\"\n}\n",
        )
        .unwrap();
        let lib = root.to_string_lossy().to_string();
        assert!(ManifestManager::is_properly_installed(&[lib], "4001890"));
        let _ = fs::remove_dir_all(&root);
    }

    /// Against the machine's real Steam libraries. Opt-in:
    /// `cargo test smoke_real_libraries -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn smoke_real_libraries() {
        let libs = ManifestManager::get_library_folders(r"C:\Program Files (x86)\Steam");
        for app_id in ["4001890", "1245620", "2622380"] {
            let installed = ManifestManager::is_properly_installed(&libs, app_id);
            let mut detail = String::from("sin ACF");
            for lib in &libs {
                let acf = PathBuf::from(lib)
                    .join("steamapps")
                    .join(format!("appmanifest_{}.acf", app_id));
                if let Ok(c) = fs::read_to_string(&acf) {
                    let flags = ManifestManager::acf_value(&c, "StateFlags").unwrap_or_default();
                    let dir = ManifestManager::acf_value(&c, "installdir").unwrap_or_default();
                    let has = ManifestManager::install_dir_has_files(lib, &c);
                    detail = format!("StateFlags={flags} installdir={dir:?} archivos={has}");
                }
            }
            eprintln!("  {app_id}: is_properly_installed={installed}  ({detail})");
        }
    }

    #[test]
    fn acf_value_reads_the_fields_the_check_depends_on() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"4001890\"\n\t\"name\"\t\t\"How to Fish\"\n\
                   \t\"StateFlags\"\t\t\"514\"\n\t\"installdir\"\t\t\"How to Fish\"\n}\n";
        assert_eq!(ManifestManager::acf_value(acf, "StateFlags").as_deref(), Some("514"));
        assert_eq!(ManifestManager::acf_value(acf, "installdir").as_deref(), Some("How to Fish"));
        assert_eq!(ManifestManager::acf_value(acf, "buildid"), None);
    }
}
