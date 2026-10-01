//! Nintendo Switch emulator management.
//!
//! Scope note — this module deliberately handles only three things:
//!   1. Downloading/updating the emulators themselves (legal software, and
//!      the only part of a Switch setup that has a legitimate public source).
//!   2. Placing keys/firmware the USER already has into the folder the
//!      emulator expects — the same "you pick a local file, Ragnarok puts it
//!      in the right place" flow the Bypass tab's local-archive option uses.
//!   3. Listing game files already present in a folder the user points at,
//!      with cover art looked up from Nintendo's own public eShop search.
//!
//! It does NOT fetch keys, firmware or games from anywhere. Those are
//! Nintendo's decryption keys, system software and copyrighted titles
//! respectively; automating their download is out of scope for this project.
//!
//! Both supported emulators are community forks kept after Nintendo shut
//! down yuzu (2024) and Ryujinx (2024). Neither survives on GitHub — the old
//! mirrors there answer HTTP 451 — so both are hosted on their own Forgejo
//! instances, which happen to expose the same `/api/v1/repos/…/releases`
//! shape, letting one code path serve both.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct EmulatorDef {
    id: &'static str,
    name: &'static str,
    description_es: &'static str,
    description_en: &'static str,
    releases_api: &'static str,
    /// Every keyword must appear in the asset filename for it to be picked —
    /// these releases ship dozens of assets (Android APKs, AppImages, macOS
    /// dmg, several Windows compiler variants), so a single "is it a .zip"
    /// test is nowhere near specific enough.
    asset_keywords: &'static [&'static str],
    /// Executable name to look for after extraction, matched case-insensitively
    /// against the file stem.
    exe_stem: &'static str,
    /// Which console this emulates. Drives what the UI offers: only Switch
    /// emulators have a keys/firmware flow to show.
    console: Console,
    /// Where this emulator reads prod.keys/title.keys from, relative to its
    /// own install folder (both forks support a portable layout, which is
    /// what a self-contained install like this one wants).
    ///
    /// `None` outside the Switch: PS3 and PS4 have no equivalent. RPCS3
    /// installs a PS3UPDAT.PUP from its own menu, and shadPS4 needs no keys
    /// at all, so there is nothing for Ragnarok to place for them.
    portable_keys_dir: Option<&'static str>,
    /// Same, but the roaming-profile location, written to as well so the
    /// emulator finds the keys whether or not portable mode ends up active.
    appdata_keys_dir: Option<&'static str>,
    /// Where the emulator keeps installed firmware NCAs, relative to the
    /// install folder (portable layout) and to the roaming profile.
    portable_firmware_dir: Option<&'static str>,
    appdata_firmware_dir: Option<&'static str>,
}

/// The console an emulator targets.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Console {
    Switch,
    Ps2,
    Ps3,
    Ps4,
}

impl Console {
    fn label(self) -> &'static str {
        match self {
            Console::Switch => "Nintendo Switch",
            Console::Ps2 => "PlayStation 2",
            Console::Ps3 => "PlayStation 3",
            Console::Ps4 => "PlayStation 4",
        }
    }
}

const EMULATORS: &[EmulatorDef] = &[
    EmulatorDef {
        id: "ryubing",
        name: "Ryubing",
        description_es: "Continuación comunitaria de Ryujinx. Muy compatible y liviana.",
        description_en: "Community continuation of Ryujinx. Highly compatible and lightweight.",
        releases_api: "https://git.ryujinx.app/api/v1/repos/ryubing/ryujinx/releases?limit=1",
        asset_keywords: &["win_x64", ".zip"],
        exe_stem: "ryujinx",
        console: Console::Switch,
        portable_keys_dir: Some("portable/system"),
        appdata_keys_dir: Some("Ryujinx/system"),
        portable_firmware_dir: Some("portable/bis/system/Contents/registered"),
        appdata_firmware_dir: Some("Ryujinx/bis/system/Contents/registered"),
    },
    EmulatorDef {
        id: "eden",
        name: "Eden",
        description_es: "Fork activo de yuzu. Suele rendir mejor en equipos modestos.",
        description_en: "Active yuzu fork. Often performs better on modest hardware.",
        releases_api: "https://git.eden-emu.dev/api/v1/repos/eden-emu/eden/releases?limit=1",
        asset_keywords: &["Windows", "amd64-msvc", ".zip"],
        exe_stem: "eden",
        console: Console::Switch,
        portable_keys_dir: Some("user/keys"),
        appdata_keys_dir: Some("eden/keys"),
        portable_firmware_dir: Some("user/nand/system/Contents/registered"),
        appdata_firmware_dir: Some("eden/nand/system/Contents/registered"),
    },
    EmulatorDef {
        id: "rpcs3",
        name: "RPCS3",
        description_es: "Emulador de PlayStation 3. Muy maduro: corre la mayor parte del catálogo.",
        description_en: "PlayStation 3 emulator. Very mature — runs most of the catalogue.",
        // The Windows builds live in their own repository; the main rpcs3 repo
        // publishes no binaries.
        releases_api: "https://api.github.com/repos/RPCS3/rpcs3-binaries-win/releases/latest",
        asset_keywords: &["win64", ".7z"],
        exe_stem: "rpcs3",
        console: Console::Ps3,
        // PS3 firmware is a PS3UPDAT.PUP the user installs from RPCS3's own
        // menu, so there is nothing for Ragnarok to drop into place.
        portable_keys_dir: None,
        appdata_keys_dir: None,
        portable_firmware_dir: None,
        appdata_firmware_dir: None,
    },
    EmulatorDef {
        id: "shadps4",
        name: "shadPS4",
        description_es: "Emulador de PlayStation 4. En desarrollo activo, ya corre bastantes juegos.",
        description_en: "PlayStation 4 emulator. Actively developed, already runs a fair few games.",
        releases_api: "https://api.github.com/repos/shadps4-emu/shadPS4/releases?per_page=1",
        asset_keywords: &["win64", ".zip"],
        exe_stem: "shadps4",
        console: Console::Ps4,
        portable_keys_dir: None,
        appdata_keys_dir: None,
        portable_firmware_dir: None,
        appdata_firmware_dir: None,
    },
    EmulatorDef {
        id: "pcsx2",
        name: "PCSX2",
        description_es: "Emulador de PlayStation 2. El más veterano y compatible de todos.",
        description_en: "PlayStation 2 emulator. The most mature and compatible of the lot.",
        // Its Windows builds ship as prereleases, which /releases/latest
        // deliberately skips — so the list endpoint is the right one here.
        releases_api: "https://api.github.com/repos/PCSX2/pcsx2/releases?per_page=1",
        // "Qt.7z" and not just ".7z": the same release also carries
        // "...-Qt-symbols.7z", a debug-symbol archive that is not the app.
        asset_keywords: &["windows-x64", "Qt.7z"],
        exe_stem: "pcsx2-qt",
        console: Console::Ps2,
        portable_keys_dir: None,
        appdata_keys_dir: None,
        portable_firmware_dir: None,
        appdata_firmware_dir: None,
    },
];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EmulatorInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// None when the release API couldn't be reached — the UI shows the
    /// emulator as unavailable rather than pretending there's an update.
    pub latest_version: Option<String>,
    pub installed_version: Option<String>,
    pub install_path: Option<String>,
    pub exe_path: Option<String>,
    /// "switch", "ps2", "ps3" or "ps4". The UI groups by this and only offers
    /// the prod.keys/firmware flow where it means anything.
    pub console: String,
    /// Human name of that console, for headings.
    pub console_label: String,
    /// Whether this emulator takes key files through Ragnarok at all.
    pub uses_keys: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SwitchGame {
    /// Parsed from the conventional `[0100XXXXXXXXXXXX]` tag dumps carry in
    /// their filename. None when the filename doesn't include one.
    pub title_id: Option<String>,
    /// Filename with the bracketed metadata tags stripped — what a person
    /// would call the game.
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub format: String,
}

