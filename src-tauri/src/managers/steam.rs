use std::fs;
use crate::diag_log;
use std::path::PathBuf;
use std::process::Command;
use serde::{Deserialize, Serialize};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn steam_exe_path() -> PathBuf {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Valve\\Steam") {
        if let Ok(p) = key.get_value::<String, _>("SteamPath") {
            let path = PathBuf::from(p.replace('/', "\\")).join("Steam.exe");
            if path.exists() { return path; }
        }
    }
    PathBuf::from("C:\\Program Files (x86)\\Steam\\Steam.exe")
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, Debug)]
pub struct SteamGameInfo {
    pub appid: String,
    pub name: String,
    pub installed: bool,
}

pub struct RustSteamManager;

impl RustSteamManager {
    /// Reads .acf files to determine if a game is installed (legacy, kept for compatibility).
    #[allow(dead_code)]
    pub fn check_installation(steam_path: &str, app_id: &str) -> bool {
        let acf_path = format!("{}/steamapps/appmanifest_{}.acf", steam_path, app_id);
        fs::metadata(&acf_path).is_ok()
    }

    /// Finds every real Steam *client* installation on the machine (a folder
    /// with its own Steam.exe), not just game-library folders.
    ///
    /// Some users keep two separate Steam client installs (e.g. one on C:,
    /// one on a second drive) and switch between them. The Windows registry
    /// only ever remembers the one that was launched most recently, so
    /// blindly trusting it can silently install the plugin into the "wrong"
    /// (currently inactive) Steam — where it does nothing. This scans the
    /// registry entries plus the common install locations on every drive
    /// letter and returns every folder that actually contains a Steam.exe.
    pub fn detect_steam_installations() -> Vec<String> {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;

        let mut found: Vec<String> = Vec::new();
        let mut push_if_valid = |raw: String| {
            let normalized = raw.replace('/', "\\").trim_end_matches('\\').to_string();
            if normalized.is_empty() {
                return;
            }
            let exe = PathBuf::from(&normalized).join("Steam.exe");
            if exe.exists() && !found.iter().any(|p: &String| p.eq_ignore_ascii_case(&normalized)) {
                found.push(normalized);
            }
        };

        // Registry-known paths (HKCU is the "currently active" one; the
        // HKLM variants can point to a different install on some setups).
        for (hive, subkey) in [
            (HKEY_CURRENT_USER, "Software\\Valve\\Steam"),
            (HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\Valve\\Steam"),
            (HKEY_LOCAL_MACHINE, "SOFTWARE\\Valve\\Steam"),
        ] {
            if let Ok(key) = RegKey::predef(hive).open_subkey(subkey) {
                if let Ok(p) = key.get_value::<String, _>("SteamPath") {
                    push_if_valid(p);
                }
                if let Ok(p) = key.get_value::<String, _>("InstallPath") {
                    push_if_valid(p);
                }
            }
        }

        // Common install locations on every drive letter — catches a second,
        // never-launched-this-boot install that the registry doesn't know
        // about (or has been overwritten by the other one).
        for letter in b'C'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            if !PathBuf::from(&drive).exists() {
                continue;
            }
            for suffix in ["Steam", "Program Files (x86)\\Steam", "Program Files\\Steam", "SteamLibrary\\Steam"] {
                push_if_valid(format!("{}{}", drive, suffix));
            }
        }

        found
    }

    /// Returns (steam.exe alive, steamwebhelper.exe alive) from a single process scan.
    pub fn process_status() -> (bool, bool) {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_processes();
        let procs = sys.processes();
        let steam_alive = procs.values().any(|p| p.name().eq_ignore_ascii_case("steam.exe"));
        let webhelper_alive = procs.values().any(|p| p.name().eq_ignore_ascii_case("steamwebhelper.exe"));
        (steam_alive, webhelper_alive)
    }

    /// Checks if Steam process is running using native OS APIs — no child process spawned.
    ///
    /// steam.exe alone is not a reliable signal: after a crash, a forced
    /// shutdown, or a bad PC power-off, steam.exe can remain as a stuck
    /// "zombie" process (bootstrapper hung, never spawning the real client)
    /// while showing up as "running" to a naive process-name check.
    /// steamwebhelper.exe is the process backing the actual client UI, so we
    /// require both to be present before reporting Steam as truly active.
    pub fn is_running() -> bool {
        let (steam_alive, webhelper_alive) = Self::process_status();
        steam_alive && webhelper_alive
    }

