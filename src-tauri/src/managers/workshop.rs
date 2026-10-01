// Steam Workshop mod installer that doesn't depend on the real Steam client's
// own Workshop subscribe flow — needed because every game this launcher
// touches runs under an emulated/spoofed Steamworks layer (SteamTools ticket
// injection or Goldberg), so ISteamUGC subscription either isn't backed by a
// real account or isn't wired up consistently across unlockers. Downloads the
// raw item content from a third-party mirror and drops it into
// `steamapps/workshop/content/<appid>/<itemid>/` (same path Steam's own
// client uses) — but for Source engine games specifically (confirmed against
// a real L4D2 test), that alone is NOT enough for the game to load it: the
// content has to also be mirrored into the game's own <moddir>/addons/
// folder, which Source engine mounts directly at startup regardless of any
// Steam subscription state. That folder — not Steam's own Properties >
// Workshop tab, which only ever reflects the real account's live/cached
// subscriptions and can't be made to show a locally-added item — is what
// actually determines whether the mod works in-game. Also confirmed: this
// only works while Steam is offline — with a live connection the game
// appears to validate content against the account's real subscription list
// and won't load an unrecognized local file, which is outside what this
// feature can or should try to work around. Same "lean on a community-run
// generator, never a hard dependency" posture as Ryuu's ticket generator
// elsewhere in this app: if the mirror is down, the caller just gets an
// error, nothing else breaks.

use serde::{Deserialize, Serialize};
use crate::diag_log;
use super::download_guard;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct WorkshopItemInfo {
    pub item_id: String,
    pub app_id: String,
    pub title: String,
    pub preview_url: String,
    pub file_size: u64,
}

/// Accepts either a bare numeric published file id, or a full Workshop URL
/// like `https://steamcommunity.com/sharedfiles/filedetails/?id=1234567890`.
pub fn parse_workshop_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }
    let pos = trimmed.find("id=")?;
    let rest = &trimmed[pos + 3..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// Resolves a Workshop item id to its title, owning AppID, preview image and
/// size via Steam's own public (keyless) Web API — this part talks to real
/// Steam infrastructure and is fully reliable.
pub async fn resolve_workshop_item(client: &reqwest::Client, item_id: &str) -> Result<WorkshopItemInfo, String> {
    let url = "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
    let params = [("itemcount", "1"), ("publishedfileids[0]", item_id)];
    let res = client
        .post(url)
        .form(&params)
        .timeout(std::time::Duration::from_secs(45))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let details = &json["response"]["publishedfiledetails"][0];
    let result = details["result"].as_i64().unwrap_or(0);
    if result != 1 {
        return Err("No se encontró ese ítem de Steam Workshop — revisá el link.".to_string());
    }
    let app_id = details["consumer_app_id"]
        .as_u64()
        .or_else(|| details["creator_app_id"].as_u64())
        .ok_or_else(|| "Steam no informó a qué juego pertenece este ítem.".to_string())?;
    let file_size = details["file_size"]
        .as_u64()
        .or_else(|| details["file_size"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    Ok(WorkshopItemInfo {
        item_id: item_id.to_string(),
        app_id: app_id.to_string(),
        title: details["title"].as_str().unwrap_or("").to_string(),
        preview_url: details["preview_url"].as_str().unwrap_or("").to_string(),
        file_size,
    })
}

/// Downloads the item's raw content bytes. Tries Valve's own SteamCMD first
/// (anonymous `workshop_download_item` — confirmed live to work for Garry's
/// Mod without owning it, and it's the official mechanism so there's no
/// ad-gate/shortener/HTML-page nonsense to work around); falls back to the
/// GGNetwork mirror only if SteamCMD itself fails (a game that genuinely
/// requires an owning account, or SteamCMD's own download failing) — same
/// "never a hard dependency on either" posture the rest of this app already
/// uses for Ryuu's ticket generator.
pub async fn download_workshop_item(client: &reqwest::Client, app_id: &str, item_id: &str) -> Result<Vec<u8>, String> {
    match download_via_steamcmd(app_id, item_id).await {
        Ok(bytes) => Ok(bytes),
        Err(steamcmd_err) => match download_via_mirror(client, item_id).await {
            Ok(bytes) => Ok(bytes),
            Err(mirror_err) => Err(format!("SteamCMD: {} — Mirror: {}", steamcmd_err, mirror_err)),
        },
    }
}

/// Valve's own official CLI tool, cached once under Ragnarok's own AppData
/// folder — never re-downloaded after the first successful run. steamcmd.zip
/// is the same stable installer every SteamCMD tutorial/game-server guide
/// links (verified reachable at time of writing); a fallback host is tried
/// if the primary one is unreachable.
fn steamcmd_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("steamcmd"))
}

async fn ensure_steamcmd(client: &reqwest::Client) -> Result<PathBuf, String> {
    let dir = steamcmd_dir().ok_or_else(|| "No se pudo determinar la carpeta de datos de la app.".to_string())?;
    let exe = dir.join("steamcmd.exe");
    if exe.exists() {
        return Ok(exe);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    const URLS: &[&str] = &[
        "https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip",
        "https://media.steampowered.com/installer/steamcmd.zip",
    ];
    // Valve's two CDN hosts, and nothing else. Both entries in URLS are
    // constants today; the check is what keeps that true if the list is ever
    // extended, since what arrives here gets extracted and executed.
    const STEAMCMD_HOSTS: &[&str] = &["steamcdn-a.akamaihd.net", "media.steampowered.com"];

    let mut bytes: Option<Vec<u8>> = None;
    let mut last_err: Option<String> = None;
    for url in URLS {
        if !download_guard::is_allowed_url(url, STEAMCMD_HOSTS) {
            last_err = Some(format!("URL de descarga no confiable: {}", url));
            continue;
        }
        if let Ok(res) = client.get(*url).send().await {
            if res.status().is_success() {
                if let Ok(b) = res.bytes().await {
                    // A mirror that answers with an error page, or a transfer
                    // that stopped early, used to reach ZipArchive::new and
                    // fail there with "invalid Zip archive" — accurate but
                    // useless. Rejected here with the actual reason instead.
                    match download_guard::verify_download(
                        "SteamCMD",
                        &b,
                        download_guard::Payload::Zip,
                        None,
                    ) {
                        Ok(()) => {
                            bytes = Some(b.to_vec());
                            break;
                        }
                        Err(e) => last_err = Some(e),
                    }
                }
            }
        }
    }
    let bytes = bytes.ok_or_else(|| {
        last_err.unwrap_or_else(|| {
            "No se pudo descargar SteamCMD desde ninguno de los servidores de Valve.".to_string()
        })
    })?;

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        // `enclosed_name()` rejects `..` and absolute paths. This was the last
        // extraction path in the project without it — every other one (the
        // bypass archives, the emulators, the save bundles) already guards
        // this way. The archive comes from Valve's own CDN so the practical
        // risk was low, but "we trust this host" is not a property the code
        // states anywhere, and it is one host change away from being wrong.
        let Some(safe_name) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            diag_log!("[steamcmd] Entrada con ruta insegura descartada: {}", name);
            continue;
        };
        let out_path = dir.join(&safe_name);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::new();
        std::io::copy(&mut entry, &mut buf).map_err(|e| e.to_string())?;
        std::fs::write(&out_path, buf).map_err(|e| e.to_string())?;
    }

    if !exe.exists() {
        return Err("El paquete descargado de SteamCMD no contenía steamcmd.exe.".to_string());
    }
    Ok(exe)
}