/// Dedicated client for the emulators' own git hosts — deliberately NOT
/// crate::make_client(), whose full desktop-Chrome User-Agent is needed for
/// Steam/GitHub scraping but breaks things here.
///
/// git.ryujinx.app (GitLab) content-negotiates on the User-Agent alone: with
/// a browser UA it answers the API path with the HTML web page and HTTP 200,
/// so parsing it as JSON fails and Ryubing showed up as "servidor no
/// disponible". Sending `Accept: application/json` does NOT override it —
/// only a non-browser UA does. Eden's Forgejo host doesn't do this, which is
/// why only one of the two was affected.
///
/// Same class of problem as make_discord_client's, and solved the same way.
fn api_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("Ragnarok-Launcher-App")
        // The same client also pulls the ~30 MB release zip, so this covers
        // the whole download, not just the API call — 120s would abort a
        // perfectly healthy install on a slow connection.
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}

pub fn emulators_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("emulators"))
}

fn install_dir_for(id: &str) -> Option<PathBuf> {
    Some(emulators_root()?.join(id))
}

/// Version marker written next to the extracted files. Reading the emulator's
/// own binary for a version is unreliable across forks; a marker we control
/// is both simpler and exact.
fn version_marker(install_dir: &Path) -> PathBuf {
    install_dir.join(".ragnarok_version")
}

fn def_for(id: &str) -> Option<&'static EmulatorDef> {
    EMULATORS.iter().find(|e| e.id == id)
}

/// Latest release tag + matching Windows asset URL from a Forgejo-style
/// `/api/v1/…/releases` endpoint.
async fn fetch_latest_release(
    client: &reqwest::Client,
    def: &EmulatorDef,
) -> Result<(String, String), String> {
    let res = client
        .get(def.releases_api)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("No se pudo contactar el servidor de {}: {}", def.name, e))?;
    if !res.status().is_success() {
        return Err(format!("{} respondió HTTP {}", def.name, res.status()));
    }
    // Two shapes, because the endpoints differ. Forgejo's `?limit=1` and
    // GitHub's `?per_page=1` answer with an array; GitHub's
    // `/releases/latest` answers with a single object. Only the array was
    // handled, so pointing an emulator at `/releases/latest` failed to parse
    // — and for RPCS3 that endpoint is the only one that gives the newest
    // build, since its release list is not ordered newest-first.
    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Respuesta inválida de {}: {}", def.name, e))?;
    let release = match &body {
        serde_json::Value::Array(list) => list
            .first()
            .cloned()
            .ok_or_else(|| format!("{} no tiene ningún release publicado.", def.name))?,
        other => other.clone(),
    };

    let tag = release["tag_name"].as_str().unwrap_or("?").to_string();
    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    let url = assets
        .iter()
        .find(|a| {
            let name = a["name"].as_str().unwrap_or("");
            def.asset_keywords.iter().all(|k| name.contains(k)) && !is_side_file(name)
        })
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| format!("El último release de {} no trae una build de Windows.", def.name))?
        .to_string();

    Ok((tag, url))
}

/// True for the little files that sit beside a real build.
///
/// Keyword matching alone cannot exclude them: RPCS3 ships
/// `rpcs3-…_win64.7z` next to `rpcs3-…_win64.7z.sha256`, and every keyword
/// that matches the archive matches the checksum too, because the archive's
/// whole name is a prefix of it. Picking the 3 KB checksum instead of the
/// 19 MB build would fail later, during extraction, with an error pointing
/// nowhere near the download.
fn is_side_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".sha256", ".sha512", ".md5", ".sig", ".asc", ".txt"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod asset_picker_tests {
    use super::is_side_file;

    /// The real pair RPCS3 publishes.
    #[test]
    fn rejects_the_checksum_next_to_the_build() {
        assert!(!is_side_file("rpcs3-v0.0.42-19818-3aac7d77_win64_msvc.7z"));
        assert!(is_side_file("rpcs3-v0.0.42-19818-3aac7d77_win64_msvc.7z.sha256"));
    }

    #[test]
    fn leaves_real_builds_alone() {
        for name in [
            "shadps4-win64-sdl-0.18.0.zip",
            "pcsx2-v2.7.524-windows-x64-Qt.7z",
            "eden-Windows-amd64-msvc.zip",
        ] {
            assert!(!is_side_file(name), "descartó {}", name);
        }
    }
}

/// Walks an extracted install looking for the emulator's executable. These
/// archives nest the payload inside a folder whose name changes per release
/// (`publish/`, `Eden-Windows-.../`), so a fixed relative path won't do.
fn find_exe(dir: &Path, exe_stem: &str) -> Option<PathBuf> {
    // Three tiers, resolved only after the whole tree has been walked.
    // Returning early on a mere "contains" match picked Eden's `eden-cli.exe`
    // over `eden.exe` purely because walkdir happened to reach it first —
    // which would have pointed both the desktop shortcut and the Launch
    // button at the console build instead of the emulator.
    let mut exact: Option<PathBuf> = None;
    let mut partial: Option<PathBuf> = None;
    let mut largest: Option<(u64, PathBuf)> = None;

    for entry in walkdir::WalkDir::new(dir).max_depth(4).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()) != Some("exe".into()) {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();

        if stem == exe_stem {
            exact = Some(path.to_path_buf());
            continue;
        }
        // Some forks rename the binary between releases, so a partial match
        // still beats nothing — but never a `-cli`/`-cmd` console build,
        // which is a companion tool, not the emulator.
        if stem.contains(exe_stem)
            && !stem.contains("cli")
            && !stem.contains("cmd")
            && !stem.contains("room")
            && partial.is_none()
        {
            partial = Some(path.to_path_buf());
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if largest.as_ref().map(|(s, _)| size > *s).unwrap_or(true) {
            largest = Some((size, path.to_path_buf()));
        }
    }

    exact.or(partial).or_else(|| largest.map(|(_, p)| p))
}

