use std::fs;
use crate::diag_log;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use serde::{Deserialize, Serialize};
use regex::Regex;
use std::collections::HashMap;
use std::io::Read;
use std::io::Write;
use futures::StreamExt;
use tauri::Manager;
use reqwest::header::{CONTENT_RANGE, RANGE};
use super::download_guard;

/// Where a GitHub release asset legitimately comes from: the release page
/// itself, or the CDN it redirects to. Every unlocker below is published as a
/// GitHub release asset or a raw.githubusercontent blob, so nothing this
/// module downloads should ever resolve anywhere else.
const GITHUB_RELEASE_HOSTS: &[&str] = &["github.com", ".githubusercontent.com"];

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;



#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HVGameFix {
    pub href: String,
    pub filename: String,
    pub size: String,
    pub badges: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HVGame {
    pub buildid: String,
    pub name: String,
    pub fixes: Vec<HVGameFix>,
}



pub struct RustUnlockerManager;

/// Parallel HTTP connections for bypass downloads (no artificial speed cap).
const BYPASS_PARALLEL_CONNECTIONS: usize = 16;
const BYPASS_MIN_PARALLEL_BYTES: u64 = 512 * 1024;
const BYPASS_WRITE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

impl RustUnlockerManager {
    /// Returns AppData temporary directory for downloads
    fn temp_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("com.ragnarok.launcher")
            .join("unlockers_temp")
    }

    /// Fetches the latest release download URL for a given GitHub repository API endpoint.
    async fn get_github_latest_release_zip(api_url: &str, prefer_name: Option<&str>) -> Result<String, String> {
        let client = reqwest::Client::builder()
            .user_agent("Ragnarok-Launcher-App")
            .build()
            .map_err(|e| e.to_string())?;

        let res = client.get(api_url).send().await.map_err(|e| e.to_string())?;
        let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

        let assets = json["assets"]
            .as_array()
            .ok_or_else(|| "No assets found in release".to_string())?;

        // First pass: prefer asset whose name contains the filter keyword (excludes "Source code" zips)
        if let Some(keyword) = prefer_name {
            for asset in assets {
                if let Some(name) = asset["name"].as_str() {
                    if (name.ends_with(".zip") || name.ends_with(".rar"))
                        && name.to_lowercase().contains(&keyword.to_lowercase())
                    {
                        if let Some(url) = asset["browser_download_url"].as_str() {
                            return Ok(url.to_string());
                        }
                    }
                }
            }
        }

        // Fallback: any zip/rar
        for asset in assets {
            if let Some(name) = asset["name"].as_str() {
                if name.ends_with(".zip") || name.ends_with(".rar") {
                    if let Some(url) = asset["browser_download_url"].as_str() {
                        return Ok(url.to_string());
                    }
                }
            }
        }

        Err("No suitable ZIP/RAR asset found".to_string())
    }

    /// Downloads and extracts a zip file to the target destination
    async fn download_and_extract(url: &str, dest_dir: &Path) -> Result<(), String> {
        // Every caller reaches this with a browser_download_url read out of a
        // GitHub releases API response, so anything else is either a redirect
        // that went somewhere unexpected or an edited constant. Checked before
        // the request rather than after, because what comes back from here is
        // extracted and then loaded into the game process.
        if !download_guard::is_allowed_url(url, GITHUB_RELEASE_HOSTS) {
            return Err(format!("URL de descarga no confiable: {}", url));
        }
        let client = reqwest::Client::new();
        let bytes = client.get(url).send().await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;

        download_guard::verify_download(
            dest_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "la herramienta".into()).as_str(),
            &bytes,
            download_guard::Payload::Zip,
            None,
        )?;

        fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
        let cursor = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            // Reject entries with `..`/absolute paths (zip-slip) instead of
            // blindly joining the raw archive-supplied name.
            let name = match file.enclosed_name() {
                Some(p) => p.to_path_buf(),
                None => continue,
            };
            let outpath = dest_dir.join(&name);

            if file.name().ends_with('/') {
                fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p).map_err(|e| e.to_string())?;
                    }
                }
                let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
                std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Downloads the latest release of a specified unlocker/tool
    pub async fn download_tool(tool_type: &str) -> Result<PathBuf, String> {
        let dest = Self::temp_dir().join(tool_type);
        if dest.exists() {
            // If already cached, reuse
            return Ok(dest);
        }

        if tool_type == "creamapi" {
            // Download DLLs directly from FroggMaster/CreamInstaller repo since the original acidicoala/CreamAPI repo was DMCA'd
            fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            let client = reqwest::Client::builder()
                .user_agent("Ragnarok-Launcher-App")
                .build()
                .map_err(|e| e.to_string())?;
            
            let dll32_url = "https://raw.githubusercontent.com/FroggMaster/CreamInstaller/main/CreamInstaller/Resources/CreamAPI/steam_api.dll";
            let dll64_url = "https://raw.githubusercontent.com/FroggMaster/CreamInstaller/main/CreamInstaller/Resources/CreamAPI/steam_api64.dll";
            
            for url in [dll32_url, dll64_url] {
                if !download_guard::is_allowed_url(url, GITHUB_RELEASE_HOSTS) {
                    return Err(format!("URL de descarga no confiable: {}", url));
                }
            }

            let bytes32 = client.get(dll32_url).send().await
                .map_err(|e| format!("Error descargando CreamAPI 32bit: {}", e))?
                .bytes().await
                .map_err(|e| format!("Error leyendo bytes CreamAPI 32bit: {}", e))?;

            let bytes64 = client.get(dll64_url).send().await
                .map_err(|e| format!("Error descargando CreamAPI 64bit: {}", e))?
                .bytes().await
                .map_err(|e| format!("Error leyendo bytes CreamAPI 64bit: {}", e))?;

            // These two get copied over the game's own steam_api.dll, so the
            // game loads them directly. Confirming they are really PE files
            // stops a raw.githubusercontent error page from being installed
            // as the game's Steam API — which fails at launch with an error
            // that says nothing about a bad download.
            download_guard::verify_download("CreamAPI 32bit", &bytes32, download_guard::Payload::Pe, None)?;
            download_guard::verify_download("CreamAPI 64bit", &bytes64, download_guard::Payload::Pe, None)?;

            fs::write(dest.join("cream_api.dll"), bytes32)
                .map_err(|e| format!("Error escribiendo CreamAPI 32bit: {}", e))?;
            fs::write(dest.join("cream_api64.dll"), bytes64)
                .map_err(|e| format!("Error escribiendo CreamAPI 64bit: {}", e))?;
                
            return Ok(dest);
        }

        let api_url = match tool_type {
            "smokeapi" => "https://api.github.com/repos/acidicoala/SmokeAPI/releases/latest",
            "uplayr1" => "https://api.github.com/repos/acidicoala/UplayR1Unlocker/releases/latest",
            "uplayr2" => "https://api.github.com/repos/acidicoala/UplayR2Unlocker/releases/latest",
            "steamless" => "https://api.github.com/repos/atom0s/Steamless/releases/latest",
            // Ships one zip containing a folder per proxy DLL name (e.g.
            // version-32/version.dll, version-64/version.dll, winmm-32/...),
            // each a prebuilt Koaloader binary for that proxy — see
            // apply_koaloader below for which one gets picked.
            "koaloader" => "https://api.github.com/repos/acidicoala/Koaloader/releases/latest",
            _ => return Err(format!("Unknown tool type: {}", tool_type)),
        };

        let zip_url = Self::get_github_latest_release_zip(api_url, Some(tool_type)).await?;
        Self::download_and_extract(&zip_url, &dest).await?;

        Ok(dest)
    }

    /// Injects CreamAPI into a game's folder, backing up the original steam_api(64).dll.
    pub async fn apply_creamapi(
        game_path: &str,
        appid: &str,
        dlcs: Vec<(String, String)>,
    ) -> Result<String, String> {
        let tool_dir = Self::download_tool("creamapi").await?;
        let game_dir = Path::new(game_path);

        Self::validate_write_permissions(game_dir)?;
        Self::check_disk_space(game_dir, 10 * 1024 * 1024)?;

        // Find standard steam_api.dll or steam_api64.dll in game directory recursively
        let original_dll = Self::find_steam_dll(game_dir)?;
        if Self::is_file_in_use(&original_dll) {
            return Err("Cannot apply CreamAPI — the game's steam_api.dll is in use (game running?). Close the game first.".to_string());
        }
        let is_64bit = original_dll.file_name().unwrap().to_string_lossy().contains("64");

        let backup_dll = original_dll.with_file_name(if is_64bit { "steam_api64_o.dll" } else { "steam_api_o.dll" });
        if !backup_dll.exists() {
            fs::copy(&original_dll, &backup_dll).map_err(|e| e.to_string())?;
        }

        // Copy CreamAPI DLL over the original DLL
        // Looking inside the downloaded folder structure
        let source_dll = if is_64bit {
            tool_dir.join("cream_api64.dll")
        } else {
            tool_dir.join("cream_api.dll")
        };

        if !source_dll.exists() {
            return Err("CreamAPI source DLL not found in downloaded package".to_string());
        }

        fs::copy(&source_dll, &original_dll).map_err(|e| e.to_string())?;

        // Generate cream_api.ini (modern v5.3.0.0+ format with [dlc] section)
        let config_path = original_dll.with_file_name("cream_api.ini");
        let mut config_lines = vec![
            "; CreamAPI Configuration".to_string(),
            "".to_string(),
            "[steam]".to_string(),
            format!("appid = {}", appid),
            "unlockall = false".to_string(),
            "orgapi = steam_api_o.dll".to_string(),
            "orgapi64 = steam_api64_o.dll".to_string(),
            "extraprotection = false".to_string(),
            "forceoffline = false".to_string(),
            "".to_string(),
            "[steam_misc]".to_string(),
            "disableuserinterface = false".to_string(),
            "".to_string(),
            "[dlc]".to_string(),
        ];

        for (dlc_id, dlc_name) in dlcs {
            config_lines.push(format!("{} = {}", dlc_id, dlc_name));
        }

        fs::write(config_path, config_lines.join("\n")).map_err(|e| e.to_string())?;

        Ok("CreamAPI applied successfully".to_string())
    }

    /// Injects SmokeAPI into a game's folder, backing up the original steam_api(64).dll.
    pub async fn apply_smokeapi(
        game_path: &str,
        _appid: &str,
        dlc_ids: Vec<String>,
    ) -> Result<String, String> {
        let tool_dir = Self::download_tool("smokeapi").await?;
        let game_dir = Path::new(game_path);

        Self::validate_write_permissions(game_dir)?;
        Self::check_disk_space(game_dir, 10 * 1024 * 1024)?;

        let original_dll = Self::find_steam_dll(game_dir)?;
        if Self::is_file_in_use(&original_dll) {
            return Err("Cannot apply SmokeAPI — the game's steam_api.dll is in use (game running?). Close the game first.".to_string());
        }
        let is_64bit = original_dll.file_name().unwrap().to_string_lossy().contains("64");

        let backup_dll = original_dll.with_file_name(if is_64bit { "steam_api64_o.dll" } else { "steam_api_o.dll" });
        if !backup_dll.exists() {
            fs::copy(&original_dll, &backup_dll).map_err(|e| e.to_string())?;
        }

        let source_dll = if is_64bit {
            tool_dir.join("smoke_api64.dll")
        } else {
            tool_dir.join("smoke_api32.dll")
        };

        if !source_dll.exists() {
            return Err("SmokeAPI source DLL not found in downloaded package".to_string());
        }

        fs::copy(&source_dll, &original_dll).map_err(|e| e.to_string())?;

        // Generate SmokeAPI.config.json (v4 format with default_app_status + extra_dlcs)
        let config_path = original_dll.with_file_name("SmokeAPI.config.json");
        let extra_dlcs: serde_json::Value = dlc_ids.iter()
            .filter_map(|id| id.parse::<u64>().ok())
            .map(|id| (id.to_string(), serde_json::json!({})))
            .collect::<serde_json::Map<_, _>>()
            .into();
        let config_data = serde_json::json!({
            "$version": 4,
            "logging": false,
            "log_steam_http": false,
            "default_app_status": "unlocked",
            "override_app_status": {},
            "override_dlc_status": {},
            "auto_inject_inventory": true,
            "extra_inventory_items": [],
            "extra_dlcs": extra_dlcs
        });

        fs::write(config_path, serde_json::to_string_pretty(&config_data).unwrap()).map_err(|e| e.to_string())?;

        Ok("SmokeAPI applied successfully".to_string())
    }

    /// Injects Uplay R1 or R2 Unlocker
    pub async fn apply_uplay_unlocker(game_path: &str, is_r2: bool) -> Result<String, String> {
        let tool_type = if is_r2 { "uplayr2" } else { "uplayr1" };
        let tool_dir = Self::download_tool(tool_type).await?;
        let game_dir = Path::new(game_path);

        Self::validate_write_permissions(game_dir)?;
        Self::check_disk_space(game_dir, 10 * 1024 * 1024)?;

        // Find uplay_r1_loader(64).dll or upc_r2_loader(64).dll recursively
        let uplay_dll_names = [
            "uplay_r1_loader.dll", "uplay_r1_loader64.dll",
            "upc_r2_loader.dll", "upc_r2_loader64.dll",
            "uplay_r2_loader.dll", "uplay_r2_loader64.dll",
        ];
        let mut original_dll: Option<PathBuf> = None;
        for entry in walkdir::WalkDir::new(game_dir).into_iter().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if uplay_dll_names.iter().any(|&n| name == n) {
                original_dll = Some(entry.path().to_path_buf());
                break;
            }
        }

        let original_dll = original_dll.ok_or_else(|| "Uplay loader DLL not found in game folder".to_string())?;
        if Self::is_file_in_use(&original_dll) {
            return Err("Cannot apply Uplay unlocker — the game's loader DLL is in use (game running?). Close the game first.".to_string());
        }
        let is_64bit = original_dll.file_name().unwrap().to_string_lossy().contains("64");

        let backup_dll = original_dll.with_file_name(if is_64bit { "uplay_loader64_o.dll" } else { "uplay_loader_o.dll" });
        if !backup_dll.exists() {
            fs::copy(&original_dll, &backup_dll).map_err(|e| e.to_string())?;
        }

        let source_dll = if is_r2 {
            // Uplay R2 unlocker may ship as UplayR2Unlocker.dll or upc_r2_loader.dll
            let candidates = [
                tool_dir.join("UplayR2Unlocker.dll"),
                tool_dir.join("UplayR2Unlocker64.dll"),
                tool_dir.join("upc_r2_loader.dll"),
                tool_dir.join("upc_r2_loader64.dll"),
                tool_dir.join("uplay_r2_loader.dll"),
                tool_dir.join("uplay_r2_loader64.dll"),
            ];
            candidates.iter().find(|p| p.exists()).cloned()
                .ok_or_else(|| "Uplay R2 Unlocker DLL not found in downloaded package".to_string())?
        } else {
            let candidates = [
                tool_dir.join("uplay_r1_loader.dll"),
                tool_dir.join("uplay_r1_loader64.dll"),
                tool_dir.join("UplayR1Unlocker.dll"),
                tool_dir.join("UplayR1Unlocker64.dll"),
            ];
            candidates.iter().find(|p| p.exists()).cloned()
                .ok_or_else(|| "Uplay R1 Unlocker DLL not found in downloaded package".to_string())?
        };

        if !source_dll.exists() {
            return Err("Uplay Unlocker source DLL not found in downloaded package".to_string());
        }

        fs::copy(&source_dll, &original_dll).map_err(|e| e.to_string())?;

        Ok(format!("Uplay Unlocker ({}) applied successfully", tool_type))
    }

    /// Fallback injection path for games where directly replacing
    /// steam_api(64).dll (what apply_creamapi/apply_smokeapi do) doesn't
    /// work — some games hash-check their own steam_api.dll at startup and
    /// refuse to run once it's been swapped. Koaloader avoids that entirely:
    /// steam_api.dll is never touched. Instead it impersonates an unrelated
    /// system DLL the game already loads (version.dll — chosen because it's
    /// the proxy name least likely to collide with something a game
    /// actually needs) and, once loaded into the process that way, loads
    /// SmokeAPI's DLL itself, which then hooks the Steam API calls in
    /// memory. Reuses SmokeAPI for the actual unlock/DLC logic — Koaloader
    /// on its own is just the loader, not an unlocker.
    pub async fn apply_koaloader(game_path: &str, _appid: &str, dlc_ids: Vec<String>) -> Result<String, String> {
        const PROXY: &str = "version";

        let koaloader_dir = Self::download_tool("koaloader").await?;
        let smokeapi_dir = Self::download_tool("smokeapi").await?;
        let game_dir = Path::new(game_path);

        Self::validate_write_permissions(game_dir)?;
        Self::check_disk_space(game_dir, 20 * 1024 * 1024)?;

        // Only used to detect bitness and the exact folder the game loads
        // its DLLs from (not always game_dir's top level) — steam_api itself
        // is never modified here, unlike CreamAPI/SmokeAPI's direct mode.
        let steam_dll = Self::find_steam_dll(game_dir)?;
        let is_64bit = steam_dll.file_name().unwrap().to_string_lossy().contains("64");
        let target_dir = steam_dll.parent()
            .ok_or_else(|| "Could not resolve the game's DLL folder".to_string())?;

        let arch = if is_64bit { "64" } else { "32" };
        let proxy_source = koaloader_dir.join(format!("{}-{}", PROXY, arch)).join(format!("{}.dll", PROXY));
        if !proxy_source.exists() {
            return Err(format!("Koaloader proxy DLL not found in downloaded package ({})", proxy_source.display()));
        }

        let proxy_dest = target_dir.join(format!("{}.dll", PROXY));
        if Self::is_file_in_use(&proxy_dest) {
            return Err("Cannot apply Koaloader — a file it needs to write is in use (game running?). Close the game first.".to_string());
        }
        // Back up a real version.dll if the game actually shipped one —
        // rare, but overwriting it without a backup would be unrecoverable.
        let proxy_backup = target_dir.join(format!("{}_o.dll", PROXY));
        if proxy_dest.exists() && !proxy_backup.exists() {
            fs::copy(&proxy_dest, &proxy_backup).map_err(|e| e.to_string())?;
        }
        fs::copy(&proxy_source, &proxy_dest).map_err(|e| e.to_string())?;

        let smoke_dest_name = if is_64bit { "smoke_api64.dll" } else { "smoke_api32.dll" };
        let smoke_source = smokeapi_dir.join(smoke_dest_name);
        if !smoke_source.exists() {
            return Err("SmokeAPI source DLL not found in downloaded package".to_string());
        }
        fs::copy(&smoke_source, target_dir.join(smoke_dest_name)).map_err(|e| e.to_string())?;

        // auto_load already knows the well-known smoke_api* filenames, but
        // listing it explicitly in `modules` is more reliable than counting
        // on that heuristic.
        let koaloader_config = serde_json::json!({
            "logging": false,
            "enabled": true,
            "auto_load": true,
            "targets": [],
            "modules": [{ "path": smoke_dest_name, "required": true }]
        });
        fs::write(
            target_dir.join("Koaloader.config.json"),
            serde_json::to_string_pretty(&koaloader_config).unwrap(),
        ).map_err(|e| e.to_string())?;

        // Same SmokeAPI.config.json format apply_smokeapi writes.
        let extra_dlcs: serde_json::Value = dlc_ids.iter()
            .filter_map(|id| id.parse::<u64>().ok())
            .map(|id| (id.to_string(), serde_json::json!({})))
            .collect::<serde_json::Map<_, _>>()
            .into();
        let smoke_config = serde_json::json!({
            "$version": 4,
            "logging": false,
            "log_steam_http": false,
            "default_app_status": "unlocked",
            "override_app_status": {},
            "override_dlc_status": {},
            "auto_inject_inventory": true,
            "extra_inventory_items": [],
            "extra_dlcs": extra_dlcs
        });
        fs::write(
            target_dir.join("SmokeAPI.config.json"),
            serde_json::to_string_pretty(&smoke_config).unwrap(),
        ).map_err(|e| e.to_string())?;

        Ok("Koaloader (con SmokeAPI) aplicado correctamente".to_string())
    }

    /// Uninstalls Koaloader: restores a real version.dll if one was backed
    /// up, otherwise removes the one Koaloader created; removes SmokeAPI's
    /// files it placed alongside it.
    pub fn uninstall_koaloader(game_path: &str) -> Result<String, String> {
        let dir = Path::new(game_path);
        if !dir.exists() {
            return Err("Game directory does not exist".to_string());
        }

        // Snapshot which folders have a version_o.dll backup BEFORE any
        // mutation below — needed so the single pass that follows can tell
        // "this version.dll has a backup, leave it to the restore branch"
        // from "we created this from scratch, just delete it" without the
        // two meanings colliding once a restore has already consumed its
        // own backup marker.
        let dirs_with_backup: std::collections::HashSet<PathBuf> = walkdir::WalkDir::new(dir)
            .into_iter()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("version_o.dll"))
            .filter_map(|e| e.path().parent().map(|p| p.to_path_buf()))
            .collect();

        let mut had_koaloader_marker = false;
        let mut changed = 0u32;

        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path().to_path_buf();
            if !path.is_file() { continue; }
            let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
            match name.as_str() {
                "koaloader.config.json" => {
                    had_koaloader_marker = true;
                    let _ = fs::remove_file(&path);
                    changed += 1;
                }
                "smokeapi.config.json" | "smoke_api.dll" | "smoke_api32.dll" | "smoke_api64.dll" => {
                    let _ = fs::remove_file(&path);
                    changed += 1;
                }
                "version_o.dll" => {
                    let original_path = path.with_file_name("version.dll");
                    if fs::copy(&path, &original_path).is_ok() {
                        let _ = fs::remove_file(&path);
                        changed += 1;
                    }
                }
                "version.dll" => {
                    let parent_had_backup = path.parent().map(|p| dirs_with_backup.contains(p)).unwrap_or(false);
                    if !parent_had_backup {
                        let _ = fs::remove_file(&path);
                        changed += 1;
                    }
                }
                _ => {}
            }
        }

        if !had_koaloader_marker {
            return Err("No Koaloader installation found".to_string());
        }
        Ok(format!("Koaloader uninstalled: {} file(s) restored/removed", changed))
    }

    /// Whether another process is holding this file open (Windows).
    ///
    /// The check used to compare against `ErrorKind::PermissionDenied`, which
    /// is not what Windows reports for a file held by someone else: opening a
    /// DLL a running game has mapped gives `ERROR_SHARING_VIOLATION` (32),
    /// which Rust surfaces as `ErrorKind::Uncategorized`. So the guard never
    /// fired once — applying a fix with the game open skipped the "close the
    /// game first" message entirely and failed further down in `fs::copy`,
    /// showing the raw OS text, and by then a backup had already been written
    /// so a retry saw one present and did not make a fresh one.
    ///
    /// Matched on the raw codes instead, since those are what the OS actually
    /// returns:
    ///   32 ERROR_SHARING_VIOLATION — someone else has it open
    ///   33 ERROR_LOCK_VIOLATION    — a byte range is locked
    ///    5 ERROR_ACCESS_DENIED     — read-only, or ACLs
    fn is_file_in_use(path: &Path) -> bool {
        if !path.exists() { return false; }
        #[cfg(windows)]
        {
            use std::fs::OpenOptions;
            match OpenOptions::new().read(true).write(true).open(path) {
                Ok(_) => false,
                Err(e) => {
                    matches!(e.raw_os_error(), Some(32) | Some(33) | Some(5))
                        || e.kind() == std::io::ErrorKind::PermissionDenied
                }
            }
        }
        #[cfg(not(windows))]
        { false }
    }

    /// Validate write permissions for a directory
    fn validate_write_permissions(dir: &Path) -> Result<(), String> {
        if !dir.exists() {
            return Err(format!("Directory does not exist: {}", dir.display()));
        }
        let test_file = dir.join(".ragnarok_write_test");
        match fs::write(&test_file, b"test") {
            Ok(_) => {
                let _ = fs::remove_file(&test_file);
                Ok(())
            }
            Err(e) => Err(format!("No write permission for directory {}: {}", dir.display(), e)),
        }
    }

    /// Check disk space availability (best-effort, always succeeds if unavailable)
    fn check_disk_space(dir: &Path, _required_bytes: u64) -> Result<(), String> {
        // Simple check: if we can write a test file, assume enough space
        Self::validate_write_permissions(dir)?;
        Ok(())
    }

    /// Uninstalls SmokeAPI: restores original steam_api DLLs from _o backups, deletes config
    pub fn uninstall_smokeapi(game_path: &str) -> Result<String, String> {
        let dir = Path::new(game_path);
        if !dir.exists() {
            return Err("Game directory does not exist".to_string());
        }
        let mut restored = 0u32;
        // Find all _o.dll backup files
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() { continue; }
            let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
            if name == "steam_api_o.dll" || name == "steam_api64_o.dll" {
                let original_name = if name == "steam_api_o.dll" { "steam_api.dll" } else { "steam_api64.dll" };
                let original_path = path.with_file_name(original_name);
                // Copy backup over original
                fs::copy(path, &original_path).map_err(|e| format!("Error restoring {}: {}", original_name, e))?;
                fs::remove_file(path).map_err(|e| format!("Error removing backup {}: {}", name, e))?;
                restored += 1;
            }
        }
        // Delete config files
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().map(|n| n.to_string_lossy().to_lowercase()) == Some("smokeapi.config.json".into()) {
                let _ = fs::remove_file(path);
            }
        }
        if restored == 0 {
            return Err("No SmokeAPI installation found (no _o.dll backups)".to_string());
        }
        Ok(format!("SmokeAPI uninstalled: {} backup(s) restored", restored))
    }

    /// Uninstalls CreamAPI: restores original steam_api DLLs from _o backups, deletes config
    pub fn uninstall_creamapi(game_path: &str) -> Result<String, String> {
        let dir = Path::new(game_path);
        if !dir.exists() {
            return Err("Game directory does not exist".to_string());
        }
        let mut restored = 0u32;
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() { continue; }
            let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
            if name == "steam_api_o.dll" || name == "steam_api64_o.dll" {
                let original_name = if name == "steam_api_o.dll" { "steam_api.dll" } else { "steam_api64.dll" };
                let original_path = path.with_file_name(original_name);
                fs::copy(path, &original_path).map_err(|e| format!("Error restoring {}: {}", original_name, e))?;
                fs::remove_file(path).map_err(|e| format!("Error removing backup {}: {}", name, e))?;
                restored += 1;
            }
        }
        // Delete cream_api.ini files
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().map(|n| n.to_string_lossy().to_lowercase()) == Some("cream_api.ini".into()) {
                let _ = fs::remove_file(path);
            }
        }
        if restored == 0 {
            return Err("No CreamAPI installation found (no _o.dll backups)".to_string());
        }
        Ok(format!("CreamAPI uninstalled: {} backup(s) restored", restored))
    }

    /// Uninstalls Uplay Unlocker (R1 or R2): restores original DLL, deletes config
    pub fn uninstall_uplay_unlocker(game_path: &str) -> Result<String, String> {
        let dir = Path::new(game_path);
        if !dir.exists() {
            return Err("Game directory does not exist".to_string());
        }
        let mut restored = 0u32;
        // Find uplay_loader_o.dll backup files
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() { continue; }
            let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
            if name == "uplay_loader_o.dll" || name == "uplay_loader64_o.dll" {
                let is_64bit = name == "uplay_loader64_o.dll";
                // Try restoring with the original R1 name first, then R2 name
                let candidates = if is_64bit {
                    ["uplay_r1_loader64.dll", "upc_r2_loader64.dll", "uplay_r2_loader64.dll"]
                } else {
                    ["uplay_r1_loader.dll", "upc_r2_loader.dll", "uplay_r2_loader.dll"]
                };
                let mut restored_any = false;
                for candidate in &candidates {
                    let original_path = path.with_file_name(candidate);
                    if original_path.exists() {
                        // If the candidate exists, it's the current unlocker DLL, overwrite it
                        fs::copy(path, &original_path).map_err(|e| format!("Error restoring {}: {}", candidate, e))?;
                        fs::remove_file(path).map_err(|e| format!("Error removing backup {}: {}", name, e))?;
                        restored += 1;
                        restored_any = true;
                        break;
                    }
                }
                if !restored_any {
                    // No original found — just remove the backup and rename it as the most likely original
                    let likely_original = if is_64bit { "upc_r2_loader64.dll" } else { "upc_r2_loader.dll" };
                    let original_path = path.with_file_name(likely_original);
                    fs::rename(path, &original_path).map_err(|e| format!("Error renaming backup to {}: {}", likely_original, e))?;
                    restored += 1;
                }
            }
        }
        // Delete Uplay unlocker config files
        let config_names = ["UplayR1Unlocker.jsonc", "UplayR2Unlocker.jsonc"];
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(fname) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) {
                    if config_names.iter().any(|c| c.to_lowercase() == fname) {
                        let _ = fs::remove_file(path);
                    }
                }
            }
        }
        if restored == 0 {
            return Err("No Uplay unlocker installation found (no uplay_loader_o.dll backups)".to_string());
        }
        Ok(format!("Uplay unlocker uninstalled: {} backup(s) restored", restored))
    }

    /// Helper to find steam_api.dll or steam_api64.dll recursively
    fn find_steam_dll(dir: &Path) -> Result<PathBuf, String> {
        // Steam wins when both are present, because both usually ARE present.
        //
        // This used to scan the whole tree for a Uplay loader FIRST and bail
        // with "this game uses Uplay, not Steam" the moment it found one. Every
        // Ubisoft title sold on Steam — Assassin's Creed, Far Cry, The
        // Division, Anno — ships Uplay as middleware alongside a perfectly
        // real steam_api64.dll, so that check fired on games that are Steam
        // releases. The message is a flat assertion about the game, so the user
        // applied the Uplay unlocker, SmokeAPI never ran, and there was nothing
        // to suggest trying again the other way.
        //
        // Uplay-only stays a useful answer, so it is still reported — just
        // after establishing that there is no Steam DLL to use.
        let mut uplay_seen = false;
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if (name == "steam_api.dll" || name == "steam_api64.dll") && !name.contains("_o") {
                return Ok(entry.path().to_path_buf());
            }
            if (name.contains("uplay_r") && name.contains("loader"))
                || name == "upc_r2_loader.dll"
                || name == "upc_r2_loader64.dll"
            {
                uplay_seen = true;
            }
        }

        if uplay_seen {
            return Err("Este juego usa Uplay y no trae steam_api.dll. Usá el unlocker 'Uplay R1' o 'Uplay R2' en vez de SmokeAPI.".to_string());
        }
        Err("steam_api.dll o steam_api64.dll no encontrado. SmokeAPI solo funciona con juegos Steam. Si el juego usa Uplay, elegí 'Uplay R1' o 'Uplay R2'.".to_string())
    }

    /// Detecta qué tipo de unlocker necesita el juego basado en los DLLs presentes
    pub fn detect_unlocker_type(game_path: &str) -> String {
        let dir = Path::new(game_path);
        if !dir.exists() {
            return "creamapi".to_string();
        }
        let mut has_steam_api = false;
        let mut has_uplay_r1 = false;
        let mut has_uplay_r2 = false;
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name == "steam_api.dll" || name == "steam_api64.dll" {
                has_steam_api = true;
            }
            if name == "uplay_r1_loader.dll" || name == "uplay_r1_loader64.dll" {
                has_uplay_r1 = true;
            }
            if name == "upc_r2_loader.dll" || name == "upc_r2_loader64.dll" || name == "uplay_r2_loader.dll" || name == "uplay_r2_loader64.dll" {
                has_uplay_r2 = true;
            }
        }
        if has_uplay_r2 { "uplayr2".to_string() }
        else if has_uplay_r1 { "uplayr1".to_string() }
        else if has_steam_api { "creamapi".to_string() }
        else { "creamapi".to_string() }
    }

    /// Unpacks SteamStub DRM from game executable using Steamless CLI
    pub async fn run_steamless(exe_path: &str) -> Result<String, String> {
        // Clear any cached stale Steamless to force fresh download
        let tool_dir = Self::temp_dir().join("steamless");
        if tool_dir.exists() {
            let _ = fs::remove_dir_all(&tool_dir);
        }

        let tool_dir = Self::download_tool("steamless").await?;
        
        let steamless_cli = tool_dir.join("Steamless.CLI.exe");
        if !steamless_cli.exists() {
            return Err("Steamless.CLI.exe not found".to_string());
        }

        // Try without --keepbind which can conflict with some exes
        let output = Command::new(&steamless_cli)
            .args([exe_path, "--realign"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("Failed to run Steamless: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let combined = format!("{}{}", stdout, stderr);

        // Always include raw output for debugging
        let raw_output = if stderr.trim().is_empty() { stdout.trim().to_string() } else { combined.trim().to_string() };

        // Steamless exits with code 0 even on failure — detect actual failures via output content
        let unpackers_failed = raw_output.contains("All unpackers failed");
        let no_drm = raw_output.contains("No DRM") || raw_output.contains("not protected");

        if !output.status.success() || unpackers_failed || no_drm {
            let base_msg = if no_drm {
                "El ejecutable no parece tener SteamStub DRM (puede que ya esté desprotegido, use otro DRM, o no tenga DRM).".to_string()
            } else if unpackers_failed {
                "Ningún plugin de Steamless pudo desempacar el archivo. El ejecutable puede no tener SteamStub DRM, usar una variante no soportada, o estar protegido con otro DRM.".to_string()
            } else {
                format!("Steamless falló con código {}.", output.status)
            };
            let truncated: String = raw_output.chars().take(2000).collect();
            return Err(format!("{}\n\n--- Steamless output ---\n{}", base_msg, truncated));
        }

        // Check if the unpacked file was actually produced
        let unpacked_path = format!("{}.unpacked.exe", exe_path.trim_end_matches(".exe"));
        if Path::new(&unpacked_path).exists() {
            // Rename old exe to .bak and replace with unpacked
            let bak_path = format!("{}.bak", exe_path);
            let _ = fs::rename(exe_path, &bak_path);
            fs::rename(&unpacked_path, exe_path).map_err(|e| e.to_string())?;
            return Ok("SteamStub DRM removido exitosamente. El ejecutable original fue respaldado como .bak".to_string());
        }

        // Steamless reported success but produced no output file — treat as soft failure
        Err(format!(
            "Steamless terminó sin errores pero no generó el archivo desempacado. \
             Es posible que el ejecutable no tenga SteamStub DRM activo.\n{}",
            raw_output
        ))
    }

    /// Checks Steam Store API for DRM information about a game.
    /// Returns a JSON string: { "status": "clean"|"denuvo"|"third_party"|"drm_detected"|"error", "message": "..." }
    pub async fn check_game_drm(app_id: &str) -> String {
        let client = reqwest::Client::builder()
            .user_agent("Ragnarok-Launcher-App")
            .build();

        let client = match client {
            Ok(c) => c,
            Err(_) => return r#"{"status":"error","message":"Failed to create HTTP client"}"#.to_string(),
        };

        let url = format!("https://store.steampowered.com/api/appdetails?appids={}&l=english", app_id);

        let res = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => return format!(r#"{{"status":"error","message":"Network error: {}"}}"#, e),
        };

        let json: serde_json::Value = match res.json().await {
            Ok(j) => j,
            Err(e) => return format!(r#"{{"status":"error","message":"Parse error: {}"}}"#, e),
        };

        let success = json[app_id]["success"].as_bool().unwrap_or(false);
        if !success {
            return r#"{"status":"error","message":"Steam Store returned no data for this app"}"#.to_string();
        }

        let drm_notice = json[app_id]["data"]["drm_notice"].as_str().unwrap_or("").to_lowercase();
        let ext_account = json[app_id]["data"]["ext_user_account_notice"].as_str().unwrap_or("");

        if drm_notice.contains("denuvo") {
            return format!(r#"{{"status":"denuvo","message":"Denuvo DRM detected: {}"}}"#, 
                json[app_id]["data"]["drm_notice"].as_str().unwrap_or(""));
        }

        if !ext_account.is_empty() {
            return format!(r#"{{"status":"third_party","message":"Third-party account required: {}"}}"#, ext_account);
        }

        if !drm_notice.is_empty() {
            return format!(r#"{{"status":"drm_detected","message":"DRM detected: {}"}}"#,
                json[app_id]["data"]["drm_notice"].as_str().unwrap_or(""));
        }

        r#"{"status":"clean","message":"No DRM detected"}"#.to_string()
    }

    /// Fetches HVAuto game list from the RagnarokManifests GitHub repository.
    pub async fn fetch_hv_games() -> Result<Vec<HVGame>, String> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| e.to_string())?;

        let url = "https://raw.githubusercontent.com/RagnarokManifests/games/HYPERVISOR/HV.json";
        let res = client.get(url).send().await
            .map_err(|e| format!("Error al conectar con GitHub para obtener hypervisor: {}", e))?;
            
        let json_text = res.text().await
            .map_err(|e| format!("Error al leer respuesta de GitHub hypervisor: {}", e))?;

        let games: Vec<HVGame> = serde_json::from_str(&json_text)
            .map_err(|e| format!("Error al parsear el JSON de Hypervisor: {}", e))?;

        Ok(games)
    }



    async fn download_with_progress(
        app_handle: Option<&tauri::AppHandle>,
        client: &reqwest::Client,
        url: &str,
        dest_path: &Path,
        href_key: &str,
    ) -> Result<(), String> {
        Self::stream_download_to_file(
            app_handle,
            client,
            url,
            dest_path,
            href_key,
            "hv_download_progress",
            "href",
        )
        .await
    }

    /// HTTP client tuned for large file downloads (no throughput limit).
    pub fn build_download_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .redirect(reqwest::redirect::Policy::limited(10))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(BYPASS_PARALLEL_CONNECTIONS)
            .timeout(std::time::Duration::from_secs(7200))
            .build()
            .map_err(|e| e.to_string())
    }

    fn parse_content_range_total(value: &str) -> Option<u64> {
        value.split('/').nth(1)?.trim().parse().ok()
    }

    /// Determines the file size and whether the server truly supports parallel
    /// range downloads.
    ///
    /// This used to trust the `Accept-Ranges` header from a HEAD request, but
    /// many of the hosts used here (Mediafire, Google Drive, Buzzheavier
    /// mirrors) honor `Range` requests correctly without ever advertising that
    /// header — so downloads were silently falling back to a single slow
    /// connection even when 16-way parallel downloading would have worked.
    /// The only reliable test is to actually send a tiny ranged request and
    /// check for a real `206 Partial Content` response.
    async fn probe_download(client: &reqwest::Client, url: &str) -> Result<(u64, bool), String> {
        let resp = client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            // Only `Content-Range` can answer "how big is the whole file" here.
            //
            // This used to fall back to `content_length()`, but inside a 206
            // that is the length of the *partial* body — one byte, because the
            // request asked for `bytes=0-0`. A mirror that answers 206 without
            // a Content-Range header (legal, and common behind CDN proxies)
            // therefore reported a total of 1. The download itself then went
            // fine, and the anti-truncation check further down compared the
            // real file against that 1 and deleted it with "se esperaban 1
            // bytes y se guardaron 734003200". That host failed every single
            // time, and the message pointed at the wrong cause.
            //
            // Reporting 0 means "unknown": the caller skips the size check
            // rather than deleting a file that is probably fine, and takes the
            // single-stream path since parallel ranges need a known length.
            let total = resp
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(Self::parse_content_range_total)
                .unwrap_or(0);
            return Ok((total, total > 0));
        }

        // Server ignored the Range header and sent the full body (200 OK) —
        // no parallel support, fall back to a single-stream download.
        Ok((resp.content_length().unwrap_or(0), false))
    }

    fn spawn_progress_reporter(
        app_handle: Option<tauri::AppHandle>,
        progress_key: String,
        progress_field: String,
        event_name: String,
        downloaded: Arc<AtomicU64>,
        total_size: u64,
        done: Arc<AtomicU64>,
    ) {
        if app_handle.is_none() || total_size == 0 {
            return;
        }
        let app = app_handle.unwrap();
        tokio::spawn(async move {
            use tokio::time::{sleep, Duration};
            loop {
                sleep(Duration::from_secs(1)).await;
                if done.load(Ordering::Relaxed) != 0 {
                    break;
                }
                let d = downloaded.load(Ordering::Relaxed);
                let pct = if total_size > 0 {
                    (d * 100 / total_size) as u32
                } else {
                    0
                };
                let mut payload = serde_json::json!({
                    "downloaded": d,
                    "total": total_size,
                    "percentage": pct,
                    "status": "downloading",
                });
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(
                        progress_field.clone(),
                        serde_json::Value::String(progress_key.clone()),
                    );
                }
                let _ = app.emit_all(&event_name, payload);
            }
        });
    }

    fn emit_download_progress(
        app_handle: Option<&tauri::AppHandle>,
        progress_key: &str,
        progress_field: &str,
        event_name: &str,
        downloaded: u64,
        total: u64,
        percentage: u32,
        status: &str,
    ) {
        if let Some(handle) = app_handle {
            let mut payload = serde_json::json!({
                "downloaded": downloaded,
                "total": total,
                "percentage": percentage,
                "status": status,
            });
            if let Some(obj) = payload.as_object_mut() {
                obj.insert(
                    progress_field.to_string(),
                    serde_json::Value::String(progress_key.to_string()),
                );
            }
            let _ = handle.emit_all(event_name, payload);
        }
    }

    async fn download_range_part(
        client: reqwest::Client,
        url: String,
        start: u64,
        end: u64,
        part_path: PathBuf,
        downloaded_counter: Arc<AtomicU64>,
    ) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;

        let resp = client
            .get(&url)
            .header(RANGE, format!("bytes={}-{}", start, end))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!("Error HTTP en segmento {}-{}: {}", start, end, resp.status()));
        }

        let mut writer = tokio::io::BufWriter::with_capacity(
            BYPASS_WRITE_BUFFER_BYTES,
            tokio::fs::File::create(&part_path)
                .await
                .map_err(|e| e.to_string())?,
        );
        let mut stream = resp.bytes_stream();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| e.to_string())?;
            downloaded_counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            writer.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }

        writer.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn parallel_download_to_file(
        client: &reqwest::Client,
        url: &str,
        dest_path: &Path,
        total_size: u64,
        downloaded_counter: Arc<AtomicU64>,
    ) -> Result<(), String> {
        use tokio::io::{AsyncWriteExt, BufWriter};

        let connections = BYPASS_PARALLEL_CONNECTIONS as u64;
        let part_size = total_size.div_ceil(connections);
        let parts_dir = std::env::temp_dir().join(format!(
            "rl_bypass_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        tokio::fs::create_dir_all(&parts_dir)
            .await
            .map_err(|e| e.to_string())?;

        let mut tasks = Vec::new();
        let mut part_paths: Vec<PathBuf> = Vec::new();
        let url = url.to_string();

        for i in 0..connections {
            let start = i * part_size;
            if start >= total_size {
                break;
            }
            let end = std::cmp::min(start + part_size - 1, total_size - 1);
            let part_path = parts_dir.join(format!("part_{:02}", i));
            part_paths.push(part_path.clone());

            let client = client.clone();
            let url = url.clone();
            let counter = Arc::clone(&downloaded_counter);
            tasks.push(tokio::spawn(async move {
                Self::download_range_part(client, url, start, end, part_path, counter).await
            }));
        }

        let mut first_err: Option<String> = None;
        for task in tasks {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(format!("Error en tarea de descarga: {}", e));
                    }
                }
            }
        }

        if let Some(err) = first_err {
            let _ = tokio::fs::remove_dir_all(&parts_dir).await;
            return Err(err);
        }

        let mut out = BufWriter::with_capacity(
            BYPASS_WRITE_BUFFER_BYTES,
            tokio::fs::File::create(dest_path)
                .await
                .map_err(|e| e.to_string())?,
        );

        for part_path in &part_paths {
            let mut part = tokio::fs::File::open(part_path)
                .await
                .map_err(|e| e.to_string())?;
            tokio::io::copy(&mut part, &mut out)
                .await
                .map_err(|e| e.to_string())?;
        }

        out.flush().await.map_err(|e| e.to_string())?;
        let _ = tokio::fs::remove_dir_all(&parts_dir).await;
        Ok(())
    }

    async fn single_stream_download(
        client: &reqwest::Client,
        url: &str,
        dest_path: &Path,
        downloaded_counter: Arc<AtomicU64>,
    ) -> Result<(), String> {
        use tokio::io::{AsyncWriteExt, BufWriter};

        let res = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Error HTTP al descargar: {}", res.status()));
        }

        if res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .contains("text/html")
        {
            return Err(
                "La descarga devolvió una página web en lugar del archivo.".to_string(),
            );
        }

        let mut writer = BufWriter::with_capacity(
            BYPASS_WRITE_BUFFER_BYTES,
            tokio::fs::File::create(dest_path)
                .await
                .map_err(|e| e.to_string())?,
        );
        let mut stream = res.bytes_stream();
        let mut checked_header = false;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| e.to_string())?;

            if !checked_header {
                let sample = &chunk[..chunk.len().min(512)];
                let prefix = String::from_utf8_lossy(sample).trim_start().to_lowercase();
                if prefix.starts_with("<!doctype")
                    || prefix.starts_with("<html")
                    || prefix.starts_with("<?xml")
                {
                    drop(writer);
                    let _ = tokio::fs::remove_file(dest_path).await;
                    return Err(
                        "La descarga devolvió una página web en lugar del archivo.".to_string(),
                    );
                }
                checked_header = true;
            }

            downloaded_counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            writer.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }

        writer.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Maximum-speed download: parallel ranged requests when supported, otherwise single stream.
    pub async fn stream_download_to_file(
        app_handle: Option<&tauri::AppHandle>,
        client: &reqwest::Client,
        url: &str,
        dest_path: &Path,
        progress_key: &str,
        event_name: &str,
        progress_field: &str,
    ) -> Result<(), String> {
        use tokio::io::AsyncReadExt;

        let (mut total_size, supports_ranges) = Self::probe_download(client, url).await?;
        let downloaded_counter = Arc::new(AtomicU64::new(0));
        let done_flag = Arc::new(AtomicU64::new(0));

        Self::spawn_progress_reporter(
            app_handle.cloned(),
            progress_key.to_string(),
            progress_field.to_string(),
            event_name.to_string(),
            Arc::clone(&downloaded_counter),
            total_size,
            Arc::clone(&done_flag),
        );

        let result = if supports_ranges && total_size >= BYPASS_MIN_PARALLEL_BYTES {
            match Self::parallel_download_to_file(
                client,
                url,
                dest_path,
                total_size,
                Arc::clone(&downloaded_counter),
            )
            .await
            {
                Ok(()) => Ok(()),
                Err(_) => {
                    downloaded_counter.store(0, Ordering::Relaxed);
                    Self::single_stream_download(
                        client,
                        url,
                        dest_path,
                        Arc::clone(&downloaded_counter),
                    )
                    .await
                }
            }
        } else {
            Self::single_stream_download(
                client,
                url,
                dest_path,
                Arc::clone(&downloaded_counter),
            )
            .await
        };

        // Google Drive answered with its warning page rather than the file.
        //
        // The direct link is built with `confirm=t`, which Drive used to
        // accept for anything and now refuses for larger files — it serves an
        // interstitial carrying the real token (and sometimes a uuid) as
        // hidden form fields instead. resolve_gdrive_download_url already
        // knows how to read those, but only when asked to probe, and nothing
        // asked: the HTML was detected, reported as "la descarga devolvió una
        // página web en lugar del archivo", and the attempt abandoned.
        //
        // So the detection now leads somewhere: probe once for the real token
        // and download again. One retry only — if the second answer is still a
        // page, the file is genuinely unavailable (quota, or taken down) and
        // looping would just hide that.
        let result = match result {
            Err(ref e)
                if e.contains("página web en lugar del archivo")
                    && (url.contains("drive.google.com")
                        || url.contains("drive.usercontent.google.com")) =>
            {
                match Self::resolve_gdrive_download_url(client, url, true).await {
                    Ok(fresh) if fresh != url => {
                        downloaded_counter.store(0, Ordering::Relaxed);
                        // The size probed earlier described the warning page,
                        // not the file — a couple of kilobytes of HTML. Left in
                        // place it made the anti-truncation check below reject
                        // the retry's perfectly good download with "se
                        // esperaban 2841 bytes y se guardaron 4294967296",
                        // undoing this whole recovery path. Nothing measured
                        // the real file, so the honest value is "unknown".
                        total_size = 0;
                        Self::single_stream_download(
                            client,
                            &fresh,
                            dest_path,
                            Arc::clone(&downloaded_counter),
                        )
                        .await
                    }
                    _ => result,
                }
            }
            other => other,
        };

        done_flag.store(1, Ordering::Relaxed);

        if result.is_err() {
            let _ = tokio::fs::remove_file(dest_path).await;
            return result;
        }

        let downloaded = downloaded_counter.load(Ordering::Relaxed);
        if downloaded < 1024 {
            let _ = tokio::fs::remove_file(dest_path).await;
            return Err("El archivo descargado es demasiado pequeño; la descarga falló.".to_string());
        }

        let mut probe = tokio::fs::File::open(dest_path)
            .await
            .map_err(|e| e.to_string())?;
        let mut header = [0u8; 512];
        let n = probe.read(&mut header).await.map_err(|e| e.to_string())?;
        let prefix = String::from_utf8_lossy(&header[..n]).trim_start().to_lowercase();
        if prefix.starts_with("<!doctype")
            || prefix.starts_with("<html")
            || prefix.starts_with("<?xml")
        {
            let _ = tokio::fs::remove_file(dest_path).await;
            return Err(
                "La descarga devolvió una página web en lugar del archivo.".to_string(),
            );
        }

        // Neither the "too small" nor the HTML-sniff check above catch a
        // download that's mostly real bytes but got cut short partway
        // through (a dropped connection mid-stream, or one parallel range
        // segment coming back shorter than requested) — that silently saved
        // a truncated file while still reporting "completed" with the size
        // the UI showed at the start. Compare what actually landed on disk
        // against what the server told us up front and fail loudly instead.
        if total_size > 0 {
            let actual_size = probe.metadata().await.map_err(|e| e.to_string())?.len();
            if actual_size != total_size {
                drop(probe);
                let _ = tokio::fs::remove_file(dest_path).await;
                return Err(format!(
                    "La descarga se cortó a mitad de camino: se esperaban {} bytes y se guardaron {}. Probá de nuevo.",
                    total_size, actual_size
                ));
            }
        }

        Self::emit_download_progress(
            app_handle,
            progress_key,
            progress_field,
            event_name,
            downloaded,
            if total_size > 0 { total_size } else { downloaded },
            100,
            "completed",
        );

        Ok(())
    }

    /// Downloads and applies a HyperVisor bypass for Denuvo to the game folder
    pub async fn apply_hv_bypass(
        app_handle: Option<tauri::AppHandle>,
        game_path: &str,
        href: &str,
    ) -> Result<String, String> {
        let game_dir = Path::new(game_path);
        let is_rar = href.to_lowercase().ends_with(".rar") || href.contains(".rar");
        let archive_name = if is_rar { "hv_temp.rar" } else { "hv_temp.zip" };
        let temp_archive = Self::temp_dir().join(archive_name);

        fs::create_dir_all(Self::temp_dir()).map_err(|e| e.to_string())?;

        // Parse Buzzheavier URL or use direct download
        if href.contains("buzzheavier.com") {
            let client = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .build()
                .map_err(|e| e.to_string())?;

            let page_html = client.get(href).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
            let re_token = Regex::new(r#"hx-get="[^"]*?/download\?t=([^"&]+)"#).unwrap();
            let token = re_token.captures(&page_html)
                .and_then(|cap| cap.get(1))
                .map(|m| m.as_str().to_string())
                .ok_or_else(|| "Buzzheavier: No download token found on page".to_string())?;

            let file_id = href.split('/').last().ok_or_else(|| "Invalid Buzzheavier URL".to_string())?;
            let trigger_url = format!("https://buzzheavier.com/{}/download?t={}", file_id, token);

            let resp = client.get(&trigger_url)
                .header("HX-Request", "true")
                .header("Referer", href)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            // If redirected, download from the redirected URL
            let download_url = if resp.status().is_redirection() || resp.headers().contains_key("Hx-Redirect") {
                resp.headers().get("Hx-Redirect")
                    .or_else(|| resp.headers().get("location"))
                    .map(|v| v.to_str().unwrap_or("").to_string())
                    .unwrap_or(trigger_url)
            } else {
                trigger_url
            };

            Self::download_with_progress(app_handle.as_ref(), &client, &download_url, &temp_archive, href).await?;
        } else {
            // Standard direct download or Google Drive link
            let client = Self::build_download_client()?;

            let download_url = if href.contains("drive.google.com") {
                Self::resolve_gdrive_download_url(&client, href, false).await.unwrap_or_else(|_| href.to_string())
            } else {
                href.to_string()
            };

            Self::download_with_progress(app_handle.as_ref(), &client, &download_url, &temp_archive, href).await?;
        }

        // Extract the downloaded zip/rar to the game folder
        if temp_archive.exists() {
            let result = Self::extract_hv_bypass_archive(game_path, &temp_archive, is_rar);
            let _ = fs::remove_file(&temp_archive);
            result
        } else {
            Err("Failed to download bypass archive".to_string())
        }
    }

    /// Applies an HV bypass from an already-downloaded local archive instead
    /// of fetching one from a URL — same extraction/post-processing as
    /// apply_hv_bypass, just skipping the download step. The source archive
    /// is left untouched (unlike the download path's temp file, this one is
    /// a file the user picked themselves).
    pub fn apply_hv_bypass_local(game_path: &str, archive_path: &str) -> Result<String, String> {
        let archive = Path::new(archive_path);
        if !archive.exists() {
            return Err("El archivo seleccionado no existe.".to_string());
        }
        let is_rar = archive_path.to_lowercase().ends_with(".rar");
        Self::extract_hv_bypass_archive(game_path, archive, is_rar)
    }

    /// Shared extraction + post-processing for both apply_hv_bypass (from a
    /// download) and apply_hv_bypass_local (from a file the user already
    /// has): extracts the archive into the game folder, then recursively
    /// unpacks any nested rar/zip it contained (e.g. DenuvOwO_SRC_V4.rar)
    /// and flattens any "DenuvOwO" subfolder into the game root.
    ///
    /// Every entry is placed at `game_dir.join(entry_name)` — trusting the
    /// archive's own internal folder structure instead of trying to guess
    /// which subfolder of the game something belongs in (LuaTools' "Fix"
    /// button does the same: the person who packaged the archive already
    /// encoded the right relative paths, so a straight 1:1 extract onto the
    /// install root is all that's needed).
    ///
    /// A single locked/undecodable entry no longer fails the whole apply —
    /// each entry is best-effort and failures are just counted, mirroring
    /// uninstall_workshop_item's non-aborting cleanup. Only a fully failed
    /// primary extraction (nothing placed at all) is a hard error; partial
    /// failures are reported back in the success message instead.
    fn extract_hv_bypass_archive(game_path: &str, archive_path: &Path, is_rar: bool) -> Result<String, String> {
        let game_dir = Path::new(game_path);
        let mut failed: usize = 0;

        if is_rar {
            // External unrar call — all-or-nothing, so a failure here means
            // nothing was placed at all and a hard error is appropriate.
            Self::extract_rar_archive(archive_path.to_str().unwrap(), game_path)?;
        } else {
            let cursor = std::io::Cursor::new(fs::read(archive_path).map_err(|e| e.to_string())?);
            let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

            for i in 0..archive.len() {
                let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
                // Reject zip-slip entries (`..`/absolute paths) instead of
                // trusting the archive's raw entry name.
                let name = match file.enclosed_name() {
                    Some(p) => p.to_path_buf(),
                    None => continue,
                };
                let outpath = game_dir.join(&name);

                if file.name().ends_with('/') {
                    if fs::create_dir_all(&outpath).is_err() {
                        failed += 1;
                    }
                } else {
                    if let Some(p) = outpath.parent() {
                        let _ = fs::create_dir_all(p);
                    }
                    match fs::File::create(&outpath) {
                        Ok(mut outfile) => {
                            if std::io::copy(&mut file, &mut outfile).is_err() {
                                failed += 1;
                            }
                        }
                        Err(_) => failed += 1,
                    }
                }
            }
        }

        // Post-procesado inteligente:
        // 1. Buscar y extraer archivos rar o zip que se hayan extraído (ej. DenuvOwO_SRC_V4.rar).
        //    Un fallo acá ya no aborta todo — la extracción principal de arriba
        //    puede haber colocado ya lo importante, y un solo rar/zip anidado
        //    que no se pudo abrir no debería tirar el resultado entero.
        let scan_dirs = vec![game_dir.to_path_buf(), game_dir.join("DenuvOwO")];
        for s_dir in scan_dirs {
            if s_dir.exists() && s_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&s_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() {
                            let path_str = path.to_string_lossy().to_lowercase();
                            if path_str.ends_with(".rar") {
                                match Self::extract_rar_archive(path.to_str().unwrap(), game_path) {
                                    Ok(_) => { let _ = fs::remove_file(&path); }
                                    Err(_) => failed += 1,
                                }
                            } else if path_str.ends_with(".zip") {
                                let mut inner_failed: usize = 0;
                                let mut opened = false;
                                if let Ok(file_bytes) = fs::read(&path) {
                                    let cursor = std::io::Cursor::new(file_bytes);
                                    if let Ok(mut inner_zip) = zip::ZipArchive::new(cursor) {
                                        opened = true;
                                        for i in 0..inner_zip.len() {
                                            if let Ok(mut file) = inner_zip.by_index(i) {
                                                // Reject zip-slip entries instead of trusting the raw name.
                                                let name = match file.enclosed_name() {
                                                    Some(p) => p.to_path_buf(),
                                                    None => continue,
                                                };
                                                let outpath = game_dir.join(&name);
                                                if file.name().ends_with('/') {
                                                    let _ = fs::create_dir_all(&outpath);
                                                } else {
                                                    if let Some(p) = outpath.parent() {
                                                        let _ = fs::create_dir_all(p);
                                                    }
                                                    match fs::File::create(&outpath) {
                                                        Ok(mut outfile) => {
                                                            if std::io::copy(&mut file, &mut outfile).is_err() {
                                                                inner_failed += 1;
                                                            }
                                                        }
                                                        Err(_) => inner_failed += 1,
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                failed += inner_failed + if opened { 0 } else { 1 };
                                if opened && inner_failed == 0 {
                                    let _ = fs::remove_file(&path);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Mover todo lo que esté dentro de cualquier subcarpeta DenuvOwO (incluyendo anidadas) a la raíz del juego
        let mut denuvo_dirs = Vec::new();
        for entry in walkdir::WalkDir::new(game_dir).into_iter().flatten() {
            if entry.file_type().is_dir() {
                if entry.file_name().to_string_lossy().to_lowercase() == "denuvowo" {
                    denuvo_dirs.push(entry.path().to_path_buf());
                }
            }
        }

        // Ordenar por longitud de ruta descendente para procesar primero las más profundas
        denuvo_dirs.sort_by(|a, b| b.to_string_lossy().len().cmp(&a.to_string_lossy().len()));

        for d_dir in denuvo_dirs {
            let missed = Self::merge_directories(&d_dir, game_dir);
            failed += missed;
            // Only clear the source once everything in it actually landed.
            // Deleting it regardless is what turned one locked file into a
            // whole archive lost, with nothing left to retry from.
            if missed == 0 {
                let _ = fs::remove_dir_all(&d_dir);
            } else {
                diag_log!(
                    "[bypass] {} archivo(s) quedaron en {} — no se borra para poder reintentar.",
                    missed,
                    d_dir.display()
                );
            }
        }

        if failed > 0 {
            Ok(format!(
                "Bypass aplicado, pero {} archivo(s) no se pudieron escribir (¿el juego está abierto o los archivos están protegidos?). Si algo no funciona, cierra el juego por completo y reintenta.",
                failed
            ))
        } else {
            Ok("HyperVisor bypass applied successfully!".to_string())
        }
    }

    /// Files under `dir`, at any depth. Used to report how much a failed merge
    /// left behind.
    fn count_files(dir: &Path) -> usize {
        let Ok(entries) = fs::read_dir(dir) else { return 0 };
        entries
            .flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() { Self::count_files(&p) } else { 1 }
            })
            .sum()
    }

    /// Moves everything from `src` into `dest`, overwriting. Returns how many
    /// files it could NOT move.
    ///
    /// Two things were wrong with the previous version, and together they
    /// destroyed data. It returned `io::Result<()>` and used `?` on every
    /// entry, so the first failure abandoned the rest of the tree — and the
    /// caller then ran `remove_dir_all` on the source whatever happened. One
    /// locked file at position three of twenty meant the other seventeen were
    /// deleted having never been written anywhere, and because the caller
    /// counted failed *directories* rather than files, the user was told "1
    /// file could not be written".
    ///
    /// It also moved with `fs::rename` alone, which fails across volumes. The
    /// staging folder lives in %TEMP% on C: while games usually sit on another
    /// drive, so on those machines every single file failed.
    fn merge_directories(src: &Path, dest: &Path) -> usize {
        if !dest.exists() && fs::create_dir_all(dest).is_err() {
            return Self::count_files(src);
        }
        let Ok(entries) = fs::read_dir(src) else {
            return Self::count_files(src);
        };

        let mut failed = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            let dest_path = dest.join(entry.file_name());

            if path.is_dir() {
                failed += Self::merge_directories(&path, &dest_path);
                continue;
            }
            if dest_path.exists() {
                let _ = fs::remove_file(&dest_path);
            }
            // Rename first — it is atomic and free on the same volume — then
            // fall back to a copy when the two are on different drives.
            if fs::rename(&path, &dest_path).is_err() && fs::copy(&path, &dest_path).is_err() {
                diag_log!("[bypass] No se pudo escribir {}", dest_path.display());
                failed += 1;
            }
        }
        failed
    }

    /// Extracts a local ZIP/RAR archive to a destination directory
    pub fn extract_local_archive(archive_path: &str, dest_dir: &str) -> Result<String, String> {
        let dest = Path::new(dest_dir);
        let arch = Path::new(archive_path);
        if !arch.exists() {
            return Err("El archivo de origen no existe.".to_string());
        }
        if !dest.exists() {
            fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        }

        if archive_path.to_lowercase().ends_with(".zip") {
            let file = fs::File::open(arch).map_err(|e| e.to_string())?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
                // Reject zip-slip entries (`..`/absolute paths) — this archive
                // can come from an arbitrary third-party download (online-fix
                // mirrors, Buzzheavier, etc.), so the entry name is untrusted.
                let name = match entry.enclosed_name() {
                    Some(p) => p.to_path_buf(),
                    None => continue,
                };
                let outpath = dest.join(&name);
                if entry.name().ends_with('/') {
                    fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
                } else {
                    if let Some(p) = outpath.parent() {
                        if !p.exists() {
                            fs::create_dir_all(p).map_err(|e| e.to_string())?;
                        }
                    }
                    let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut outfile).map_err(|e| e.to_string())?;
                }
            }
            Ok("Archivo ZIP extraído e instalado correctamente!".to_string())
        } else if archive_path.to_lowercase().ends_with(".rar") {
            Self::extract_rar_archive(archive_path, dest_dir)
                .map(|_| "Archivo RAR extraído e instalado correctamente!".to_string())
        } else {
            Err("Formato no soportado directamente. Por favor, selecciona un archivo .zip o .rar".to_string())
        }
    }

    /// Helper to extract RAR archive using UnRAR.exe, 7-Zip, or bsdtar (tar)
    fn extract_rar_archive(archive_path: &str, dest_dir: &str) -> Result<(), String> {
        #[cfg(windows)]
        {
            let unrar_path = Path::new("C:\\Program Files\\WinRAR\\UnRAR.exe");
            if unrar_path.exists() {
                let dest_with_slash = format!("{}\\", dest_dir.trim_end_matches('\\'));
                let output = Command::new(unrar_path)
                    .args([
                        "x",
                        "-y",
                        archive_path,
                        &dest_with_slash,
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .map_err(|e| format!("Failed to run UnRAR.exe: {}", e))?;
                
                if output.status.success() {
                    return Ok(());
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("UnRAR failed: {}", err));
                }
            }

            let seven_zip_path = Path::new("C:\\Program Files\\7-Zip\\7z.exe");
            if seven_zip_path.exists() {
                let output = Command::new(seven_zip_path)
                    .args([
                        "x",
                        "-y",
                        &format!("-o{}", dest_dir),
                        archive_path,
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .map_err(|e| format!("Failed to run 7z.exe: {}", e))?;
                
                if output.status.success() {
                    return Ok(());
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("7z failed: {}", err));
                }
            }

            let output = Command::new("tar")
                .args([
                    "-xf",
                    archive_path,
                    "-C",
                    dest_dir,
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map_err(|e| format!("Failed to run tar: {}", e))?;
            
            if output.status.success() {
                Ok(())
            } else {
                let err = String::from_utf8_lossy(&output.stderr);
                Err(format!("tar failed: {}", err))
            }
        }
        #[cfg(not(windows))]
        {
            Err("Extracción de RAR solo es soportada en Windows".to_string())
        }
    }

    /// Extracts depot decryption keys from config.vdf
    pub fn extract_depot_keys(config_vdf_path: &str) -> Result<HashMap<String, String>, String> {
        let path = Path::new(config_vdf_path);
        if !path.exists() {
            return Err("El archivo config.vdf no existe en la ruta especificada.".to_string());
        }

        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| e.to_string())?;

        let mut keys = HashMap::new();
        // Regex to match depot ID and decryption key block
        let re = Regex::new(r#""(\d+)"\s*\{\s*(?s:[^}]+?)\s*"DecryptionKey"\s*"([0-9a-fA-F]{32})""#).unwrap();

        for cap in re.captures_iter(&content) {
            let depot_id = cap.get(1).map(|m| m.as_str().to_string());
            let key = cap.get(2).map(|m| m.as_str().to_string());
            if let (Some(d), Some(k)) = (depot_id, key) {
                keys.insert(d, k);
            }
        }

        Ok(keys)
    }

    /// Generates Goldberg Emulator configuration files
    pub async fn generate_goldberg_config(
        steam_api_key: &str,
        app_id: &str,
        settings_dir: &str,
    ) -> Result<String, String> {
        let output_path = Path::new(settings_dir);
        fs::create_dir_all(output_path).map_err(|e| e.to_string())?;

        // 1. Write steam_appid.txt
        let appid_file = output_path.join("steam_appid.txt");
        fs::write(&appid_file, app_id).map_err(|e| e.to_string())?;

        // 2. Fetch DLCs (reusing steam API or public store details API)
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0")
            .build()
            .map_err(|e| e.to_string())?;

        let details_url = format!(
            "https://store.steampowered.com/api/appdetails?appids={}&l=english",
            app_id
        );

        let mut dlc_lines = Vec::new();
        if let Ok(res) = client.get(&details_url).send().await {
            if let Ok(json) = res.json::<serde_json::Value>().await {
                if json[app_id]["success"].as_bool().unwrap_or(false) {
                    if let Some(dlcs) = json[app_id]["data"]["dlc"].as_array() {
                        for dlc in dlcs {
                            if let Some(dlc_id) = dlc.as_u64() {
                                dlc_lines.push(format!("{}=", dlc_id));
                            }
                        }
                    }
                }
            }
        }

        // Write DLC.txt
        if !dlc_lines.is_empty() {
            let dlc_file = output_path.join("DLC.txt");
            fs::write(&dlc_file, dlc_lines.join("\n")).map_err(|e| e.to_string())?;
        }

        // 3. Fetch achievements schema from Steam Web API
        if !steam_api_key.is_empty() {
            let schema_url = format!(
                "https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?key={}&appid={}",
                steam_api_key, app_id
            );

            if let Ok(res) = client.get(&schema_url).send().await {
                if let Ok(json) = res.json::<serde_json::Value>().await {
                    if let Some(achievements_arr) = json["game"]["availableGameStats"]["achievements"].as_array() {
                        let mut gbe_achievements = Vec::new();
                        for ach in achievements_arr {
                            if let Some(name) = ach["name"].as_str() {
                                let display_name = ach["displayName"].as_str().unwrap_or(name);
                                let description = ach["description"].as_str().unwrap_or("");
                                let hidden = ach["hidden"].as_i64().unwrap_or(0) == 1;
                                gbe_achievements.push(serde_json::json!({
                                    "name": name,
                                    "displayName": display_name,
                                    "description": description,
                                    "hidden": hidden
                                }));
                            }
                        }
                        if !gbe_achievements.is_empty() {
                            let ach_file = output_path.join("achievements.json");
                            if let Ok(ach_data) = serde_json::to_string_pretty(&gbe_achievements) {
                                let _ = fs::write(&ach_file, ach_data);
                            }
                        }
                    }
                }
            }
        }

        // 4. Write steam_interfaces.txt
        let interfaces_file = output_path.join("steam_interfaces.txt");
        let default_interfaces = vec![
            "SteamClient020",
            "SteamUser021",
            "SteamFriends017",
            "SteamUtils010",
            "SteamMatchmaking009",
            "SteamMatchmakingServers002",
            "SteamUserStats012",
            "SteamApps008",
            "SteamNetworking006",
            "SteamRemoteStorage016",
            "SteamScreenshots003",
            "SteamHTTP003",
            "SteamController008",
            "SteamUGC016",
            "SteamAppList001",
            "SteamMusic001",
            "SteamMusicRemote001",
            "SteamHTMLSurface005",
            "SteamInventory003",
            "SteamVideo002",
            "SteamParentalSettings001",
            "SteamInput006",
        ];
        fs::write(&interfaces_file, default_interfaces.join("\n")).map_err(|e| e.to_string())?;

        // 5. Generate steam_token.txt
        let token_file = output_path.join("steam_token.txt");
        fs::write(&token_file, "ragnarok_token").map_err(|e| e.to_string())?;

        Ok("Configuración de Goldberg generada con éxito!".to_string())
    }

    fn get_url_query_param_internal(url: &str, param: &str) -> Option<String> {
        let query = url.split('?').nth(1)?;
        for part in query.split('&') {
            let mut key_val = part.splitn(2, '=');
            let key = key_val.next()?;
            if key == param {
                let val = key_val.next()?;
                return Some(urlencoding::decode(val).ok()?.into_owned());
            }
        }
        None
    }

    /// Resolves a MediaFire file page URL to a direct CDN download link.
    pub async fn resolve_mediafire_download_url(client: &reqwest::Client, url: &str) -> Result<String, String> {
        if !url.contains("mediafire.com") {
            return Ok(url.to_string());
        }
        if url.contains("download") && url.contains(".mediafire.com/") && !url.contains("/file/") {
            return Ok(url.to_string());
        }

        static CDN_RE: OnceLock<Regex> = OnceLock::new();
        let cdn_re = CDN_RE.get_or_init(|| {
            Regex::new(r#"href="(https://download\d+\.mediafire\.com/[^"]+)""#).unwrap()
        });

        let resp = client.get(url).send().await
            .map_err(|e| format!("Error al acceder a MediaFire: {}", e))?;

        let mut accumulated = Vec::with_capacity(96 * 1024);
        let mut stream = resp.bytes_stream();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| format!("Error leyendo MediaFire: {}", e))?;
            accumulated.extend_from_slice(&chunk);

            // CDN link appears ~65 KB into the page — stop early once found
            if accumulated.len() >= 48 * 1024 {
                let html = String::from_utf8_lossy(&accumulated);
                if let Some(cap) = cdn_re.captures(&html) {
                    return Ok(cap[1].replace("&amp;", "&"));
                }
            }
            if accumulated.len() > 384 * 1024 {
                break;
            }
        }

        let html = String::from_utf8_lossy(&accumulated);
        static ALT_RES: OnceLock<[Regex; 2]> = OnceLock::new();
        let patterns = ALT_RES.get_or_init(|| {
            [
                Regex::new(r#"href="(https://download\d+\.mediafire\.com/[^"]+)""#).unwrap(),
                Regex::new(r#"aria-label="Download file"[^>]*href="([^"]+)""#).unwrap(),
            ]
        });
        for re in patterns.iter() {
            if let Some(cap) = re.captures(&html) {
                let direct = cap[1].replace("&amp;", "&");
                if direct.starts_with("http") {
                    return Ok(direct);
                }
            }
        }

        Err("No se pudo obtener enlace directo de MediaFire".to_string())
    }

    fn extract_gdrive_file_id(url: &str) -> Option<String> {
        Self::get_url_query_param_internal(url, "id").or_else(|| {
            if url.contains("/file/d/") {
                url.split("/file/d/")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
    }

    /// Resolves a Google Drive share/download URL to a direct file download
    /// link. With `force_probe = false`, guesses the fast-path
    /// `confirm=t` link straight from the file id — correct for most
    /// files, but Google rejects that guessed token for some (large files,
    /// or ones getting heavy traffic) and serves an HTML warning/quota page
    /// instead. `force_probe = true` skips the guess and actually fetches
    /// that page to read Google's real confirm token (and reports the
    /// specific reason — quota exceeded, file private/removed — when one
    /// applies) instead of repeating the same guess and failing the same
    /// way again. Callers should retry with `force_probe = true` after a
    /// fast-path download fails rather than blindly re-trying it.
    pub async fn resolve_gdrive_download_url(client: &reqwest::Client, url: &str, force_probe: bool) -> Result<String, String> {
        if !url.contains("drive.google.com") && !url.contains("drive.usercontent.google.com") {
            return Ok(url.to_string());
        }
        if !force_probe && url.contains("drive.usercontent.google.com") && url.contains("confirm=") {
            return Ok(url.to_string());
        }

        let id = Self::extract_gdrive_file_id(url);

        if !force_probe {
            if let Some(id) = &id {
                // Fast path: skip the virus-scan HTML round-trip for public files
                return Ok(format!(
                    "https://drive.usercontent.google.com/download?id={}&export=download&confirm=t",
                    id
                ));
            }
        }

        let probe_url = match &id {
            Some(id) => format!("https://drive.usercontent.google.com/download?id={}&export=download", id),
            None => url.to_string(),
        };

        let res = client.get(&probe_url).send().await
            .map_err(|e| format!("Error en petición inicial a Google Drive: {}", e))?;

        let content_type = res.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();

        if !content_type.contains("text/html") {
            return Ok(probe_url);
        }

        let body = res.text().await
            .map_err(|e| format!("Error leyendo HTML de Google Drive: {}", e))?;

        if body.contains("Too many users") || body.contains("demasiados usuarios") || body.contains("Demasiados usuarios") {
            return Err("Google Drive alcanzó el límite de descargas para este archivo por hoy — probá de nuevo más tarde.".to_string());
        }
        if body.contains("Request access") || body.contains("You need permission") || body.contains("no tienes permiso") {
            return Err("El archivo de Google Drive es privado, fue eliminado o ya no está disponible.".to_string());
        }

        let href_re = Regex::new(r#"href="(https://drive\.usercontent\.google\.com/download[^"]+)""#)
            .map_err(|e| e.to_string())?;
        if let Some(cap) = href_re.captures(&body) {
            return Ok(cap[1].replace("&amp;", "&"));
        }

        // Google's actual warning-page form carries the real confirm token
        // (and, sometimes, a uuid) as hidden inputs rather than a plain link.
        if let Some(id) = &id {
            let confirm_re = Regex::new(r#"name="confirm"\s+value="([^"]+)""#).map_err(|e| e.to_string())?;
            if let Some(confirm_cap) = confirm_re.captures(&body) {
                let confirm = &confirm_cap[1];
                let uuid_part = Regex::new(r#"name="uuid"\s+value="([^"]+)""#)
                    .ok()
                    .and_then(|re| re.captures(&body).map(|c| format!("&uuid={}", &c[1])))
                    .unwrap_or_default();
                return Ok(format!(
                    "https://drive.usercontent.google.com/download?id={}&export=download&confirm={}{}",
                    id, confirm, uuid_part
                ));
            }
        }

        Err("No se pudo extraer el enlace directo de Google Drive (la página de confirmación tiene un formato inesperado).".to_string())
    }

    /// Resolves bypass download URLs (MediaFire pages, Google Drive warning pages) to direct links.
    pub async fn resolve_bypass_download_url(client: &reqwest::Client, url: &str, force_probe: bool) -> Result<String, String> {
        if url.contains("mediafire.com") {
            Self::resolve_mediafire_download_url(client, url).await
        } else if url.contains("drive.google.com") {
            Self::resolve_gdrive_download_url(client, url, force_probe).await
        } else {
            Ok(url.to_string())
        }
    }

    /// Downloads the latest Goldberg Emulator (gbe_fork) release DLLs for Windows
    pub async fn download_goldberg_emulator() -> Result<PathBuf, String> {
        let dest = Self::temp_dir().join("goldberg_emu");
        if dest.exists() && dest.join("steam_api.dll").exists() {
            return Ok(dest);
        }
        let _ = fs::remove_dir_all(&dest);

        let client = reqwest::Client::builder()
            .user_agent("Ragnarok-Launcher-App")
            .build()
            .map_err(|e| e.to_string())?;

        let download_url = Self::get_goldberg_download_url(&client).await?;
        if !download_guard::is_allowed_url(&download_url, GITHUB_RELEASE_HOSTS) {
            return Err(format!("URL de descarga no confiable: {}", download_url));
        }

        let bytes = client.get(&download_url).send().await
            .map_err(|e| format!("Error de descarga: {}", e))?
            .bytes().await
            .map_err(|e| format!("Error al leer bytes: {}", e))?;

        download_guard::verify_download(
            "Goldberg (gbe_fork)",
            &bytes,
            download_guard::Payload::SevenZ,
            None,
        )?;

        let temp_7z = Self::temp_dir().join("goldberg_temp.7z");
        fs::create_dir_all(Self::temp_dir()).map_err(|e| e.to_string())?;
        fs::write(&temp_7z, &bytes).map_err(|e| e.to_string())?;

        // Extract .7z using sevenz-rust (pure Rust, no 7-Zip required)
        let extracted = dest.join("extracted");
        fs::create_dir_all(&extracted).map_err(|e| e.to_string())?;

        sevenz_rust::decompress_file(&temp_7z, &extracted)
            .map_err(|e| format!("Error extrayendo el archivo 7z: {}", e))?;

        let _ = fs::remove_file(&temp_7z);

        // Find steam_api.dll within the extracted folder structure
        let dll_dir = Self::find_goldberg_dll_dir(&extracted)?;

        // Copy files to the final destination
        fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

        for entry in fs::read_dir(&dll_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file() {
                let name = entry.file_name();
                let dest_path = dest.join(&name);
                let _ = fs::copy(&path, &dest_path);
            }
        }

        let _ = fs::remove_dir_all(&extracted);

        Ok(dest)
    }

    /// Gets the download URL for Goldberg emulator, with API fallback
    async fn get_goldberg_download_url(client: &reqwest::Client) -> Result<String, String> {
        let api_url = "https://api.github.com/repos/Detanup01/gbe_fork/releases/latest";

        match client.get(api_url).send().await {
            Ok(res) => {
                if !res.status().is_success() {
                    // API returned an error (e.g., rate limited) — use fallback
                    return Ok(Self::goldberg_fallback_url());
                }
                match res.json::<serde_json::Value>().await {
                    Ok(json) => {
                        // Check if the response is an error (rate limiting, etc.)
                        if json.get("message").is_some() {
                            return Ok(Self::goldberg_fallback_url());
                        }
                        // Try to find emu-win-release.7z, then emu-win-release-vs22.7z, then any win .7z
                        if let Some(assets) = json["assets"].as_array() {
                            // Preferred asset names in order
                            let preferred = ["emu-win-release.7z", "emu-win-release-vs22.7z"];
                            for preferred_name in &preferred {
                                if let Some(url) = assets.iter().find_map(|a| {
                                    let name = a["name"].as_str().unwrap_or("");
                                    if name == *preferred_name {
                                        a["browser_download_url"].as_str().map(|s| s.to_string())
                                    } else {
                                        None
                                    }
                                }) {
                                    return Ok(url);
                                }
                            }
                            // Fallback: find any Windows emu .7z file
                            if let Some(url) = assets.iter().find_map(|a| {
                                let name = a["name"].as_str().unwrap_or("");
                                if name.starts_with("emu-win-") && name.ends_with(".7z") {
                                    a["browser_download_url"].as_str().map(|s| s.to_string())
                                } else {
                                    None
                                }
                            }) {
                                return Ok(url);
                            }
                        }
                        // No suitable asset found — use fallback
                        Ok(Self::goldberg_fallback_url())
                    }
                    Err(_) => Ok(Self::goldberg_fallback_url()),
                }
            }
            Err(_) => Ok(Self::goldberg_fallback_url()),
        }
    }

    /// Hardcoded fallback URL for the latest known gbe_fork release
    fn goldberg_fallback_url() -> String {
        "https://github.com/Detanup01/gbe_fork/releases/download/release-2026_05_30/emu-win-release.7z".to_string()
    }

    /// Locates the directory containing steam_api.dll within the extracted 7z folder
    fn find_goldberg_dll_dir(extracted: &Path) -> Result<PathBuf, String> {
        // Check root first
        if extracted.join("steam_api.dll").exists() {
            return Ok(extracted.to_path_buf());
        }

        // Check immediate subdirectories
        if let Ok(entries) = fs::read_dir(extracted) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("steam_api.dll").exists() {
                    return Ok(path);
                }
            }
        }

        // Recursive search as last resort
        for entry in walkdir::WalkDir::new(extracted).into_iter().flatten() {
            if entry.file_name() == "steam_api.dll" || entry.file_name() == "steam_api64.dll" {
                if let Some(parent) = entry.path().parent() {
                    return Ok(parent.to_path_buf());
                }
            }
        }

        Err("No se encontraron los archivos steam_api.dll en el paquete extraído.".to_string())
    }

    /// Scans a game folder and returns the most likely game executable path
    pub fn detect_game_exe(game_folder: &str) -> Result<String, String> {
        let dir = Path::new(game_folder);
        if !dir.exists() {
            return Err("La carpeta del juego no existe.".to_string());
        }

        // Known non-game executables to exclude
        let excluded: &[&str] = &[
            "unitycrashhandler", "crashhandler", "launcher", "unins",
            "setup", "install", "redist", "dotnet", "vc_redist",
            "dxsetup", "oalinst", "vcredist",
        ];

        let mut exes: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext.to_string_lossy().to_lowercase() == "exe" {
                        let name = path.file_stem()
                            .map(|n| n.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        let should_exclude = excluded.iter().any(|&e| name.contains(e));
                        if !should_exclude {
                            exes.push(path);
                        }
                    }
                }
            }
        }

        if exes.is_empty() {
            return Err("No se encontraron archivos .exe en la carpeta del juego.".to_string());
        }

        // Prefer the largest exe (usually the main game executable, not a launcher)
        exes.sort_by(|a, b| {
            let a_size = fs::metadata(a).map(|m| m.len()).unwrap_or(0);
            let b_size = fs::metadata(b).map(|m| m.len()).unwrap_or(0);
            b_size.cmp(&a_size)
        });

        Ok(exes[0].to_string_lossy().to_string())
    }

    /// Name fragments (case-insensitive substring match) that identify a
    /// bundled prerequisite installer rather than the game itself — VC++/
    /// DirectX/.NET redistributables, or per-game middleware some crossplay
    /// titles need (EOS/EasyAntiCheat, the PlayStation PC SDK). Scene
    /// repacks (DODI, FitGirl, etc.) almost always bundle these already.
    const PREREQUISITE_NAME_HINTS: &[&str] = &[
        "vc_redist", "vcredist", "dxsetup", "directx", "oalinst",
        "dotnetfx", "dotnet-runtime", "dotnet_", "eos_eac_install",
        "pspcsdkruntime", "xnafx", "physx",
    ];

    /// Folder names repacks conventionally drop prerequisite installers
    /// into — anything in one of these counts even without matching a name
    /// hint above, since repack groups don't all use the same filenames.
    const PREREQUISITE_FOLDER_HINTS: &[&str] = &[
        "_commonredist", "commonredist", "redist", "prerequisites", "prereq",
    ];

    /// Walks a game folder (bounded depth — these installers are never
    /// buried deep) looking for bundled prerequisite installers, so the
    /// user doesn't have to hunt through subfolders by hand after hitting a
    /// "runtime is missing" error the repack's own instructions already
    /// told them how to fix manually.
    pub fn find_prerequisite_installers(game_folder: &str) -> Vec<PathBuf> {
        let root = Path::new(game_folder);
        let mut found = Vec::new();
        for entry in walkdir::WalkDir::new(root).max_depth(4).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            if ext != "exe" && ext != "bat" {
                continue;
            }
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            let in_prereq_folder = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| {
                    let nl = n.to_lowercase();
                    Self::PREREQUISITE_FOLDER_HINTS.iter().any(|h| nl.contains(h))
                })
                .unwrap_or(false);
            let name_matches = Self::PREREQUISITE_NAME_HINTS.iter().any(|h| name.contains(h));
            if in_prereq_folder || name_matches {
                found.push(path.to_path_buf());
            }
        }
        found.sort();
        found.dedup();
        found
    }

    /// Runs every detected prerequisite installer in sequence — not
    /// parallel, since some (VC++ redist chains in particular) conflict
    /// with each other if launched at the same time — waiting for each to
    /// exit before starting the next. Shown to the user, not hidden: unlike
    /// steamcmd/gmad these can have a real UI (EULA, progress bar) that may
    /// need a click. Best-effort per installer, mirroring the resilient
    /// pattern used elsewhere (Workshop uninstall, HV bypass apply) — one
    /// failing or being cancelled doesn't stop the rest from running.
    pub fn run_prerequisite_installers(app_handle: &tauri::AppHandle, installers: &[PathBuf]) -> (usize, usize) {
        use tauri::Manager;
        let mut ok = 0usize;
        let mut failed = 0usize;
        for installer in installers {
            let display_name = installer.file_name().unwrap_or_default().to_string_lossy().to_string();
            let _ = app_handle.emit_all("repair_log", format!("⟳ Ejecutando {}...", display_name));
            let result = std::process::Command::new(installer)
                .current_dir(installer.parent().unwrap_or(Path::new(".")))
                .status();
            match result {
                Ok(status) if status.success() => {
                    ok += 1;
                    let _ = app_handle.emit_all("repair_log", format!("✔ {} terminó OK", display_name));
                }
                Ok(status) => {
                    failed += 1;
                    let _ = app_handle.emit_all(
                        "repair_log",
                        format!("⚠ {} salió con código {} (puede ser normal si cerraste un diálogo)", display_name, status),
                    );
                }
                Err(e) => {
                    failed += 1;
                    let _ = app_handle.emit_all("repair_log", format!("✗ No se pudo ejecutar {}: {}", display_name, e));
                }
            }
        }
        (ok, failed)
    }

    /// Detects if a PE executable is 64-bit by reading its machine header
    #[allow(dead_code)]
    fn is_64bit_exe(exe_path: &str) -> Result<bool, String> {
        let bytes = fs::read(exe_path).map_err(|e| format!("Error al leer el ejecutable: {}", e))?;
        if bytes.len() < 64 || &bytes[0..2] != b"MZ" {
            return Err("El archivo no es un ejecutable PE válido.".to_string());
        }
        // Get PE signature offset at 0x3C
        let pe_offset = u32::from_le_bytes([bytes[0x3C], bytes[0x3D], bytes[0x3E], bytes[0x3F]]) as usize;
        if pe_offset + 4 >= bytes.len() || &bytes[pe_offset..pe_offset+4] != b"PE\x00\x00" {
            return Err("Firma PE no encontrada.".to_string());
        }
        let machine = u16::from_le_bytes([bytes[pe_offset+4], bytes[pe_offset+5]]);
        match machine {
            0x8664 => Ok(true),
            0x14C => Ok(false),
            _ => Err(format!("Arquitectura de ejecutable no soportada: 0x{:04X}", machine)),
        }
    }

    /// Downloads the Steam API Check Bypass DLLs
    pub async fn download_steam_api_bypass() -> Result<PathBuf, String> {
        let dest = Self::temp_dir().join("steam_api_bypass");
        if dest.exists() {
            return Ok(dest);
        }

        fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

        // Download from the raw GitHub repo (small DLLs ~300KB each)
        let client = reqwest::Client::builder()
            .user_agent("Ragnarok-Launcher-App")
            .build()
            .map_err(|e| e.to_string())?;

        let base = "https://raw.githubusercontent.com/SteamAutoCracks/Steam-API-Check-Bypass/master/Release_dlls";
        let files = ["SteamAPICheckBypass.dll", "SteamAPICheckBypass_x32.dll"];

        for file in &files {
            let url = format!("{}/{}", base, file);
            let bytes = client.get(&url).send().await
                .map_err(|e| format!("Error descargando {}: {}", file, e))?
                .bytes().await
                .map_err(|e| format!("Error leyendo {}: {}", file, e))?;
            fs::write(dest.join(file), &bytes).map_err(|e| e.to_string())?;
        }

        Ok(dest)
    }

    /// Finds all steam_api.dll or steam_api64.dll recursively in a folder
    fn find_steam_dlls_recursive(dir: &Path) -> Vec<PathBuf> {
        let mut results = Vec::new();
        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if (name == "steam_api.dll" || name == "steam_api64.dll") && !name.contains("_o") && !name.contains(".bak") {
                results.push(entry.path().to_path_buf());
            }
        }
        results
    }

    /// Full automatic crack pipeline:
    /// 1. Detect game executable
    /// 2. Backup ALL steam_api DLLs found recursively (rename to .bak)
    /// 3. Run Steamless to unpack SteamStub
    /// 4. Apply Goldberg Emulator per-directory (DLL + steam_settings/ + config)
    /// 5. Apply Steam API Check Bypass per-directory
    pub async fn auto_crack(
        game_folder: String,
        app_id: String,
        steam_api_key: String,
    ) -> Result<String, String> {
        let game_dir = Path::new(&game_folder);
        if !game_dir.exists() {
            return Err("La carpeta del juego no existe.".to_string());
        }

        let mut log = String::new();
        log.push_str("=== Iniciando Auto Crack ===\n");
        let mut errors = 0usize;

        // Step 1: Detect game executable
        log.push_str("[1/5] Detectando ejecutable del juego...\n");
        let exe_path = Self::detect_game_exe(&game_folder)?;
        log.push_str(&format!("  Ejecutable: {}\n", exe_path));

        // Step 2: Find ALL steam_api.dll / steam_api64.dll recursively and backup
        log.push_str("[2/5] Buscando y respaldando DLLs de Steam...\n");
        let dlls = Self::find_steam_dlls_recursive(game_dir);
        if dlls.is_empty() {
            log.push_str("  No se encontraron steam_api.dll o steam_api64.dll en el juego.\n");
        }
        let mut backed_up = 0usize;
        for dll_path in &dlls {
            let bak_path = dll_path.with_extension("dll.bak");
            if !bak_path.exists() {
                match fs::rename(dll_path, &bak_path) {
                    Ok(_) => {
                        log.push_str(&format!("  Respaldado: {} -> .bak\n", dll_path.file_name().unwrap_or_default().to_string_lossy()));
                        backed_up += 1;
                    }
                    Err(e) => {
                        log.push_str(&format!("  Error respaldando {}: {}\n", dll_path.display(), e));
                        errors += 1;
                    }
                }
            } else {
                log.push_str(&format!("  Backup ya existe: {}\n", bak_path.file_name().unwrap_or_default().to_string_lossy()));
            }
        }
        log.push_str(&format!("  {} DLL(s) respaldada(s).\n", backed_up));

        // Step 3: Run Steamless to unpack exe
        log.push_str("[3/5] Ejecutando Steamless (desempaquetando SteamStub)...\n");
        match Self::run_steamless(&exe_path).await {
            Ok(msg) => {
                log.push_str(&format!("  {}\n", msg));
            }
            Err(e) => {
                log.push_str(&format!("  Advertencia: {} (continuando de todas formas)\n", e));
            }
        }

        // Step 4: Download Goldberg Emulator
        log.push_str("[4/5] Aplicando Goldberg Emulator...\n");
        let goldberg_dir = Self::download_goldberg_emulator().await?;

        // For each found DLL directory, copy Goldberg DLL + steam_settings/
        for dll_path in &dlls {
            let parent = dll_path.parent().unwrap_or(game_dir);

            // Copy Goldberg DLL
            let goldberg_dll_name = dll_path.file_name().unwrap().to_string_lossy();
            let goldberg_src = goldberg_dir.join(&*goldberg_dll_name);
            if goldberg_src.exists() {
                match fs::copy(&goldberg_src, dll_path) {
                    Ok(_) => log.push_str(&format!("  Goldberg {} -> {}\n", goldberg_dll_name, parent.display())),
                    Err(e) => {
                        log.push_str(&format!("  Error copiando Goldberg {}: {}\n", goldberg_dll_name, e));
                        errors += 1;
                    }
                }
            } else {
                log.push_str(&format!("  Goldberg {} no encontrado en el paquete.\n", goldberg_dll_name));
            }

            // Copy steam_settings/ to parent directory
            let settings_dir = parent.join("steam_settings");
            if !settings_dir.exists() {
                log.push_str(&format!("  Generando steam_settings/ en {}\n", parent.display()));
                match Self::generate_goldberg_config(&steam_api_key, &app_id, settings_dir.to_str().unwrap()).await {
                    Ok(msg) => log.push_str(&format!("    {}\n", msg)),
                    Err(e) => {
                        log.push_str(&format!("    Error en configuración: {}\n", e));
                        errors += 1;
                    }
                }
            } else {
                log.push_str(&format!("  steam_settings/ ya existe en {}\n", parent.display()));
            }
        }

        // Also copy Goldberg files (non-DLL) to game root for convenience
        if let Ok(entries) = fs::read_dir(&goldberg_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.ends_with(".txt") || name.ends_with(".ini") {
                        let dest = game_dir.join(entry.file_name());
                        if !dest.exists() {
                            let _ = fs::copy(&path, &dest);
                        }
                    }
                }
            }
        }

        // Step 5: Apply Steam API Check Bypass per-DLL-directory
        log.push_str("[5/5] Aplicando Steam API Check Bypass...\n");
        match Self::download_steam_api_bypass().await {
            Ok(bypass_dir) => {
                for dll_path in &dlls {
                    let parent = dll_path.parent().unwrap_or(game_dir);
                    let is_64bit = dll_path.file_name().map(|n| n.to_string_lossy().contains("64")).unwrap_or(false);

                    // Copy the appropriate bypass DLL as version.dll in the DLL's directory
                    let src_name = if is_64bit { "SteamAPICheckBypass.dll" } else { "SteamAPICheckBypass_x32.dll" };
                    let bypass_src = bypass_dir.join(src_name);
                    let dest_dll = parent.join("version.dll");

                    if bypass_src.exists() {
                        // Backup existing version.dll
                        let version_bak = parent.join("version.dll.bak");
                        if dest_dll.exists() && !version_bak.exists() {
                            let _ = fs::rename(&dest_dll, &version_bak);
                        }
                        match fs::copy(&bypass_src, &dest_dll) {
                            Ok(_) => log.push_str(&format!("  Bypass {} -> version.dll en {}\n", src_name, parent.display())),
                            Err(e) => {
                                log.push_str(&format!("  Error copiando bypass: {}\n", e));
                                errors += 1;
                            }
                        }

                        // Generate config pointing to .bak files
                        let config = serde_json::json!({
                            "steam_api64.dll": {
                                "mode": "file_redirect",
                                "to": "steam_api64.dll.bak",
                                "hook_times_mode": "all"
                            },
                            "steam_api.dll": {
                                "mode": "file_redirect",
                                "to": "steam_api.dll.bak",
                                "hook_times_mode": "all"
                            }
                        });
                        let config_path = parent.join("SteamAPICheckBypass.json");
                        if let Ok(json) = serde_json::to_string_pretty(&config) {
                            let _ = fs::write(&config_path, json);
                        }
                    }
                }
            }
            Err(e) => {
                log.push_str(&format!("  Advertencia: {} (continuando)\n", e));
            }
        }

        if errors > 0 {
            log.push_str(&format!("=== Auto Crack completado con {} error(es) ===\n", errors));
        } else {
            log.push_str("=== Auto Crack completado exitosamente ===\n");
        }
        Ok(log)
    }

    /// Applies Goldberg Emulator: replaces steam_api(64).dll + generates config.
    /// Does NOT run Steamless (use run_steamless separately for SteamStub games).
    ///
    /// `steam_api_key` is optional (pass "" to skip it) — without it, Steam's
    /// GetSchemaForGame Web API can't be called, so steam_settings/achievements.json
    /// never gets written and the emulator has no idea what achievements exist
    /// for this AppID, silently breaking achievement tracking for the game.
    pub async fn apply_goldberg(game_folder: String, app_id: String, steam_api_key: String) -> Result<String, String> {
        let game_dir = Path::new(&game_folder);
        if !game_dir.exists() {
            return Err("La carpeta del juego no existe.".to_string());
        }

        let mut log = String::new();
        log.push_str("=== Aplicando Goldberg Emulator ===\n");

        // 1. Find steam_api.dll/steam_api64.dll recursively
        log.push_str("[1/4] Buscando steam_api.dll...\n");
        let dlls = Self::find_steam_dlls_recursive(game_dir);
        if dlls.is_empty() {
            return Err("No se encontró steam_api.dll ni steam_api64.dll en la carpeta del juego.".to_string());
        }
        log.push_str(&format!("  Encontradas {} DLL(s).\n", dlls.len()));

        // 2. Backup DLLs
        log.push_str("[2/4] Respaldando DLLs originales...\n");
        for dll_path in &dlls {
            let bak_path = dll_path.with_extension("dll.bak");
            if !bak_path.exists() {
                fs::rename(dll_path, &bak_path).map_err(|e| {
                    format!("Error respaldando {}: {}", dll_path.display(), e)
                })?;
                log.push_str(&format!("  {} -> .bak\n", dll_path.file_name().unwrap_or_default().to_string_lossy()));
            } else {
                log.push_str(&format!("  Backup ya existe para {}\n", dll_path.file_name().unwrap_or_default().to_string_lossy()));
                // Remove current DLL so Goldberg can be placed
                let _ = fs::remove_file(dll_path);
            }
        }

        // 3. Download Goldberg and replace DLLs
        log.push_str("[3/4] Descargando e instalando Goldberg Emulator...\n");
        let goldberg_dir = Self::download_goldberg_emulator().await?;
        for dll_path in &dlls {
            let parent = dll_path.parent().unwrap_or(game_dir);
            let dll_name = dll_path.file_name().unwrap().to_string_lossy();
            let goldberg_src = goldberg_dir.join(&*dll_name);
            if goldberg_src.exists() {
                fs::copy(&goldberg_src, dll_path).map_err(|e| {
                    format!("Error copiando Goldberg {}: {}", dll_name, e)
                })?;
                log.push_str(&format!("  Goldberg {} -> {}\n", dll_name, parent.display()));
            } else {
                log.push_str(&format!("  Goldberg {} no encontrado en el paquete.\n", dll_name));
            }
        }

        // 4. Generate steam_settings/ config
        log.push_str("[4/4] Generando configuración...\n");
        let settings_dir_game = game_dir.join("steam_settings");
        if !settings_dir_game.exists() {
            let settings_str = settings_dir_game.to_str().ok_or("Ruta inválida")?;
            match Self::generate_goldberg_config(&steam_api_key, &app_id, settings_str).await {
                Ok(msg) => log.push_str(&format!("  {}\n", msg)),
                Err(e) => log.push_str(&format!("  Error en configuración: {} (continuando)\n", e)),
            }
        } else {
            log.push_str("  steam_settings/ ya existe.\n");
        }

        // Also try DLL parent directories if game root has no DLL directly
        let mut settings_dirs_created = false;
        for dll_path in &dlls {
            let parent = dll_path.parent().unwrap_or(game_dir);
            let settings_dir = parent.join("steam_settings");
            if !settings_dir.exists() {
                let settings_str = settings_dir.to_str().ok_or("Ruta inválida")?;
                match Self::generate_goldberg_config(&steam_api_key, &app_id, settings_str).await {
                    Ok(_) => settings_dirs_created = true,
                    Err(_) => {}
                }
            }
        }
        if settings_dirs_created {
            log.push_str("  steam_settings/ creado en directorio(s) de DLL.\n");
        }

        if steam_api_key.trim().is_empty() {
            log.push_str("  Aviso: sin Steam Web API Key no se generó achievements.json — los logros no se podrán rastrear para este juego. Configúrala en Herramientas.\n");
        }

        log.push_str("=== Goldberg Emulator aplicado exitosamente ===\n");
        Ok(log)
    }

    /// Restores original steam_api DLLs from backup (.bak) recursively
    pub fn restore_crack_backup(game_folder: &str) -> Result<String, String> {
        let game_dir = Path::new(game_folder);
        if !game_dir.exists() {
            return Err("La carpeta del juego no existe.".to_string());
        }

        let mut log = String::new();
        log.push_str("=== Restaurando respaldo ===\n");

        // Find all .bak files recursively and restore them.
        //
        // Every path restored here is remembered, because the config sweep at
        // the end of this function deletes files named `version.dll` and would
        // otherwise delete the very one just put back — it is not named
        // `.bak`, so that guard does not cover it. The log said "version.dll
        // restaurado" and "1 archivo(s) restaurado(s)" while the file was gone.
        let mut restored_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut restored = 0;
        for entry in walkdir::WalkDir::new(game_dir).into_iter().flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                if name.ends_with(".bak") && (name.starts_with("steam_api") || name.starts_with("version")) {
                    // Original name is the .bak file without the .bak extension
                    let original_path = path.with_extension("");
                    // Remove current file if exists
                    if original_path.exists() {
                        match fs::remove_file(&original_path) {
                            Ok(_) => {},
                            Err(e) => log.push_str(&format!("  Error eliminando {}: {}\n", original_path.display(), e)),
                        }
                    }
                    match fs::rename(path, &original_path) {
                        Ok(_) => {
                            log.push_str(&format!("  {} restaurado desde backup.\n", original_path.file_name().unwrap_or_default().to_string_lossy()));
                            restored_paths.insert(original_path.clone());
                            restored += 1;
                        }
                        Err(e) => log.push_str(&format!("  Error restaurando {}: {}\n", original_path.display(), e)),
                    }
                }
            }
        }

        // Remove all steam_settings directories recursively
        let mut settings_removed = 0usize;
        let mut settings_dirs: Vec<PathBuf> = Vec::new();
        for entry in walkdir::WalkDir::new(game_dir).into_iter().flatten() {
            if entry.file_type().is_dir() && entry.file_name().to_string_lossy() == "steam_settings" {
                settings_dirs.push(entry.path().to_path_buf());
            }
        }
        // Remove deepest first
        settings_dirs.sort_by(|a, b| b.to_string_lossy().len().cmp(&a.to_string_lossy().len()));
        for dir in settings_dirs {
            match fs::remove_dir_all(&dir) {
                Ok(_) => settings_removed += 1,
                Err(e) => log.push_str(&format!("  Error eliminando {}: {}\n", dir.display(), e)),
            }
        }
        if settings_removed > 0 {
            log.push_str(&format!("  {} directorio(s) steam_settings/ eliminado(s).\n", settings_removed));
        }

        // Remove bypass + config files recursively
        let config_patterns = [
            "SteamAPICheckBypass.json", "version.dll",
        ];
        let mut config_removed = 0usize;
        for entry in walkdir::WalkDir::new(game_dir).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !config_patterns.iter().any(|p| name == *p) || name.ends_with(".bak") {
                continue;
            }
            let path = entry.path();

            // Never delete what the restore loop above just put back.
            if restored_paths.contains(path) {
                continue;
            }

            // `version.dll` is a real Windows API set, and plenty of games ship
            // their own for overlays or their engine's DRM. Deleting every file
            // by that name anywhere under the install folder took those out
            // too — in subfolders this launcher had never written to — and the
            // log reported it in green as a successful restore.
            //
            // Only remove one we can show we installed: our own loader leaves a
            // `version_o.dll` (the original it displaced) or a
            // `Koaloader.config.json` beside it.
            if name == "version.dll" {
                let ours = path.parent().map_or(false, |dir| {
                    dir.join("version_o.dll").is_file() || dir.join("Koaloader.config.json").is_file()
                });
                if !ours {
                    log.push_str(&format!(
                        "  Se conserva {} — parece del juego, no instalado por Ragnarok.\n",
                        path.display()
                    ));
                    continue;
                }
            }

            if fs::remove_file(path).is_ok() {
                config_removed += 1;
            }
        }
        if config_removed > 0 {
            log.push_str(&format!("  {} archivo(s) de configuración eliminado(s).\n", config_removed));
        }

        if restored == 0 {
            log.push_str("  No se encontraron archivos de respaldo para restaurar.\n");
        } else {
            log.push_str(&format!("  {} archivo(s) restaurado(s).\n", restored));
        }

        log.push_str("=== Restauración completada ===\n");
        Ok(log)
    }

    /// Generates a crack-only zip file with Goldberg emulator files
    pub async fn generate_crack_zip(
        game_folder: &str,
        _app_id: &str,
        output_path: &str,
    ) -> Result<String, String> {
        let game_dir = Path::new(game_folder);
        if !game_dir.exists() {
            return Err("La carpeta del juego no existe.".to_string());
        }

        let output_file = Path::new(output_path);
        if let Some(parent) = output_file.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let file = fs::File::create(output_file).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipWriter::new(file);

        let options = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        let mut added = 0usize;

        // Include steam_api*.dll and version.dll recursively (only non-bak, non-backup)
        for entry in walkdir::WalkDir::new(game_dir).into_iter().flatten() {
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(game_dir).map_err(|e| e.to_string())?;
                let rel_str = rel.to_string_lossy();
                let name_lower = entry.file_name().to_string_lossy().to_lowercase();
                let is_relevant = name_lower == "steam_api.dll"
                    || name_lower == "steam_api64.dll"
                    || name_lower == "version.dll"
                    || name_lower == "steam_appid.txt"
                    || name_lower == "dlc.txt"
                    || name_lower == "steam_interfaces.txt"
                    || name_lower == "steam_token.txt"
                    || name_lower == "achievements.json"
                    || name_lower == "steamapicheckbypass.json";
                if is_relevant && !rel_str.contains(".bak") {
                    let data = fs::read(entry.path()).map_err(|e| e.to_string())?;
                    zip.start_file(rel_str.to_string(), options.clone()).map_err(|e| e.to_string())?;
                    zip.write_all(&data).map_err(|e| e.to_string())?;
                    added += 1;
                }
            }
        }

        // Include steam_settings/ directories recursively
        for entry in walkdir::WalkDir::new(game_dir).into_iter().flatten() {
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(game_dir).map_err(|e| e.to_string())?;
                let rel_str = rel.to_string_lossy();
                if rel_str.contains("steam_settings") {
                    let data = fs::read(entry.path()).map_err(|e| e.to_string())?;
                    zip.start_file(rel_str.to_string(), options.clone()).map_err(|e| e.to_string())?;
                    zip.write_all(&data).map_err(|e| e.to_string())?;
                    added += 1;
                }
            }
        }

        zip.finish().map_err(|e| e.to_string())?;

        if added == 0 {
            return Err("No se encontraron archivos del emulador para incluir en el zip.".to_string());
        }

        Ok(format!(
            "Crack-only zip generado: {} ({} archivos incluidos)",
            output_file.file_name().unwrap_or_default().to_string_lossy(),
            added
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gdrive_scrape() {
        let res = RustUnlockerManager::fetch_hv_games().await;
        println!("Result: {:?}", res);
        assert!(res.is_ok());
        let games = res.unwrap();
        println!("Games: {:?}", games);
        assert!(!games.is_empty());
    }

    #[tokio::test]
    async fn test_bypass_scrape() {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap();
        let bypass_id = "13iry6bKKpBR75EcpCzOMA8rIlr4zIZO6";
        let folder_url = format!("https://drive.google.com/drive/folders/{}", bypass_id);
        let res = client.get(&folder_url).send().await.unwrap();
        let html = res.text().await.unwrap();
        let pattern = format!(
            r#"\\x5b\\x22([a-zA-Z0-9_-]{{28,}})\\x22,\\x5b\\x22{}\\x22\\x5d,\\x22([^"\\]+)\\x22,\\x22application\\/[a-z-]+\\x22,0,null,0,0,0,[0-9]+,([0-9]+),null,null,([0-9]+),"#,
            bypass_id
        );
        let re = Regex::new(&pattern).unwrap();
        let mut count = 0;
        for _caps in re.captures_iter(&html) {
            count += 1;
        }
        println!("Bypass files count: {}", count);
        assert!(count > 0);
    }
}