/// Runs `steamcmd +login anonymous +workshop_download_item <appid> <itemid>
/// +quit` and zips up whatever it wrote to its own
/// steamapps/workshop/content/<appid>/<itemid>/ so the caller can treat the
/// result identically to a mirror-sourced zip. SteamCMD's own first run does
/// a real self-update (can take up to a minute); a 5-minute timeout is
/// generous for that plus a normal-sized addon download.
async fn download_via_steamcmd(app_id: &str, item_id: &str) -> Result<Vec<u8>, String> {
    let client = crate::make_client()?;
    let exe = ensure_steamcmd(&client).await?;
    let dir = exe.parent().ok_or_else(|| "Ruta de SteamCMD inválida.".to_string())?.to_path_buf();
    let app_id_owned = app_id.to_string();
    let item_id_owned = item_id.to_string();

    let spawn = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(["+login", "anonymous", "+workshop_download_item", &app_id_owned, &item_id_owned, "+quit"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW — no console flicker
        }
        cmd.output()
    });

    let output = tokio::time::timeout(std::time::Duration::from_secs(300), spawn)
        .await
        .map_err(|_| "SteamCMD tardó más de 5 minutos — se canceló.".to_string())?
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    // Pulled into every failure message below instead of just a generic
    // "it didn't work" — SteamCMD's own stdout usually says exactly why
    // (e.g. "ERROR! Download item ... failed (No subscription)"), and
    // surfacing it beats guessing at the cause from outside.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let relevant: String = stdout
        .lines()
        .filter(|l| {
            let lower = l.to_ascii_lowercase();
            lower.contains("error") || lower.contains("fail") || lower.contains("success")
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let diagnostic = if relevant.trim().is_empty() {
        stdout.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ")
    } else {
        relevant
    };

    if !output.status.success() {
        return Err(format!("SteamCMD terminó con error (código {:?}): {}", output.status.code(), diagnostic));
    }

    let content_dir = dir.join("steamapps").join("workshop").join("content").join(app_id).join(item_id);
    if !content_dir.exists() {
        return Err(format!("SteamCMD no descargó nada: {}", diagnostic));
    }

    let zipped = zip_folder_to_bytes(&content_dir);

    // Only discard SteamCMD's own copy once the zip actually succeeded. It
    // used to be deleted unconditionally, so a zip that failed halfway (a
    // file locked by an antivirus, a full disk) threw away the only copy of
    // content that had just taken minutes to download — leaving the user to
    // fetch the whole item again for no reason. On failure the folder stays
    // put and the next attempt reuses it.
    if zipped.is_ok() {
        let _ = std::fs::remove_dir_all(&content_dir);
    }
    zipped
}

fn zip_folder_to_bytes(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let cursor = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    fn add_dir(
        zip: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>,
        base: &std::path::Path,
        current: &std::path::Path,
        options: zip::write::FileOptions,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(current).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let rel = path.strip_prefix(base).map_err(|e| e.to_string())?;
            let name = rel.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                zip.add_directory(&name, options).map_err(|e| e.to_string())?;
                add_dir(zip, base, &path, options)?;
            } else {
                zip.start_file(&name, options).map_err(|e| e.to_string())?;
                let data = std::fs::read(&path).map_err(|e| e.to_string())?;
                zip.write_all(&data).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    add_dir(&mut zip, dir, dir, options)?;
    let result = zip.finish().map_err(|e| e.to_string())?;
    Ok(result.into_inner())
}

/// Downloads the item's raw content bytes via the GGNetwork mirror (a
/// community-run service used by several other open-source Workshop
/// downloaders — not an official Valve API). Best-effort by nature: any
/// failure here is just a normal error the caller surfaces to the user, not
/// something to work around silently.
async fn download_via_mirror(client: &reqwest::Client, item_id: &str) -> Result<Vec<u8>, String> {
    let request_url = "https://api.ggntw.com/steam.request";
    let workshop_page = format!("https://steamcommunity.com/sharedfiles/filedetails/?id={}", item_id);
    let payload = serde_json::json!({ "url": workshop_page });

    let res = client
        .post(request_url)
        .header("Origin", "https://ggntw.com")
        .header("Referer", "https://ggntw.com/")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(45))
        .send()
        .await
        .map_err(|e| format!("No se pudo contactar al servicio de descarga: {}", e))?;

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Respuesta inesperada del servicio de descarga: {}", e))?;

    let download_url = json["download_url"]
        .as_str()
        .or_else(|| json["url"].as_str())
        .or_else(|| json["link"].as_str())
        .or_else(|| json["file"].as_str())
        .or_else(|| json["download"].as_str())
        .or_else(|| json["data"]["download_url"].as_str())
        .or_else(|| json["data"]["url"].as_str())
        .ok_or_else(|| "El servicio de descarga no devolvió un link válido para este ítem.".to_string())?;

    // Not every item resolves to a direct file link — some route through an
    // ad-gate/link-shortener instead (confirmed live: a real Garry's Mod item
    // came back pointing at ouo.io, while a Left 4 Dead 2 item came back
    // pointing straight at cdn.steamusercontent.com). A shortener page needs
    // a browser (ads, a wait timer, sometimes a captcha) to reveal the real
    // link — deliberately built to resist exactly the kind of automated GET
    // this app would otherwise do, which is also why that plain GET got 403
    // Forbidden rather than the file. Not something to work around: hand the
    // user the link instead so they can go through it themselves if they want.
    const KNOWN_SHORTENERS: &[&str] = &["ouo.io", "ouo.press"];
    if KNOWN_SHORTENERS.iter().any(|d| download_url.contains(d)) {
        return Err(format!(
            "Este ítem requiere pasar por un acortador con publicidad que la app no puede abrir sola. Abrilo manualmente: {}",
            download_url
        ));
    }

    // Same Origin/Referer as the initial request — the actual file host
    // behind ggntw.com's returned link rejects a bare cross-origin GET with
    // 403 Forbidden (confirmed against a real Garry's Mod item) without them,
    // same anti-hotlinking pattern as the API call itself.
    let file_res = client
        .get(download_url)
        .header("Origin", "https://ggntw.com")
        .header("Referer", "https://ggntw.com/")
        .timeout(std::time::Duration::from_secs(45))
        .send()
        .await
        .map_err(|e| format!("No se pudo descargar el contenido: {}", e))?;
    if !file_res.status().is_success() {
        return Err(format!("El servidor de descarga respondió {}", file_res.status()));
    }
    // Content-Type is the cheap first check, but not trustworthy alone —
    // some responses get served with a generic/wrong type. A real Garry's
    // Mod test confirmed `download_url` can point at the mirror's OWN
    // download page (title "GGNETWORK") instead of a raw file — a real
    // webpage, not an error — that presumably has its own "click to
    // download" step a browser would show. Without this check that page got
    // silently written to disk and reported as a successful ~19KB "mod".
    // Sniffing the actual bytes for an HTML/XML doctype is what catches this
    // (and the plain-error-page case) either way — same fix, same fallback:
    // hand the user the link instead of pretending the install worked.
    let looks_like_html = file_res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.to_ascii_lowercase().contains("html"))
        .unwrap_or(false);
    let bytes = file_res.bytes().await.map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("El archivo descargado está vacío.".to_string());
    }
    let starts_with_markup = {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64)]).to_ascii_lowercase();
        let trimmed = head.trim_start();
        trimmed.starts_with("<!doctype") || trimmed.starts_with("<html") || trimmed.starts_with("<?xml")
    };
    if looks_like_html || starts_with_markup {
        return Err(format!(
            "Este ítem no se pudo descargar automáticamente — el mirror devolvió su propia página de descarga en vez del archivo. Abrila manualmente: {}",
            download_url
        ));
    }
    Ok(bytes.to_vec())
}