pub async fn list_emulators() -> Vec<EmulatorInfo> {
    let client = match api_client() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for def in EMULATORS {
        let install_dir = install_dir_for(def.id);
        let installed_version = install_dir
            .as_ref()
            .and_then(|d| fs::read_to_string(version_marker(d)).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let exe_path = if installed_version.is_some() {
            install_dir
                .as_ref()
                .and_then(|d| find_exe(d, def.exe_stem))
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };

        // Best-effort: a fork's server being down shouldn't blank the whole
        // tab, it should just hide the update prompt for that one entry.
        let latest_version = fetch_latest_release(&client, def).await.ok().map(|(tag, _)| tag);

        out.push(EmulatorInfo {
            id: def.id.to_string(),
            name: def.name.to_string(),
            description: def.description_es.to_string(),
            latest_version,
            installed_version,
            install_path: install_dir.map(|d| d.to_string_lossy().to_string()),
            exe_path,
            console: match def.console {
                Console::Switch => "switch",
                Console::Ps2 => "ps2",
                Console::Ps3 => "ps3",
                Console::Ps4 => "ps4",
            }
            .to_string(),
            console_label: def.console.label().to_string(),
            uses_keys: def.portable_keys_dir.is_some(),
        });
    }
    out
}

/// English descriptions live alongside the Spanish ones so the frontend can
/// pick without a second round-trip.
pub fn description_for(id: &str, english: bool) -> String {
    def_for(id)
        .map(|d| if english { d.description_en } else { d.description_es })
        .unwrap_or("")
        .to_string()
}

/// Whether a download URL points at a 7-Zip archive.
///
/// Decided by the asset's file name rather than by which emulator it is: the
/// projects change packaging between releases, and a hardcoded per-emulator
/// answer would silently go stale the first time one of them switched.
fn archive_is_7z(url: &str) -> bool {
    url.rsplit('/')
        .next()
        .unwrap_or(url)
        // A CDN redirect can append a query string; the name ends before it.
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .ends_with(".7z")
}

#[cfg(test)]
mod archive_format_tests {
    use super::archive_is_7z;

    /// The real assets each project publishes for Windows.
    #[test]
    fn tells_the_two_formats_apart() {
        let base = "https://github.com/o/r/releases/download/v1/";
        assert!(archive_is_7z(&format!("{}rpcs3-v0.0.42_win64_msvc.7z", base)));
        assert!(archive_is_7z(&format!("{}pcsx2-v2.7.524-windows-x64-Qt.7z", base)));
        assert!(!archive_is_7z(&format!("{}shadps4-win64-sdl-0.18.0.zip", base)));
        assert!(!archive_is_7z(&format!("{}eden-Windows-amd64-msvc.zip", base)));
    }

    /// A signed CDN link still ends in the asset's own name.
    #[test]
    fn ignores_a_query_string() {
        assert!(archive_is_7z("https://cdn/x/rpcs3_win64.7z?token=abc"));
        assert!(!archive_is_7z("https://cdn/x/shadps4.zip?token=abc"));
    }

    /// ".7z.sha256" is a checksum, not an archive — is_side_file already
    /// filters these out, but neither should this call it an archive.
    #[test]
    fn a_checksum_is_not_an_archive() {
        assert!(!archive_is_7z("https://cdn/x/rpcs3_win64.7z.sha256"));
    }
}

pub async fn install_emulator(app: Option<&tauri::AppHandle>, id: &str) -> Result<String, String> {
    use futures::StreamExt;
    use tauri::Manager;

    let def = def_for(id).ok_or_else(|| format!("Emulador desconocido: {}", id))?;
    let install_dir = install_dir_for(id).ok_or("No se pudo resolver AppData")?;

    // Same non-browser client for the download itself: on the host that
    // sniffs the User-Agent, a browser UA would hand back an HTML page here
    // too, and the zip parse would fail with a confusing error.
    let client = api_client()?;
    let (tag, url) = fetch_latest_release(&client, def).await?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Error descargando {}: {}", def.name, e))?;
    let total = response.content_length().unwrap_or(0);

    // Streamed rather than buffered in one .bytes() call so the UI can show
    // real progress — these are ~30 MB downloads, long enough that a button
    // stuck on "working…" reads as a hang.
    let mut bytes: Vec<u8> = Vec::with_capacity(total as usize);
    let mut stream = response.bytes_stream();
    let mut last_emitted = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Descarga incompleta de {}: {}", def.name, e))?;
        bytes.extend_from_slice(&chunk);
        if let Some(app) = app {
            // Throttled to whole percent steps; emitting per chunk would
            // flood the webview with events for no visible gain.
            let pct = if total > 0 { bytes.len() as u64 * 100 / total } else { 0 };
            if pct != last_emitted {
                last_emitted = pct;
                let _ = app.emit_all(
                    "emulator_download_progress",
                    serde_json::json!({
                        "id": id,
                        "percentage": pct,
                        "downloaded": bytes.len() as u64,
                        "total": total,
                    }),
                );
            }
        }
    }

    if let Some(app) = app {
        let _ = app.emit_all(
            "emulator_download_progress",
            serde_json::json!({ "id": id, "percentage": 100, "downloaded": bytes.len() as u64, "total": total, "extracting": true }),
        );
    }

    // Replace rather than merge: leftovers from an older version's folder
    // layout are a classic source of "it launches but crashes" reports.
    // The keys folders are preserved below, since those are the user's own
    // files and re-placing them after every update would be needless work.
    let preserved = stash_keys(&install_dir, def);

    // Extract beside the install and swap only once it worked.
    //
    // This used to delete the working install first and find out afterwards
    // whether the download was usable. Every `?` in the extraction below then
    // returned early, so `restore_keys` never ran either — and the failure it
    // hits most is the one this module's own header documents: a host serving
    // an HTML error page instead of the archive. The user pressed "Actualizar"
    // on a working emulator and was left with an empty folder and no
    // prod.keys, having lost both to a download that was never valid.
    //
    // The staging folder is a sibling, not %TEMP%, so the final rename stays
    // on one volume.
    let sibling = |suffix: &str| -> PathBuf {
        let name = install_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.to_string());
        install_dir.with_file_name(format!("{}{}", name, suffix))
    };
    let staging = sibling(".ragnarok-new");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    // Everything below writes here; `install_dir` is not touched until the
    // swap at the end.
    let install_dir = staging.clone();

    // The archive format follows the asset, not the emulator: RPCS3 and PCSX2
    // both ship .7z on Windows while everything else ships .zip. Feeding a 7z
    // to the zip reader fails with "Could not find central directory end" —
    // which is exactly what a user hit installing the PS3 emulator, and an
    // error that points nowhere near the real cause.
    if archive_is_7z(&url) {
        // sevenz-rust reads from a path, so the download has to land on disk
        // before it can be extracted.
        let tmp = std::env::temp_dir().join(format!("ragnarok_emu_{}.7z", id));
        fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        let extracted = sevenz_rust::decompress_file(&tmp, &install_dir)
            .map_err(|e| format!("No se pudo extraer el archivo de {}: {}", def.name, e));
        let _ = fs::remove_file(&tmp);
        extracted?;
    } else {
        let cursor = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("El archivo descargado no es un ZIP válido: {}", e))?;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            // enclosed_name() rejects zip-slip entries (`..`, absolute paths).
            let Some(name) = file.enclosed_name().map(|p| p.to_path_buf()) else { continue };
            let outpath = install_dir.join(&name);
            if file.name().ends_with('/') {
                fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(p) = outpath.parent() {
                fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
        }
    }

    // Extraction worked. Now — and only now — replace the old install.
    let install_dir = {
        let target = staging.with_file_name(
            staging
                .file_name()
                .map(|n| n.to_string_lossy().replace(".ragnarok-new", ""))
                .unwrap_or_else(|| id.to_string()),
        );
        if target.exists() {
            let retired = sibling(".ragnarok-old");
            let _ = fs::remove_dir_all(&retired);
            fs::rename(&target, &retired).map_err(|e| {
                let _ = fs::remove_dir_all(&staging);
                format!("No se pudo apartar la instalación anterior: {}", e)
            })?;
            if let Err(e) = fs::rename(&staging, &target) {
                // Put the working install back rather than leaving nothing.
                let _ = fs::rename(&retired, &target);
                let _ = fs::remove_dir_all(&staging);
                return Err(format!("No se pudo instalar la versión nueva: {}", e));
            }
            let _ = fs::remove_dir_all(&retired);
        } else {
            fs::rename(&staging, &target).map_err(|e| e.to_string())?;
        }
        target
    };

    restore_keys(&install_dir, def, preserved);
    fs::write(version_marker(&install_dir), &tag).map_err(|e| e.to_string())?;

    // Emulators land in Ragnarok's own AppData folder, which nobody browses
    // to — without a shortcut the install genuinely looks like it did
    // nothing. Best-effort: a missing shortcut is not a failed install.
    if let Some(exe) = find_exe(&install_dir, def.exe_stem) {
        let _ = create_desktop_shortcut(def.name, &exe);
    }

    Ok(tag)
}