    /// Clears the "Steam already running" state Steam itself leaves behind in
    /// the registry (`ActiveProcess\pid`) and on disk (`steam.pid`, `.crash`).
    ///
    /// Root cause of "Steam worked fine, but after shutting down the PC (or
    /// after a few days) it won't open again": Steam tracks its own PID in
    /// `HKCU\Software\Valve\Steam\ActiveProcess` to detect a second instance.
    /// If Windows is powered off abruptly, or "Fast Startup" hibernates the
    /// session instead of a clean shutdown, that PID never gets cleared. On
    /// the next boot Steam believes an old instance is already alive, so a
    /// new Steam.exe launch just silently signals the (nonexistent) old
    /// instance and exits — nothing visibly happens, and no error is shown.
    /// Only call this when we've confirmed steam.exe is NOT actually running.
    pub fn clear_stale_state() {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
        use winreg::RegKey;

        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Software\\Valve\\Steam\\ActiveProcess", KEY_SET_VALUE)
        {
            let _ = key.set_value("pid", &0u32);
            let _ = key.set_value("ActiveUser", &0u32);
        }

        if let Some(steam_dir) = steam_exe_path().parent() {
            for f in [".crash", "steam.pid"] {
                let fp = steam_dir.join(f);
                if fp.exists() {
                    let _ = fs::remove_file(&fp);
                }
            }
        }
    }

    /// Forcefully kills Steam and every helper process/service it spawns.
    ///
    /// Killing only steam.exe is not enough: steamwebhelper.exe and
    /// GameOverlayUI.exe often keep plugin DLLs (dwmapi.dll, xinput1_4.dll,
    /// OpenSteamTool.dll) locked, so a subsequent repair/reinstall silently
    /// fails to overwrite them. Stopping "Steam Client Service" too avoids
    /// the service relaunching steam.exe mid-repair.
    /// Whether the Steam client is up right now.
    ///
    /// Lives here rather than inside one caller because install_plugin needs
    /// it too: it kills Steam unconditionally, and without a way to ask
    /// afterwards it could only ever claim to have restarted it.
    pub fn steam_is_running() -> bool {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_processes();
        sys.processes()
            .values()
            .any(|p| p.name().eq_ignore_ascii_case("steam.exe"))
    }

    pub fn kill_all_steam_processes() {
        for proc in ["steam.exe", "steamwebhelper.exe", "steamerrorreporter.exe", "steamservice.exe", "GameOverlayUI.exe"] {
            let _ = Command::new("taskkill")
                .args(["/F", "/IM", proc])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
        let _ = Command::new("net")
            .args(["stop", "Steam Client Service"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }

    /// Launches a game via Steam.exe with DRM bypass flags to prevent "Buy" prompts.
    pub fn launch_game(app_id: &str, steam_path: &str, _app_name: &str) -> Result<(), String> {

        // Steam already up: hand it the game over the protocol and leave the
        // client alone.
        //
        // Everything below this point is cold-start handling — it starts
        // Steam with bootstrap-bypass flags and wipes a web cache that is
        // routinely corrupt on a broken install. That is right for launching
        // Steam ourselves, and wrong for a client the user is already in:
        // it would delete files Steam currently holds open. Which matters
        // here specifically, because the whole point of launching from the
        // library is that Steam is already sitting in the background.
        if Self::steam_is_running() {
            return Command::new("cmd")
                .args(["/C", "start", &format!("steam://rungameid/{}", app_id)])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("Error usando protocolo steam:// : {}", e));
        }

        // --- LIMPIEZA PREVENTIVA SILENCIOSA ---
        // 1. Limpiar la caché web (htmlcache) de Steam que suele corromperse y causar que no abra
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let htmlcache = PathBuf::from(local_app_data).join("Steam").join("htmlcache");
            if htmlcache.exists() {
                // Borramos silenciosamente ignorando errores si hay archivos en uso
                let _ = fs::remove_dir_all(&htmlcache);
            }
        }
        // --------------------------------------

        let mut exe = PathBuf::from(steam_path).join("steam.exe");
        if !exe.exists() {
            exe = PathBuf::from(steam_path).join("Steam.exe");
        }

        let target_exe = if exe.exists() {
            exe
        } else {
            steam_exe_path() // Fallback to registry if not found in steam_path
        };

        if target_exe.exists() {
            Command::new(&target_exe)
                .args([
                    "-nobootstrapupdate",
                    "-skipinitialbootstrap",
                    "-noverifyfiles",
                    "-applaunch",
                    app_id,
                ])
                .spawn()
                .map_err(|e| format!("Error ejecutando Steam.exe: {}", e))?;
        } else {
            // Fallback: protocol handler (no bypass flags)
            Command::new("cmd")
                .args(["/C", "start", &format!("steam://run/{}", app_id)])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| format!("Error usando protocolo steam:// : {}", e))?;
        }
        Ok(())
    }