/// Picks whichever Steam library actually has the game installed (falls back
/// to the main Steam path if it isn't installed there yet — the mod folder
/// just sits waiting in the default location in that case).
fn library_for_app(library_paths: &[String], steam_path: &str, app_id: &str) -> String {
    for lib in library_paths {
        let acf = PathBuf::from(lib)
            .join("steamapps")
            .join(format!("appmanifest_{}.acf", app_id));
        if acf.exists() {
            return lib.clone();
        }
    }
    library_paths.first().cloned().unwrap_or_else(|| steam_path.to_string())
}

/// Writes the downloaded bytes into `steamapps/workshop/content/<appid>/<itemid>/`
/// — the same path Steam's own client uses, so any game that scans its own
/// workshop folder tree directly (most engines do, rather than requiring a
/// live ISteamUGC subscription check) picks it up regardless of which
/// unlocker installed the game itself. `title`/`preview_url` are only for
/// Ragnarok's own installed-mods list (see list_installed_workshop_items) —
/// Steam itself never sees them.
pub fn install_workshop_content(
    library_paths: &[String],
    steam_path: &str,
    app_id: &str,
    item_id: &str,
    title: &str,
    preview_url: &str,
    bytes: &[u8],
) -> Result<WorkshopInstallResult, String> {
    let library = library_for_app(library_paths, steam_path, app_id);
    let dest_dir = PathBuf::from(&library)
        .join("steamapps")
        .join("workshop")
        .join("content")
        .join(app_id)
        .join(item_id);
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;

    let mut written_bytes: u64 = 0;
    let mut is_zip = false;
    // Any file recognized as a Source engine addon by its actual content —
    // not by whatever name it arrived with. SteamCMD in particular writes
    // some items under an opaque "<id>_legacy.bin" name with a useless
    // extension, so relying on the name alone (as this used to) silently
    // skipped the addons/ mirror for every SteamCMD-sourced download, which
    // is now the primary path. Queued up here and mirrored once below,
    // regardless of whether the content arrived as a zip or a single file.
    let mut engine_addon_files: Vec<(String, Vec<u8>)> = Vec::new();

    if let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) {
        let mut written = 0usize;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_string();
            if name.ends_with('/') {
                continue;
            }
            let out_path = dest_dir.join(&name);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut buf = Vec::new();
            std::io::copy(&mut entry, &mut buf).map_err(|e| e.to_string())?;
            written_bytes += buf.len() as u64;
            let ext = sniff_extension(&buf);
            if ext == "vpk" || ext == "gma" {
                let stem = std::path::Path::new(&name).file_stem().and_then(|s| s.to_str()).unwrap_or(item_id);
                engine_addon_files.push((format!("{}.{}", stem, ext), buf.clone()));
            }
            std::fs::write(&out_path, buf).map_err(|e| e.to_string())?;
            written += 1;
        }
        is_zip = written > 0;
    }

    if !is_zip {
        // Not a zip (or an empty one) — the mirror served the mod's single raw
        // file directly, same as Steam itself does for single-file items. The
        // extension matters here (unlike a made-up name): Source engine games
        // specifically look for *.vpk, and a wrong/generic extension is exactly
        // what silently made a real L4D2 mod invisible to the game the first
        // time this shipped.
        let ext = sniff_extension(bytes);
        let out_path = dest_dir.join(format!("{}.{}", item_id, ext));
        std::fs::write(&out_path, bytes).map_err(|e| e.to_string())?;
        written_bytes = bytes.len() as u64;
        if ext == "vpk" || ext == "gma" {
            engine_addon_files.push((format!("{}.{}", item_id, ext), bytes.to_vec()));
        }
    }

    // steamapps/workshop/content/ alone isn't enough for Source engine
    // games (L4D2, CS:GO/2, Garry's Mod, HL2 mods, ...) to load it —
    // mirroring into the game's own <moddir>/addons/ folder is the same
    // manual-install method these communities already use, on top of (not
    // instead of) the ISteamUGC registration below. A raw .vpk dropped there
    // auto-mounts on its own (confirmed against a real L4D2 test); a raw
    // .gma does NOT (confirmed against a real Garry's Mod test — the addon
    // just never showed up in-game) — it has to be extracted first, same as
    // manually dragging it onto the game's own gmad.exe would do. Best-
    // effort throughout: any failure here just means one fewer place the
    // file lives, not a failed install.
    // Names actually written under addons/ (files or, for extracted .gma,
    // folder names) — recorded on the index entry below so uninstall can
    // remove exactly these later. The mirrored name isn't always
    // `<item_id>.<ext>`: a zip-sourced (SteamCMD) download keeps whatever
    // stem the archive entry itself had (confirmed against a real Garry's
    // Mod item: it arrived as "gmpublisher.gma", not "<item_id>.gma").
    let mut mirrored_addon_names: Vec<String> = Vec::new();
    // .gma files that were written raw because they couldn't be extracted.
    let mut unmounted_gma: Vec<String> = Vec::new();
    let mut unmounted_reason: Option<String> = None;

    if !engine_addon_files.is_empty() {
        if let Some(addons_dir) = find_source_engine_addons_dir(steam_path, app_id) {
            if std::fs::create_dir_all(&addons_dir).is_ok() {
                let gmad_exe = crate::managers::crackers::find_game_folder(steam_path, app_id)
                    .and_then(|root| find_gmad_exe(&root));
                for (fname, data) in &engine_addon_files {
                    if fname.ends_with(".gma") {
                        if let Some(gmad) = &gmad_exe {
                            match extract_gma_via_gmad(gmad, &addons_dir, fname, data) {
                                Ok(()) => {
                                    let stem = fname.strip_suffix(".gma").unwrap_or(fname);
                                    mirrored_addon_names.push(stem.to_string());
                                    continue;
                                }
                                Err(e) => unmounted_reason = Some(e),
                            }
                        } else {
                            unmounted_reason =
                                Some("no se encontró gmad.exe en la carpeta del juego".to_string());
                        }
                        // The raw .gma is still written below so the user can
                        // extract it by hand, but it will NOT auto-mount —
                        // and the install used to be reported as a plain
                        // success anyway, which is how someone ends up with a
                        // mod listed as installed that never appears in-game.
                        unmounted_gma.push(fname.clone());
                    }
                    let _ = std::fs::write(addons_dir.join(fname), data);
                    mirrored_addon_names.push(fname.clone());
                }
            }
        }
    }

    // Best-effort: writes steamapps/workshop/appworkshop_<appid>.acf, the
    // file Steam's OWN client uses to track "what am I subscribed to" for
    // fully offline/emulated setups. Confirmed via a real test that the real
    // Steam client's own Properties > Workshop tab does NOT read this for an
    // item it doesn't already know about — even offline, with a genuine
    // account — so this does not make an item show up there. Kept anyway
    // (harmless, and may still help other emulated-Steam scenarios), but
    // never blocks the install: the actual functional part is the addons/
    // mirror above and the content folder itself, both of which are already
    // done by this point.
    let _ = register_workshop_subscription(library_paths, steam_path, app_id, item_id, written_bytes);

    let mut index = load_index(&library, app_id);
    index.insert(item_id.to_string(), InstalledWorkshopItem {
        item_id: item_id.to_string(),
        title: title.to_string(),
        preview_url: preview_url.to_string(),
        size_bytes: written_bytes,
        addon_names: mirrored_addon_names,
        needs_extraction: !unmounted_gma.is_empty(),
    });
    let _ = save_index(&library, app_id, &index);

    let warning = if unmounted_gma.is_empty() {
        None
    } else {
        Some(format!(
            "El contenido se descargó, pero {} quedó sin extraer ({}). Garry's Mod no carga un .gma sin extraer: instalá el juego por completo y volvé a instalar el mod, o arrastrá el archivo sobre bin/gmad.exe a mano.",
            unmounted_gma.join(", "),
            unmounted_reason.as_deref().unwrap_or("no se pudo extraer")
        ))
    };

    Ok(WorkshopInstallResult {
        path: dest_dir.to_string_lossy().to_string(),
        warning,
    })
}