/// Drops a .lnk on the desktop pointing at the emulator's exe. Uses the
/// WScript.Shell COM object through PowerShell rather than pulling in a COM
/// crate for one call; CREATE_NO_WINDOW keeps the console from flashing,
/// the same way every other PowerShell call in this project does.
#[cfg(windows)]
fn create_desktop_shortcut(name: &str, target: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let desktop = dirs::desktop_dir().ok_or("No se pudo resolver el Escritorio")?;
    let lnk = desktop.join(format!("{}.lnk", name));
    let working = target.parent().unwrap_or(target);

    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.WorkingDirectory='{}'; $s.Description='{}'; $s.Save()",
        lnk.display(),
        target.display(),
        working.display(),
        name
    );

    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err("PowerShell no pudo crear el acceso directo".into()) }
}

#[cfg(not(windows))]
fn create_desktop_shortcut(_name: &str, _target: &Path) -> Result<(), String> {
    Ok(())
}

/// Opens the emulator's install folder in Explorer — pairs with the path
/// shown in the UI so "where did it install?" is one click, not a hunt
/// through AppData.
pub fn open_install_folder(id: &str) -> Result<(), String> {
    let dir = install_dir_for(id).ok_or("No se pudo resolver AppData")?;
    if !dir.exists() {
        return Err("Ese emulador no está instalado.".to_string());
    }
    opener::open(&dir).map_err(|e| e.to_string())
}

/// Reads the user's key files out of an existing install so an update can
/// wipe the folder without losing them.
fn stash_keys(install_dir: &Path, def: &EmulatorDef) -> Vec<(String, Vec<u8>)> {
    // Nothing to stash for a console with no key files.
    let Some(rel) = def.portable_keys_dir else { return Vec::new() };
    let keys_dir = install_dir.join(rel);
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&keys_dir) {
        for entry in entries.flatten() {
            if entry.path().is_file() {
                if let Ok(bytes) = fs::read(entry.path()) {
                    out.push((entry.file_name().to_string_lossy().to_string(), bytes));
                }
            }
        }
    }
    out
}

fn restore_keys(install_dir: &Path, def: &EmulatorDef, files: Vec<(String, Vec<u8>)>) {
    if files.is_empty() {
        return;
    }
    // Nothing to place for a console with no key files.
    let Some(rel) = def.portable_keys_dir else { return };
    let keys_dir = install_dir.join(rel);
    if fs::create_dir_all(&keys_dir).is_err() {
        return;
    }
    for (name, bytes) in files {
        let _ = fs::write(keys_dir.join(name), bytes);
    }
}

pub fn uninstall_emulator(id: &str) -> Result<(), String> {
    let install_dir = install_dir_for(id).ok_or("No se pudo resolver AppData")?;
    if !install_dir.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&install_dir)
        .map_err(|e| format!("No se pudo borrar la carpeta del emulador (¿está abierto?): {}", e))
}

/// When each emulator was last launched from here.
///
/// Exists only to close the double-click race described in
/// `launch_emulator`; the process table is still the source of truth for
/// anything older than the cooldown.
fn recent_launches() -> &'static Mutex<HashMap<String, Instant>> {
    static RECENT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    RECENT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True if the executable we are about to run already has a live process.
///
/// Matched on the file name of the exe we actually resolved rather than a
/// hardcoded one, so a fork that renames its binary is still recognised.
fn emulator_already_running(exe: &Path) -> bool {
    use sysinfo::System;
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let mut sys = System::new();
    sys.refresh_processes();
    sys.processes()
        .values()
        .any(|p| p.name().eq_ignore_ascii_case(name))
}

/// Extra arguments an emulator needs in order to open a usable window.
///
/// shadPS4 stopped shipping a GUI in its own repository — the only Windows
/// asset it publishes now is the SDL build, which is a command-line program.
/// Double-clicking it answers with a dialog saying so and nothing else, which
/// is what a user reported: the emulator installed fine and then refused to
/// open. `-b` is the flag that dialog itself points at, and brings up its Big
/// Picture interface.
///
/// The full desktop GUI moved to shadps4-emu/shadps4-qtlauncher and is a
/// separate download, not bundled here.
fn launch_args_for(id: &str) -> &'static [&'static str] {
    match id {
        "shadps4" => &["-b"],
        _ => &[],
    }
}

#[cfg(test)]
mod launch_args_tests {
    use super::launch_args_for;

    #[test]
    fn shadps4_asks_for_its_gui() {
        assert_eq!(launch_args_for("shadps4"), &["-b"]);
    }

    /// Every other emulator opens its own window unaided; passing a flag they
    /// do not recognise is how you turn a working launch into an error.
    #[test]
    fn the_others_are_launched_bare() {
        for id in ["rpcs3", "pcsx2", "ryujinx", "eden"] {
            assert!(launch_args_for(id).is_empty(), "{} deberia ir sin argumentos", id);
        }
    }
}

pub fn launch_emulator(id: &str) -> Result<(), String> {
    let def = def_for(id).ok_or_else(|| format!("Emulador desconocido: {}", id))?;
    let install_dir = install_dir_for(id).ok_or("No se pudo resolver AppData")?;
    let exe = find_exe(&install_dir, def.exe_stem)
        .ok_or_else(|| format!("{} no está instalado.", def.name))?;

    // A second instance does not just duplicate a window — it dies. Ryujinx
    // rewrites its own Config.json during startup (ReloadConfig -> SaveConfig
    // -> File.Create), and while the first instance holds that file the new
    // process throws an unhandled IOException and never opens:
    //
    //   The process cannot access the file '...\Ryujinx\Config.json'
    //   because it is being used by another process.
    //
    // Reported as "the emulator crashes when I press open". Nothing was
    // stopping the duplicate launch, so every extra click hit this.
    // Already-open counts as success: the user asked for the emulator, and
    // the emulator is there.
    // Checking the process table is necessary but not sufficient. The launch
    // button has no busy state, so a double click issues two commands at
    // once, and the second can reach this check before Windows has even
    // created the first process — landing right back on the crash above.
    // Recording the launch under the same lock that performs the check makes
    // the pair atomic, so the second call always sees the first.
    const LAUNCH_COOLDOWN: Duration = Duration::from_secs(10);
    {
        let mut recent = recent_launches()
            .lock()
            .map_err(|_| "El registro de lanzamientos quedó en mal estado.".to_string())?;

        if recent.get(id).map(|t| t.elapsed() < LAUNCH_COOLDOWN).unwrap_or(false) {
            return Ok(());
        }
        if emulator_already_running(&exe) {
            return Ok(());
        }
        recent.insert(id.to_string(), Instant::now());
    }

    let spawned = std::process::Command::new(&exe)
        .args(launch_args_for(id))
        .current_dir(exe.parent().unwrap_or(&install_dir))
        // The emulator logs to stdout for as long as it runs. Inherited
        // handles pour all of that into Ragnarok's own console during
        // `tauri dev`, which is how the crash above surfaced buried in
        // emulator noise, and leave pipes attached to a child we never wait
        // on.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    if let Err(e) = spawned {
        // Nothing was started, so the cooldown must not stand — otherwise a
        // failed launch would silently swallow the user's next 10 seconds of
        // clicks and look like the button does nothing.
        if let Ok(mut recent) = recent_launches().lock() {
            recent.remove(id);
        }
        return Err(format!("No se pudo abrir {}: {}", def.name, e));
    }
    Ok(())
}