    /// Generates SteamTools.lua with ownership, manifest GIDs, and DLC table.
    /// - app_entries:   (game_id, dlc_ids)
    /// - manifest_map:  game_id → { depot_id → manifest_gid }  (fixes 0 B install size)
    pub fn update_steam_tools(
        steam_path: &str,
        app_entries: Vec<(String, Vec<String>)>,
        manifest_map: &std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    ) -> Result<(), String> {
        // An empty list means something upstream failed, not that the user has
        // no games.
        //
        // This function replaces all three .lua files outright, and its only
        // input is `load_apps_map`, which turns any read or parse failure into
        // an empty map with `unwrap_or_default()` — no error, no log. So a
        // truncated `ragnarok_apps.json` (a power cut during an earlier save,
        // an antivirus lock, a hand edit with a missing comma) made the next
        // sync rewrite every file with zero `addappid` lines. The user opened
        // Steam and their whole spoofed library was gone, with the operation
        // having reported success — and retrying could not fix it, because the
        // sidecar was still broken.
        //
        // Worse, this is reached from background work (`sync_games` after
        // auto-updating manifests, and `check_installed_game_updates`), so it
        // could happen without anyone pressing anything.
        //
        // Refusing to write is always the right call here: keeping yesterday's
        // working file costs nothing, and rewriting it empty costs everything.
        if app_entries.is_empty() {
            diag_log!(
                "[steam_tools] Lista de juegos vacía — no se reescriben los .lua para no borrar la biblioteca."
            );
            return Ok(());
        }

        let mut lines = Vec::new();
        lines.push("-- Auto-generated by Ragnarok Launcher".to_string());
        lines.push("-- Do not edit manually".to_string());
        lines.push(String::new());

        // ── 1. Ownership / license spoofing ────────────────────────────────
        // Games installed via Ryuu's ticket generator already have their own
        // config/lua/{app_id}.lua declaring a bare addappid(app_id) (no
        // ownership flag — that's intentional, see download_and_install).
        // Re-declaring the SAME appid here with a DIFFERENT flag
        // (addappid(app_id, 1)) is the exact same class of conflict that was
        // confirmed crashing Steam for Dave the Diver over duplicate/
        // conflicting manifest ids — here it manifested as DLCs added
        // through the DLC Manager never actually showing as owned even after
        // a full Steam restart (confirmed: SteamTools.lua itself was written
        // correctly, so the conflict is in how OpenSteamTool reconciles two
        // files disagreeing about the same appid, not in our data). Defer to
        // the ticket file as the sole source of truth for the base game's
        // own addappid line; still declare every DLC id here (the ticket
        // never lists those) plus the apps[] table below.
        for (game_id, dlc_ids) in &app_entries {
            let has_own_ticket = PathBuf::from(steam_path)
                .join("config").join("lua")
                .join(format!("{}.lua", game_id))
                .exists();
            if !has_own_ticket {
                lines.push(format!("addappid({}, 1)", game_id));
            }
            for dlc_id in dlc_ids {
                lines.push(format!("addappid({}, 1)", dlc_id));
            }
        }
        lines.push(String::new());

        // ── 1b. Stats/Achievements (OpenSteamTool's own setStat mechanism) ──
        // Per github.com/OpenSteam001/OpenSteamTool#usage, an unowned game has
        // no real Steam stats blob of its own — setStat(appid, steamid) tells
        // OpenSteamTool which SteamID's stats schema to serve for it instead,
        // which is what lets the game's native Steam achievement panel show
        // real achievement structure instead of erroring out. Without an
        // explicit override, OpenSteamTool's own priority chain falls back to
        // a stats API (stats.opensteamtool.com) and finally to this exact
        // hardcoded SteamID — we only implement the fallback here, since the
        // API sits behind a Cloudflare bot challenge that a plain HTTP client
        // can never pass, making it unreachable from this app in practice.
        const OPENSTEAMTOOL_FALLBACK_STEAMID: &str = "76561198028121353";
        for (game_id, _) in &app_entries {
            lines.push(format!("setStat({}, \"{}\")", game_id, OPENSTEAMTOOL_FALLBACK_STEAMID));
        }
        lines.push(String::new());

        // ── 2. Manifest GIDs (tells Steam the correct depot version → real size) ──
        let mut wrote_manifest = false;
        for (game_id, _) in &app_entries {
            if let Some(depots) = manifest_map.get(game_id) {
                for (depot_id, gid) in depots {
                    lines.push(format!("setManifestid({}, \"{}\")", depot_id, gid));
                    wrote_manifest = true;
                }
            }
        }
        if wrote_manifest {
            lines.push(String::new());
        }

        // ── 3. DLC unlocking table ─────────────────────────────────────────
        lines.push("local apps = {}".to_string());
        lines.push(String::new());
        for (game_id, dlc_ids) in &app_entries {
            let dlc_part = if dlc_ids.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{}}}", dlc_ids.join(", "))
            };
            lines.push(format!("apps[{}] = {}", game_id, dlc_part));
        }

        lines.push(String::new());
        lines.push("return apps".to_string());

        let content = lines.join("\n");

        // Primary location: config/lua/SteamTools.lua
        let plugin_dir = PathBuf::from(steam_path).join("config").join("lua");
        fs::create_dir_all(&plugin_dir).map_err(|e| e.to_string())?;
        let primary_path = plugin_dir.join("SteamTools.lua");
        fs::write(&primary_path, &content).map_err(|e| e.to_string())?;

        // Compatibility copy: steamapps/SteamTools.lua
        let steamapps_dir = PathBuf::from(steam_path).join("steamapps");
        fs::create_dir_all(&steamapps_dir).map_err(|e| e.to_string())?;
        let compat_path = steamapps_dir.join("SteamTools.lua");
        fs::write(&compat_path, &content).map_err(|e| e.to_string())?;

        // OpenSteamTool hot-reload location: config/lua/RagnarokGames.lua
        let ost_lua_dir = PathBuf::from(steam_path).join("config").join("lua");
        fs::create_dir_all(&ost_lua_dir).map_err(|e| e.to_string())?;
        let ost_path = ost_lua_dir.join("RagnarokGames.lua");
        fs::write(&ost_path, &content).map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Copies OpenSteamTool DLLs from the launcher files to the Steam root directory.
    pub fn install_open_steam_tool(steam_path: &str) -> Result<(), String> {
        let src_dir = std::env::current_dir()
            .map(|p| p.join("archivos de OpenSteamTool"))
            .map_err(|e| e.to_string())?;

        if !src_dir.exists() {
            return Err("No se encontró la carpeta 'archivos de OpenSteamTool' en el directorio del launcher.".to_string());
        }

        let dest_dir = std::path::Path::new(steam_path);
        if !dest_dir.exists() {
            return Err("El directorio de Steam especificado no existe.".to_string());
        }

        // Files to copy
        let files = ["OpenSteamTool.dll", "dwmapi.dll", "xinput1_4.dll"];
        for file in &files {
            let src_file = src_dir.join(file);
            if src_file.exists() {
                let dest_file = dest_dir.join(file);
                fs::copy(&src_file, &dest_file)
                    .map_err(|e| format!("Error al copiar {}: {}", file, e))?;
            } else {
                return Err(format!("Falta el archivo requerido: {}", file));
            }
        }

        // Copy the real bundled opensteamtool.toml if the user doesn't have one
        // yet. This one has the [manifest]/[stats]/[lua]/[inject] sections —
        // the old fallback below (kept only if the bundled file is somehow
        // missing) wrote just [general]/[unlock], silently dropping the
        // [manifest] section that controls which upstream service OpenSteamTool
        // queries to resolve a depot's current manifest id when the Lua files
        // don't pin one explicitly.
        let toml_path = dest_dir.join("opensteamtool.toml");
        if !toml_path.exists() {
            let src_toml = src_dir.join("opensteamtool.toml");
            if src_toml.exists() {
                let _ = fs::copy(&src_toml, &toml_path);
            } else {
                let default_toml = r#"[general]
# OpenSteamTool Configuration
[unlock]
enabled = true

[manifest]
# Upstream API for depot manifest request codes.
# Options: "manifestdex"
url = "manifestdex"

[stats]
enable_api = true

[lua]
paths = []

[inject]
enabled = false
"#;
                let _ = fs::write(&toml_path, default_toml);
            }
        }

        Ok(())
    }

    /// Restarts Steam by killing the process and launching it again.
    pub fn restart_steam() -> Result<(), String> {
        // Kill Steam
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "steam.exe"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        std::thread::sleep(std::time::Duration::from_millis(1000));

        Command::new("cmd")
            .args(["/C", "start", "steam://open/main"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Failed to start Steam: {}", e))?;
        Ok(())
    }
}