/// What an install actually achieved. `warning` is Some when the files are on
/// disk but the game will not load them as-is — the case a plain `Ok(path)`
/// could not express, and which the panel used to report as a clean success.
#[derive(Serialize)]
pub struct WorkshopInstallResult {
    pub path: String,
    pub warning: Option<String>,
}

/// One entry in Ragnarok's own "installed mods" list for a game — purely
/// local bookkeeping (title/preview cached from the resolve step, since
/// Steam's own tracking doesn't reliably expose this data back to us) so the
/// Workshop panel can list and remove what it installed without needing a
/// GameCard/appid cross-reference or another network round-trip.
#[derive(Serialize, Deserialize, Clone)]
pub struct InstalledWorkshopItem {
    pub item_id: String,
    pub title: String,
    pub preview_url: String,
    pub size_bytes: u64,
    // Names actually written under the game's addons/ folder for this item
    // (file or extracted-folder names — see install_workshop_content).
    // #[serde(default)] so index entries written before this field existed
    // still deserialize instead of failing to load the whole list.
    #[serde(default)]
    pub addon_names: Vec<String>,
    /// True when a .gma was left raw because it could not be extracted — the
    /// item is on disk but the game will not load it. Shown in the installed
    /// list so it stays visible after the install notification is gone.
    #[serde(default)]
    pub needs_extraction: bool,
}