/// Copies a key file the user already has into the folders the emulator
/// reads them from. Written to both the portable and roaming locations so it
/// works regardless of which mode the emulator ends up running in.
///
/// Only accepts the two key filenames — this is a "put my file where it
/// goes" helper, not a general file dropper into an executable's directory.
pub fn install_keys(emulator_id: &str, source_path: &str) -> Result<String, String> {
    let def = def_for(emulator_id).ok_or_else(|| format!("Emulador desconocido: {}", emulator_id))?;
    let src = Path::new(source_path);
    if !src.is_file() {
        return Err("El archivo seleccionado no existe.".to_string());
    }
    let fname = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if fname != "prod.keys" && fname != "title.keys" {
        return Err("Elegí un archivo prod.keys o title.keys.".to_string());
    }

    let bytes = fs::read(src).map_err(|e| format!("No se pudo leer el archivo: {}", e))?;

    let install_dir = install_dir_for(emulator_id).ok_or("No se pudo resolver AppData")?;
    let Some(portable) = def.portable_keys_dir else {
        return Err(format!(
            "{} no usa archivos de claves — no hay nada que colocar.",
            def.name
        ));
    };
    let mut targets: Vec<PathBuf> = vec![install_dir.join(portable)];
    if let Some(appdata) = dirs::data_dir() {
        if let Some(roaming) = def.appdata_keys_dir {
            targets.push(appdata.join(roaming));
        }
        if emulator_id == "eden" {
            targets.push(appdata.join("yuzu/keys"));
        }
    }
    if emulator_id == "ryubing" {
        targets.push(install_dir.join("publish/portable/system"));
    }

    let mut written = 0usize;
    let mut last_err = String::new();
    for dir in &targets {
        if let Err(e) = fs::create_dir_all(dir) {
            last_err = e.to_string();
            continue;
        }
        match fs::write(dir.join(&fname), &bytes) {
            Ok(()) => written += 1,
            Err(e) => last_err = e.to_string(),
        }
    }

    if written == 0 {
        return Err(format!("No se pudo copiar {}: {}", fname, last_err));
    }
    Ok(format!("{} colocado en {} ubicación(es) de {}.", fname, written, def.name))
}