fn index_path(library: &str, app_id: &str) -> PathBuf {
    PathBuf::from(library).join("steamapps").join("workshop").join(format!("ragnarok_index_{}.json", app_id))
}

fn load_index(library: &str, app_id: &str) -> std::collections::HashMap<String, InstalledWorkshopItem> {
    std::fs::read_to_string(index_path(library, app_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(library: &str, app_id: &str, index: &std::collections::HashMap<String, InstalledWorkshopItem>) -> Result<(), String> {
    let path = index_path(library, app_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Lists everything Ragnarok has installed for this game via the Workshop
/// panel — self-heals against entries whose content folder was deleted some
/// other way (manually, or by the game itself) by dropping them from the
/// index rather than showing a stale "installed" mod that isn't actually
/// there anymore.
pub fn list_installed_workshop_items(library_paths: &[String], steam_path: &str, app_id: &str) -> Vec<InstalledWorkshopItem> {
    let library = library_for_app(library_paths, steam_path, app_id);
    let index = load_index(&library, app_id);
    let content_root = PathBuf::from(&library).join("steamapps").join("workshop").join("content").join(app_id);

    let mut still_present = std::collections::HashMap::new();
    let mut items: Vec<InstalledWorkshopItem> = Vec::new();
    for (id, item) in index.iter() {
        if content_root.join(id).exists() {
            still_present.insert(id.clone(), item.clone());
            items.push(item.clone());
        }
    }
    if still_present.len() != index.len() {
        let _ = save_index(&library, app_id, &still_present);
    }

    // Content folders that exist on disk but have no index entry — installed
    // by a Ragnarok build from before this list existed (confirmed to happen
    // in practice), or dropped there some other way. Shown with a generic
    // title (just the item id) rather than left invisible/unmanageable.
    if let Ok(entries) = std::fs::read_dir(&content_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !path.is_dir() || still_present.contains_key(id) {
                continue;
            }
            items.push(InstalledWorkshopItem {
                item_id: id.to_string(),
                title: format!("Item {}", id),
                preview_url: String::new(),
                size_bytes: dir_size(&path),
                // Unknown for an orphan entry — no record of what (if
                // anything) got mirrored into addons/ for it, so uninstall
                // falls back to guessing `<item_id>.vpk/.gma` for these.
                addon_names: Vec::new(),
                // Nothing recorded either way for an orphan entry; not
                // claiming a problem we have no evidence of.
                needs_extraction: false,
            });
        }
    }

    items
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Removes a previously-installed item: its content folder, its entry in
/// Ragnarok's own index, its entry in appworkshop_<appid>.acf (best-effort,
/// same reasoning as when it's written), and — if it was mirrored into a
/// Source engine game's addons/ folder — that copy too.
pub fn uninstall_workshop_item(library_paths: &[String], steam_path: &str, app_id: &str, item_id: &str) -> Result<(), String> {
    let library = library_for_app(library_paths, steam_path, app_id);
    let dest_dir = PathBuf::from(&library).join("steamapps").join("workshop").join("content").join(app_id).join(item_id);

    // Every step below is attempted regardless of whether an earlier one
    // failed — a real test found remove_dir_all aborting the WHOLE function
    // early (via `?`) on a locked file (the game was still running), which
    // left the content folder, the addons/ mirror, AND the index entry all
    // untouched — the mod looked completely unremoved even though most of it
    // could have been cleaned up. Errors are collected instead and only
    // reported (not fatal) at the end, and the index entry — the thing that
    // actually controls whether it still shows as "installed" in the
    // Workshop panel — is always removed even if some files couldn't be.
    let mut failures: Vec<String> = Vec::new();

    if dest_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dest_dir) {
            failures.push(format!("carpeta de contenido: {}", e));
        }
    }

    let mut index = load_index(&library, app_id);
    let known_addon_names = index.get(item_id).map(|i| i.addon_names.clone()).unwrap_or_default();
    index.remove(item_id);
    let _ = save_index(&library, app_id, &index);

    let _ = unregister_workshop_subscription(&library, app_id, item_id);

    if let Some(addons_dir) = find_source_engine_addons_dir(steam_path, app_id) {
        // Recorded names first (handles zip-sourced mirrors that didn't use
        // `<item_id>` as the stem — see install_workshop_content) — each
        // could be either a raw file (.vpk, or a .gma left as a fallback) or
        // an extracted .gma's own folder.
        for name in &known_addon_names {
            let p = addons_dir.join(name);
            if p.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&p) {
                    failures.push(format!("{}: {}", name, e));
                }
            } else if p.exists() {
                if let Err(e) = std::fs::remove_file(&p) {
                    failures.push(format!("{}: {}", name, e));
                }
            }
        }
        // Fallback guesses for orphan entries with no recorded names.
        for candidate in [
            addons_dir.join(format!("{}.vpk", item_id)),
            addons_dir.join(format!("{}.gma", item_id)),
            addons_dir.join(item_id),
        ] {
            if candidate.is_dir() {
                let _ = std::fs::remove_dir_all(&candidate);
            } else if candidate.exists() {
                let _ = std::fs::remove_file(&candidate);
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Se quitó de la lista, pero algunos archivos no se pudieron borrar (¿el juego está abierto?): {}",
            failures.join(" | ")
        ))
    }
}

/// Registers `item_id` as installed/subscribed in Steam's own local
/// tracking file for this app — steamapps/workshop/appworkshop_<appid>.acf —
/// which is what a game's in-game Workshop UI actually queries via
/// ISteamUGC, as opposed to scanning the content folder itself. Preserves
/// any existing entries already in the file (e.g. items the user's real
/// Steam account is genuinely subscribed to) using the same quote/brace-
/// aware scan-and-splice approach launch_options.rs uses for
/// localconfig.vdf, rather than risking a naive rewrite corrupting them.
fn register_workshop_subscription(
    library_paths: &[String],
    steam_path: &str,
    app_id: &str,
    item_id: &str,
    size_bytes: u64,
) -> Result<(), String> {
    let library = library_for_app(library_paths, steam_path, app_id);
    let workshop_dir = PathBuf::from(&library).join("steamapps").join("workshop");
    std::fs::create_dir_all(&workshop_dir).map_err(|e| e.to_string())?;
    let acf_path = workshop_dir.join(format!("appworkshop_{}.acf", app_id));

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // "Cannot read it" is not the same as "it isn't there".
    //
    // `unwrap_or_default()` collapsed a read error into an empty string, and an
    // empty string means "write a fresh skeleton" a few lines down — over a
    // file that holds the account's real Workshop subscriptions for this game.
    // A momentary lock or a non-UTF-8 byte was enough to replace it, and since
    // the whole call sits behind `let _ =` at the call site, neither the
    // failure nor the overwrite was reported anywhere.
    let original = match std::fs::read_to_string(&acf_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(format!(
                "No se pudo leer {} ({}). No se toca para no perder las suscripciones de Workshop.",
                acf_path.display(),
                e
            ));
        }
    };
    let text = if original.trim().is_empty() {
        format!(
            "\"AppWorkshop\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"SizeOnDisk\"\t\t\"0\"\n\t\"NeedsUpdate\"\t\t\"0\"\n\t\"NeedsDownload\"\t\t\"0\"\n\t\"TimeLastUpdated\"\t\t\"0\"\n\t\"TimeLastAppRan\"\t\t\"0\"\n\t\"WorkshopItemsInstalled\"\n\t{{\n\t}}\n\t\"WorkshopItemDetails\"\n\t{{\n\t}}\n}}\n",
            appid = app_id
        )
    } else {
        original
    };

    let updated = upsert_workshop_item(&text, "WorkshopItemsInstalled", item_id, &[
        ("size", size_bytes.to_string()),
        ("timeupdated", now.to_string()),
    ])?;
    let updated = upsert_workshop_item(&updated, "WorkshopItemDetails", item_id, &[
        ("manifest", "0".to_string()),
        ("timeupdated", now.to_string()),
        ("timetouched", now.to_string()),
    ])?;

    std::fs::write(&acf_path, updated).map_err(|e| e.to_string())
}

/// Finds (or creates) `<container_key> { "<item_id>" { ...fields... } }`
/// inside the `"AppWorkshop" { ... }` root and writes/overwrites the item's
/// block with the given fields.
fn upsert_workshop_item(text: &str, container_key: &str, item_id: &str, fields: &[(&str, String)]) -> Result<String, String> {
    use crate::managers::launch_options::{find_block_path, find_entry, VdfEntry};

    let Some(root_range) = find_block_path(text, &["AppWorkshop"]) else {
        return Err("appworkshop_<appid>.acf no tiene la sección \"AppWorkshop\" esperada — puede estar dañado.".to_string());
    };

    let fields_body: String = fields.iter()
        .map(|(k, v)| format!("\n\t\t\t\"{}\"\t\t\"{}\"", k, v))
        .collect();
    let item_block = format!("\"{}\"\n\t\t{{{}\n\t\t}}", item_id, fields_body);

    let (container_range, working_text) = match find_entry(text, root_range, container_key) {
        Some((VdfEntry::Block(s, e), _, _)) => ((s, e), text.to_string()),
        _ => {
            let (root_start, root_end) = root_range;
            let insertion = format!("\n\t\"{}\"\n\t{{\n\t}}", container_key);
            let new_text = format!("{}{}{}", &text[..root_start], insertion, &text[root_start..]);
            let new_root_range = (root_start, root_end + insertion.len());
            match find_entry(&new_text, new_root_range, container_key) {
                Some((VdfEntry::Block(s, e), _, _)) => ((s, e), new_text),
                _ => return Err(format!("No se pudo preparar la sección \"{}\".", container_key)),
            }
        }
    };

    let updated = match find_entry(&working_text, container_range, item_id) {
        Some((VdfEntry::Block(..), entry_start, entry_end)) => {
            format!("{}{}{}", &working_text[..entry_start], item_block, &working_text[entry_end..])
        }
        _ => {
            let (cstart, _) = container_range;
            let insertion = format!("\n\t\t{}", item_block);
            format!("{}{}{}", &working_text[..cstart], insertion, &working_text[cstart..])
        }
    };

    Ok(updated)
}