/// Same as `install_keys` but accepts the key data as an in-memory slice, so
/// the download path can avoid writing a temporary file to disk at all.
/// `key_files` is a list of `(filename, bytes)` pairs; only `prod.keys` and
/// `title.keys` are accepted (the same constraint as `install_keys`).
pub fn install_keys_from_bytes(emulator_id: &str, key_files: &[(String, Vec<u8>)]) -> Result<String, String> {
    let def = def_for(emulator_id).ok_or_else(|| format!("Emulador desconocido: {}", emulator_id))?;

    let install_dir = install_dir_for(emulator_id).ok_or("No se pudo resolver AppData")?;
    let Some(portable) = def.portable_keys_dir else {
        return Err(format!(
            "{} no usa archivos de claves — no hay nada que colocar.",
            def.name
        ));
    };
    let mut targets: Vec<PathBuf> = vec![install_dir.join(portable)];
    if let Some(appdata) = dirs::data_dir() {
        if let Some(roaming) = def.appdata_keys_dir {
            targets.push(appdata.join(roaming));
        }
        if emulator_id == "eden" {
            targets.push(appdata.join("yuzu/keys"));
        }
    }
    if emulator_id == "ryubing" {
        targets.push(install_dir.join("publish/portable/system"));
    }

    // Counted as two separate things, because they answer two different
    // questions: which key files got installed, and how many folders they
    // reached. The message used to divide one write count by the length of
    // the whole input list — including entries that were skipped for not
    // being a key file — so a zip carrying a readme alongside the keys
    // produced "instaladas correctamente en 0 ubicación(es)", a sentence
    // that contradicts itself.
    let mut installed_names: Vec<String> = Vec::new();
    let mut dirs_reached: Vec<&PathBuf> = Vec::new();
    let mut failed_writes = 0usize;
    let mut last_err = String::new();

    for (fname, bytes) in key_files {
        let fname_lower = fname.to_lowercase();
        if fname_lower != "prod.keys" && fname_lower != "title.keys" {
            continue;
        }
        let mut wrote_this_file = false;
        for dir in &targets {
            if let Err(e) = fs::create_dir_all(dir) {
                last_err = e.to_string();
                failed_writes += 1;
                continue;
            }
            match fs::write(dir.join(&fname_lower), bytes) {
                Ok(()) => {
                    wrote_this_file = true;
                    if !dirs_reached.iter().any(|d| *d == dir) {
                        dirs_reached.push(dir);
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                    failed_writes += 1;
                }
            }
        }
        if wrote_this_file && !installed_names.contains(&fname_lower) {
            installed_names.push(fname_lower);
        }
    }

    if installed_names.is_empty() {
        if last_err.is_empty() {
            return Err("El archivo no contenía prod.keys ni title.keys.".to_string());
        }
        return Err(format!("No se pudo instalar las keys: {}", last_err));
    }

    let mut msg = format!(
        "{} instalada(s) en {} carpeta(s) de {}.",
        installed_names.join(" y "),
        dirs_reached.len(),
        def.name
    );
    // A partial success is still a success worth reporting, but silently
    // dropping the half that failed is how a user ends up with keys in the
    // portable folder and an emulator reading the roaming one.
    if failed_writes > 0 {
        msg.push_str(&format!(
            " {} escritura(s) fallaron: {}",
            failed_writes, last_err
        ));
    }
    Ok(msg)
}

fn extract_nca_id(full_name: &str) -> Option<String> {
    let normalized = full_name.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let last = parts.last().unwrap();
    if *last == "00" && parts.len() >= 2 {
        let parent = parts[parts.len() - 2];
        if parent.to_lowercase().ends_with(".nca") {
            return Some(parent.to_string());
        }
    }
    if last.to_lowercase().ends_with(".nca") {
        return Some(last.to_string());
    }
    None
}

/// Installs a firmware archive the USER already has by extracting its NCA
/// files into the folder the emulator reads firmware from.
///
/// Note on emulator storage formats:
/// - Ryujinx (bis/system/Contents/registered) enumerates DIRECTORIES, not files.
///   Each NCA must be placed in a subdirectory named `<nca_name>/00`.
/// - Eden / Yuzu (nand/system/Contents/registered) expects flat `.nca` files.
/// Both ZIP and 7Z archives are supported.
pub fn install_firmware(emulator_id: &str, source_path: &str) -> Result<String, String> {
    let def = def_for(emulator_id).ok_or_else(|| format!("Emulador desconocido: {}", emulator_id))?;
    let src = Path::new(source_path);
    if !src.is_file() {
        return Err("El archivo seleccionado no existe.".to_string());
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if ext != "zip" && ext != "7z" {
        return Err("Elegí el archivo .zip o .7z del firmware.".to_string());
    }

    let mut nca_files: Vec<(String, Vec<u8>)> = Vec::new();

    if ext == "zip" {
        let bytes = fs::read(src).map_err(|e| format!("No se pudo leer el archivo: {}", e))?;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!("El firmware no es un ZIP válido: {}", e))?;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let Some(nca_name) = extract_nca_id(entry.name()) else { continue };
            let mut buf = Vec::new();
            std::io::copy(&mut entry, &mut buf).map_err(|e| e.to_string())?;
            nca_files.push((nca_name, buf));
        }
    } else if ext == "7z" {
        let tmp_extract = std::env::temp_dir().join(format!(
            "ragnarok_fw_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        let _ = fs::create_dir_all(&tmp_extract);
        let res = sevenz_rust::decompress_file(src, &tmp_extract);
        if let Err(e) = res {
            let _ = fs::remove_dir_all(&tmp_extract);
            return Err(format!("No se pudo descomprimir el archivo 7z de firmware: {}", e));
        }
        for entry in walkdir::WalkDir::new(&tmp_extract).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                let rel = entry.path().strip_prefix(&tmp_extract).unwrap_or(entry.path());
                if let Some(nca_name) = extract_nca_id(&rel.to_string_lossy()) {
                    if let Ok(buf) = fs::read(entry.path()) {
                        nca_files.push((nca_name, buf));
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&tmp_extract);
    }

    if nca_files.is_empty() {
        return Err("Ese archivo no contiene archivos .nca — no parece un firmware de Switch.".to_string());
    }

    struct TargetDir {
        path: PathBuf,
        is_ryujinx: bool,
    }
    let mut targets: Vec<TargetDir> = Vec::new();
    let install_dir = install_dir_for(emulator_id).ok_or("No se pudo resolver AppData")?;
    let appdata = dirs::data_dir();

    if emulator_id == "ryubing" {
        if let Some(ref ad) = appdata {
            targets.push(TargetDir {
                path: ad.join("Ryujinx/bis/system/Contents/registered"),
                is_ryujinx: true,
            });
        }
        targets.push(TargetDir {
            path: install_dir.join("portable/bis/system/Contents/registered"),
            is_ryujinx: true,
        });
        targets.push(TargetDir {
            path: install_dir.join("publish/portable/bis/system/Contents/registered"),
            is_ryujinx: true,
        });

        // Also sync Eden if installed so both work
        if let Some(ref ad) = appdata {
            if ad.join("eden").exists() {
                targets.push(TargetDir {
                    path: ad.join("eden/nand/system/Contents/registered"),
                    is_ryujinx: false,
                });
            }
        }
    } else if emulator_id == "eden" {
        if let Some(ref ad) = appdata {
            targets.push(TargetDir {
                path: ad.join("eden/nand/system/Contents/registered"),
                is_ryujinx: false,
            });
            if ad.join("yuzu").exists() {
                targets.push(TargetDir {
                    path: ad.join("yuzu/nand/system/Contents/registered"),
                    is_ryujinx: false,
                });
            }
        }
        targets.push(TargetDir {
            path: install_dir.join("user/nand/system/Contents/registered"),
            is_ryujinx: false,
        });

        // Also sync Ryubing if installed so both work
        if let Some(ref ad) = appdata {
            if ad.join("Ryujinx").exists() {
                targets.push(TargetDir {
                    path: ad.join("Ryujinx/bis/system/Contents/registered"),
                    is_ryujinx: true,
                });
            }
        }
    } else {
        if let Some(portable) = def.portable_firmware_dir {
            targets.push(TargetDir {
                path: install_dir.join(portable),
                is_ryujinx: false,
            });
        }
        if let Some(ref ad) = appdata {
            if let Some(roaming) = def.appdata_firmware_dir {
                targets.push(TargetDir {
                    path: ad.join(roaming),
                    is_ryujinx: false,
                });
            }
        }
    }

    for target in &targets {
        let _ = fs::create_dir_all(&target.path);
    }

    let mut installed = 0usize;
    let mut failed = 0usize;
    let mut first_error: Option<String> = None;

    for (nca_name, buf) in &nca_files {
        let mut wrote_this_file = false;
        for target in &targets {
            if target.is_ryujinx {
                // Ryujinx requires each NCA in its own subfolder named <nca_name>, containing a file named "00"
                // e.g. bis/system/Contents/registered/0100000000000809.nca/00
                let nca_dir = target.path.join(nca_name);
                if nca_dir.is_file() {
                    let _ = fs::remove_file(&nca_dir);
                }
                if let Err(e) = fs::create_dir_all(&nca_dir) {
                    failed += 1;
                    if first_error.is_none() {
                        first_error = Some(format!("{}: {}", nca_dir.display(), e));
                    }
                    continue;
                }
                match fs::write(nca_dir.join("00"), buf) {
                    Ok(()) => wrote_this_file = true,
                    Err(e) => {
                        failed += 1;
                        if first_error.is_none() {
                            first_error = Some(format!("{}: {}", nca_dir.join("00").display(), e));
                        }
                    }
                }
            } else {
                // Eden / Yuzu expects flat .nca files directly inside registered/
                let nca_file = target.path.join(nca_name);
                if nca_file.is_dir() {
                    let _ = fs::remove_dir_all(&nca_file);
                }
                match fs::write(&nca_file, buf) {
                    Ok(()) => wrote_this_file = true,
                    Err(e) => {
                        failed += 1;
                        if first_error.is_none() {
                            first_error = Some(format!("{}: {}", nca_file.display(), e));
                        }
                    }
                }
            }
        }
        if wrote_this_file {
            installed += 1;
        }
    }

    if installed == 0 {
        return Err(format!(
            "No se pudo escribir ningún archivo de firmware en {}. ¿Está el emulador abierto, o la carpeta es de solo lectura?{}",
            def.name,
            first_error.map(|e| format!(" ({})", e)).unwrap_or_default()
        ));
    }
    if failed > 0 {
        return Ok(format!(
            "Firmware instalado en {} ({} de {} archivos; {} escritura(s) fallaron).",
            def.name, installed, nca_files.len(), failed
        ));
    }
    Ok(format!("Firmware instalado en {} ({} archivos).", def.name, installed))
}

const GAME_EXTENSIONS: &[&str] = &["nsp", "xci", "nsz", "xcz"];

/// Pulls the `[0100XXXXXXXXXXXX]` title id out of a dump's filename. Switch
/// title ids are 16 hex digits and application ids always end in "000".
fn parse_title_id(filename: &str) -> Option<String> {
    let bytes = filename.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = filename[i + 1..].find(']') {
                let inner = &filename[i + 1..i + 1 + close];
                if inner.len() == 16 && inner.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(inner.to_uppercase());
                }
                i += close + 1;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Strips the bracketed metadata tags dumps carry — `[0100…][v0][US]` — plus
/// the extension, leaving something a person would recognise as the title.
fn clean_game_name(filename: &str) -> String {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let mut out = String::new();
    let mut depth = 0i32;
    for ch in stem.chars() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    let cleaned = out.replace('_', " ").replace('.', " ");
    let collapsed: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() { stem.to_string() } else { collapsed }
}

/// Lists Switch game files already present in a folder the user points at.
/// Read-only: nothing is downloaded, moved or modified.
pub fn scan_games(folder: &str) -> Result<Vec<SwitchGame>, String> {
    let root = Path::new(folder);
    if !root.is_dir() {
        return Err("La carpeta seleccionada no existe.".to_string());
    }
    let mut games = Vec::new();
    for entry in walkdir::WalkDir::new(root).max_depth(3).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => continue,
        };
        if !GAME_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        games.push(SwitchGame {
            title_id: parse_title_id(filename),
            name: clean_game_name(filename),
            path: path.to_string_lossy().to_string(),
            size_bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
            format: ext.to_uppercase(),
        });
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(games)
}

/// Folders that never hold games and cost real time to walk. Pruned rather
/// than filtered so walkdir doesn't descend into them at all.
const SCAN_SKIP: &[&str] = &[
    "windows", "program files", "program files (x86)", "programdata",
    "$recycle.bin", "system volume information", "appdata", "node_modules",
    ".git", "recovery", "msocache", "onedrivetemp",
];

/// Scans every fixed drive for Switch game files, bounded to a shallow depth
/// and with the noisy system folders pruned — a full recursive walk of a
/// multi-terabyte drive would take minutes, which is not a thing to do
/// behind a spinner.
pub fn autodetect_games() -> Vec<SwitchGame> {
    let mut games = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for letter in 'A'..='Z' {
        let root = PathBuf::from(format!("{}:\\", letter));
        if !root.is_dir() {
            continue;
        }
        let walker = walkdir::WalkDir::new(&root)
            .max_depth(4)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with('.') && e.file_type().is_dir() {
                    return false;
                }
                !SCAN_SKIP.contains(&name.as_str())
            });

        for entry in walker.flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };
            if !GAME_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }
            let key = path.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            games.push(SwitchGame {
                title_id: parse_title_id(filename),
                name: clean_game_name(filename),
                path: path.to_string_lossy().to_string(),
                size_bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
                format: ext.to_uppercase(),
            });
        }
    }

    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}