/// Counterpart to register_workshop_subscription — strips `item_id`'s entry
/// out of both WorkshopItemsInstalled and WorkshopItemDetails, if present.
/// A no-op (not an error) when the .acf doesn't exist or never had the entry.
fn unregister_workshop_subscription(library: &str, app_id: &str, item_id: &str) -> Result<(), String> {
    let acf_path = PathBuf::from(library).join("steamapps").join("workshop").join(format!("appworkshop_{}.acf", app_id));
    let Ok(original) = std::fs::read_to_string(&acf_path) else { return Ok(()) };

    let updated = remove_workshop_item(&original, "WorkshopItemsInstalled", item_id);
    let updated = remove_workshop_item(&updated, "WorkshopItemDetails", item_id);

    std::fs::write(&acf_path, updated).map_err(|e| e.to_string())
}

/// Splices `"<item_id>" { ... }` out of `<container_key>` inside the
/// `"AppWorkshop" { ... }` root, if it's there. Returns `text` unchanged on
/// any lookup miss (missing section, missing item, malformed file) — this is
/// cleanup, not something worth failing an uninstall over.
fn remove_workshop_item(text: &str, container_key: &str, item_id: &str) -> String {
    use crate::managers::launch_options::{find_block_path, find_entry, VdfEntry};

    let Some(root_range) = find_block_path(text, &["AppWorkshop"]) else { return text.to_string() };
    let Some((VdfEntry::Block(cs, ce), _, _)) = find_entry(text, root_range, container_key) else { return text.to_string() };
    let Some((VdfEntry::Block(..), entry_start, entry_end)) = find_entry(text, (cs, ce), item_id) else { return text.to_string() };

    format!("{}{}", &text[..entry_start], &text[entry_end..])
}

/// Identifies common single-file Workshop content by magic bytes so the
/// extension on disk actually means something to the game reading it,
/// instead of a made-up placeholder. Falls back to `.bin` (a normal "unknown
/// binary" convention) for anything unrecognized.
fn sniff_extension(bytes: &[u8]) -> &'static str {
    // VPK: Valve Pak, used by every Source engine game's Workshop addons.
    // Little-endian magic 0x55AA1234, i.e. bytes [0x34, 0x12, 0xAA, 0x55].
    if bytes.len() >= 4 && bytes[0..4] == [0x34, 0x12, 0xAA, 0x55] {
        return "vpk";
    }
    // GMA: Garry's Mod's own addon format — ASCII "GMAD" magic. Unlike other
    // Source titles, GMod Workshop items are published as .gma, not .vpk.
    if bytes.len() >= 4 && &bytes[0..4] == b"GMAD" {
        return "gma";
    }
    "bin"
}

/// Walks a couple levels into the game's actual install folder looking for
/// `gameinfo.txt` — the marker Source engine uses for a game/mod directory —
/// and returns its `addons/` subfolder. None if this isn't a Source engine
/// game, or the install folder can't be resolved at all (game not actually
/// downloaded yet, ticket-only install).
fn find_source_engine_addons_dir(steam_path: &str, app_id: &str) -> Option<PathBuf> {
    let root = crate::managers::crackers::find_game_folder(steam_path, app_id)?;
    for entry in std::fs::read_dir(&root).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("gameinfo.txt").exists() {
            return Some(path.join("addons"));
        }
        // One more level (e.g. root/left4dead2/left4dead2_dlc3/gameinfo.txt-
        // style layouts some Source games use for content packs).
        for sub in std::fs::read_dir(&path).ok()?.flatten() {
            let sub_path = sub.path();
            if sub_path.is_dir() && sub_path.join("gameinfo.txt").exists() {
                return Some(sub_path.join("addons"));
            }
        }
    }
    None
}

/// Locates the addon-creator tool Garry's Mod (and other GMod-derived games)
/// ships in its own install — confirmed present at `bin/gmad.exe` on a real
/// install. Only ever exists for games that actually use the .gma format.
fn find_gmad_exe(game_root: &std::path::Path) -> Option<PathBuf> {
    for candidate in ["bin/gmad.exe", "bin/win64/gmad.exe", "bin/win32/gmad.exe"] {
        let p = game_root.join(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Extracts a .gma into `<addons_dir>/<name-without-.gma>/` via the game's
/// own gmad.exe — the same tool (and exact invocation) a manual "drag the
/// .gma onto gmad.exe" install does. Confirmed via a real Garry's Mod test
/// that a raw, unextracted .gma sitting in addons/ is NOT picked up by the
/// game at all (unlike a Source engine .vpk, which auto-mounts as-is).
fn extract_gma_via_gmad(gmad_exe: &std::path::Path, addons_dir: &std::path::Path, fname: &str, data: &[u8]) -> Result<(), String> {
    let stem = fname.strip_suffix(".gma").unwrap_or(fname);
    let temp_gma = std::env::temp_dir().join(format!("ragnarok_workshop_{}.gma", stem));
    std::fs::write(&temp_gma, data).map_err(|e| e.to_string())?;

    let out_dir = addons_dir.join(stem);
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let temp_gma_str = temp_gma.to_str().ok_or_else(|| "Ruta temporal inválida.".to_string())?;
    let out_dir_str = out_dir.to_str().ok_or_else(|| "Ruta de addons inválida.".to_string())?;

    let mut cmd = std::process::Command::new(gmad_exe);
    cmd.args(["extract", "-file", temp_gma_str, "-out", out_dir_str, "-warninvalid", "-quiet"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let status = cmd.status().map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&temp_gma);

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("gmad.exe terminó con código {:?}.", s.code())),
        Err(e) => Err(e),
    }
}