/// Adds `folder` to the emulator's own game-directory list so the games show
/// up inside it without the user configuring anything by hand.
///
/// Both configs are edited in place rather than rewritten: Ryujinx's
/// Config.json carries a schema `version` plus every other setting, and
/// Eden's qt-config.ini is ~1300 lines of Qt state — authoring either from
/// scratch would silently reset the user's whole configuration.
pub fn register_games_dir(emulator_id: &str, folder: &str) -> Result<String, String> {
    let def = def_for(emulator_id).ok_or_else(|| format!("Emulador desconocido: {}", emulator_id))?;
    if !Path::new(folder).is_dir() {
        return Err("Esa carpeta no existe.".to_string());
    }

    match emulator_id {
        "ryubing" => register_ryujinx_dir(folder, def),
        "eden" => register_eden_dir(folder, def),
        _ => Err(format!("No sé configurar {} todavía.", def.name)),
    }
}

/// Writes a config file atomically: full contents into a sibling temp file
/// first, then a single rename over the original.
///
/// A plain `fs::write` truncates the target before it writes a byte, so an
/// interruption at the wrong moment (the process exiting, a full disk, an
/// antivirus grabbing the handle) leaves a half-written file behind. For
/// these two files that is not a small loss: Ryujinx's Config.json holds
/// every setting the user has, and Eden's qt-config.ini is ~1300 lines of Qt
/// state — a truncated one makes the emulator start over from defaults, with
/// the user's whole configuration gone. The rename is atomic and the temp
/// file is a sibling, so it is always on the same volume.
fn write_config_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let mut tmp_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .ok_or_else(|| "Ruta de configuracion invalida.".to_string())?;
    tmp_name.push(".ragnarok-tmp");
    let tmp = path.with_file_name(tmp_name);

    fs::write(&tmp, contents)
        .map_err(|e| format!("No se pudo guardar la configuracion: {}", e))?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // The original is still intact — the rename never touched it.
            let _ = fs::remove_file(&tmp);
            Err(format!("No se pudo guardar la configuracion: {}", e))
        }
    }
}

/// Candidate config paths, roaming first then portable — whichever exists is
/// the one the emulator is actually using.
fn config_candidates(def: &EmulatorDef, install_dir: &Path, rel_appdata: &str, rel_portable: &str) -> Vec<PathBuf> {
    let _ = def;
    let mut out = Vec::new();
    if let Some(appdata) = dirs::data_dir() {
        out.push(appdata.join(rel_appdata));
    }
    out.push(install_dir.join(rel_portable));
    out
}

fn register_ryujinx_dir(folder: &str, def: &EmulatorDef) -> Result<String, String> {
    let install_dir = install_dir_for(def.id).ok_or("No se pudo resolver AppData")?;
    let candidates = config_candidates(def, &install_dir, "Ryujinx/Config.json", "portable/Config.json");

    let path = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| format!("Abrí {} una vez primero para que cree su configuración.", def.name))?;

    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("La configuración de {} no es un JSON válido: {}", def.name, e))?;

    let dirs_arr = json
        .get_mut("game_dirs")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "La configuración no tiene 'game_dirs'.".to_string())?;

    if dirs_arr.iter().any(|v| v.as_str().map(|s| s.eq_ignore_ascii_case(folder)).unwrap_or(false)) {
        return Ok(format!("Esa carpeta ya estaba configurada en {}.", def.name));
    }
    dirs_arr.push(serde_json::Value::String(folder.to_string()));

    let out = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    write_config_atomic(&path, &out)?;
    Ok(format!("Carpeta agregada a {}. Reabrilo para ver los juegos.", def.name))
}

fn register_eden_dir(folder: &str, def: &EmulatorDef) -> Result<String, String> {
    let install_dir = install_dir_for(def.id).ok_or("No se pudo resolver AppData")?;
    let candidates = config_candidates(
        def,
        &install_dir,
        "eden/config/qt-config.ini",
        "user/config/qt-config.ini",
    );

    let path = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| format!("Abrí {} una vez primero para que cree su configuración.", def.name))?;

    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    // Qt writes paths with forward slashes; matching that keeps the file
    // consistent and avoids a duplicate entry for the same folder.
    let qt_folder = folder.replace('\\', "/");
    if text.lines().any(|l| {
        l.starts_with("Paths\\gamedirs\\")
            && l.contains("\\path=")
            && l.split('=').nth(1).map(|v| v.eq_ignore_ascii_case(&qt_folder)).unwrap_or(false)
    }) {
        return Ok(format!("Esa carpeta ya estaba configurada en {}.", def.name));
    }

    let size_prefix = "Paths\\gamedirs\\size=";
    let current_size: usize = text
        .lines()
        .find(|l| l.starts_with(size_prefix))
        .and_then(|l| l[size_prefix.len()..].trim().parse().ok())
        .ok_or_else(|| "No se encontró 'Paths\\gamedirs\\size' en la configuración.".to_string())?;
    let new_index = current_size + 1;

    let mut out: Vec<String> = Vec::with_capacity(text.lines().count() + 6);
    let mut last_gamedir_line = 0usize;
    for (i, line) in text.lines().enumerate() {
        if line.starts_with("Paths\\gamedirs\\") && !line.starts_with(size_prefix) {
            last_gamedir_line = i;
        }
        out.push(line.to_string());
    }
    if last_gamedir_line == 0 {
        return Err("No se encontró la lista de carpetas en la configuración.".to_string());
    }

    // Bump the declared count, then append the new entry right after the
    // existing block so the numbering stays contiguous.
    for line in out.iter_mut() {
        if line.starts_with(size_prefix) {
            *line = format!("{}{}", size_prefix, new_index);
            break;
        }
    }
    let new_entry = vec![
        format!("Paths\\gamedirs\\{}\\path={}", new_index, qt_folder),
        format!("Paths\\gamedirs\\{}\\deep_scan\\default=false", new_index),
        format!("Paths\\gamedirs\\{}\\deep_scan=true", new_index),
        format!("Paths\\gamedirs\\{}\\expanded\\default=true", new_index),
        format!("Paths\\gamedirs\\{}\\expanded=true", new_index),
    ];
    for (offset, entry) in new_entry.into_iter().enumerate() {
        out.insert(last_gamedir_line + 1 + offset, entry);
    }

    write_config_atomic(&path, &out.join("\r\n"))?;
    Ok(format!("Carpeta agregada a {}. Reabrilo para ver los juegos.", def.name))
}

/// Cover art from Nintendo's own public eShop search — the same kind of
/// per-title metadata lookup the Library already does against Steam's store
/// API, and the reason this doesn't need the 85 MB community title database.
/// None when nothing matches, so the UI falls back to a plain tile.
pub async fn fetch_cover(client: &reqwest::Client, name: &str) -> Option<String> {
    let query = urlencoding::encode(name);
    let url = format!(
        "https://searching.nintendo-europe.com/en/select?q={}&fq=type:GAME%20AND%20system_type:nintendoswitch*&rows=1&wt=json",
        query
    );
    let res = client.get(&url).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let json: serde_json::Value = res.json().await.ok()?;
    let img = json["response"]["docs"][0]["image_url"].as_str()?;
    if img.is_empty() {
        return None;
    }
    // The API returns protocol-relative URLs for some entries.
    Some(if img.starts_with("//") { format!("https:{}", img) } else { img.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real end-to-end install against the live servers — downloads the
    /// actual release and asserts an executable came out the other end.
    /// Network-dependent by design: the whole class of bug this feature hit
    /// (a host answering the API path with an HTML page based on the
    /// User-Agent alone) is invisible to any mocked test.
    ///
    /// Run with: cargo test emulators -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn installs_both_emulators() {
        for def in EMULATORS {
            let client = api_client().expect("client");
            let (tag, url) = fetch_latest_release(&client, def)
                .await
                .unwrap_or_else(|e| panic!("{}: no se pudo leer el release: {}", def.name, e));
            println!("{}: release {} -> {}", def.name, tag, url);

            let installed_tag = install_emulator(None, def.id)
                .await
                .unwrap_or_else(|e| panic!("{}: la instalación falló: {}", def.name, e));
            assert_eq!(installed_tag, tag);

            let dir = install_dir_for(def.id).expect("install dir");
            let exe = find_exe(&dir, def.exe_stem)
                .unwrap_or_else(|| panic!("{}: no quedó ningún .exe tras instalar", def.name));
            println!("{}: OK -> {}", def.name, exe.display());

            // Regression guard: Eden ships eden.exe next to eden-cli.exe, and
            // an earlier version of find_exe returned whichever walkdir hit
            // first — pointing the shortcut at the console build.
            let stem = exe.file_stem().unwrap().to_string_lossy().to_lowercase();
            assert!(
                !stem.contains("cli") && !stem.contains("cmd"),
                "{}: eligió el binario de consola ({}) en vez del emulador",
                def.name,
                stem
            );

            let marker = fs::read_to_string(version_marker(&dir)).expect("version marker");
            assert_eq!(marker.trim(), tag);
        }
    }

    /// Writes into the emulators' REAL config files, then verifies the folder
    /// is actually listed and the file still parses. Both configs carry the
    /// user's entire setup, so "did we corrupt it?" is the thing worth
    /// testing, not just "did the function return Ok".
    ///
    /// Run with: cargo test registers_games_dir -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn registers_games_dir() {
        let folder = std::env::temp_dir().join("ragnarok_switch_test_games");
        fs::create_dir_all(&folder).expect("temp games dir");
        let folder_str = folder.to_string_lossy().to_string();

        for def in EMULATORS {
            match register_games_dir(def.id, &folder_str) {
                Ok(msg) => println!("{}: {}", def.name, msg),
                Err(e) => {
                    println!("{}: omitido ({})", def.name, e);
                    continue;
                }
            }

            // Re-read and confirm the folder really is in there now.
            let install_dir = install_dir_for(def.id).unwrap();
            if def.id == "ryubing" {
                let path = config_candidates(def, &install_dir, "Ryujinx/Config.json", "portable/Config.json")
                    .into_iter().find(|p| p.is_file()).unwrap();
                let json: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("config sigue siendo JSON válido");
                let listed = json["game_dirs"].as_array().unwrap().iter()
                    .any(|v| v.as_str() == Some(folder_str.as_str()));
                assert!(listed, "Ryubing: la carpeta no quedó en game_dirs");
            } else {
                let path = config_candidates(def, &install_dir, "eden/config/qt-config.ini", "user/config/qt-config.ini")
                    .into_iter().find(|p| p.is_file()).unwrap();
                let text = fs::read_to_string(&path).unwrap();
                let qt = folder_str.replace('\\', "/");
                assert!(text.contains(&format!("path={}", qt)), "Eden: la carpeta no quedó en el ini");
                // size must match the highest index actually present.
                let size: usize = text.lines().find(|l| l.starts_with("Paths\\gamedirs\\size="))
                    .unwrap().split('=').nth(1).unwrap().trim().parse().unwrap();
                let max_idx = text.lines()
                    .filter_map(|l| l.strip_prefix("Paths\\gamedirs\\"))
                    .filter_map(|r| r.split('\\').next())
                    .filter_map(|n| n.parse::<usize>().ok())
                    .max().unwrap_or(0);
                assert_eq!(size, max_idx, "Eden: 'size' no coincide con las entradas escritas");
            }

            // Idempotent: running it twice must not add a duplicate.
            let second = register_games_dir(def.id, &folder_str).expect("segunda pasada");
            assert!(second.contains("ya estaba"), "{}: no detectó el duplicado ({})", def.name, second);
            println!("{}: sin duplicados OK", def.name);
        }

        let _ = fs::remove_dir_all(&folder);
    }
}
