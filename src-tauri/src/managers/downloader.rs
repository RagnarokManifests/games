use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use flate2::read::GzDecoder;
use zip::ZipArchive;

#[allow(dead_code)]
pub struct RustDownloader;

#[allow(dead_code)]
impl RustDownloader {
    /// Decompresses a .gz file to a target path.
    pub fn decompress_gz(compressed_path: &Path, target_path: &Path) -> Result<(), String> {
        let compressed_file = fs::File::open(compressed_path).map_err(|e| e.to_string())?;
        let mut decoder = GzDecoder::new(compressed_file);
        let mut buffer = Vec::new();
        decoder.read_to_end(&mut buffer).map_err(|e| e.to_string())?;
        fs::write(target_path, buffer).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Calculates the total size in bytes of all files in a directory (recursive).
    pub fn dir_size(path: &Path) -> u64 {
        if !path.exists() {
            return 0;
        }
        let mut total: u64 = 0;
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    total += Self::dir_size(&p);
                } else if let Ok(meta) = fs::metadata(&p) {
                    total += meta.len();
                }
            }
        }
        total
    }

    /// Extracts the first bare numeric id inside `fn_name(<id>, ...)`.
    fn extract_first_id(line: &str) -> Option<&str> {
        let start = line.find('(')? + 1;
        let rest = &line[start..];
        let end = rest.find(|c: char| !c.is_ascii_digit())?;
        if end == 0 { return None; }
        Some(&rest[..end])
    }

    /// Strips `addappid`/`setManifestid` lines that reference a depot id NOT
    /// in `windows_depot_ids` — this third-party-generated ticket lists every
    /// depot indiscriminately, including platform-specific ones (e.g. the
    /// macOS build of a cross-platform game). Registering a macOS-only depot
    /// with Steam's Windows client is what was making Steam crash mid-download
    /// for games like Dave the Diver, even after our own generated
    /// SteamTools.lua started filtering by platform — this ticket file is a
    /// separate one Steam's plugin loads from the same folder, so it needed
    /// the same treatment.
    fn sanitize_lua_for_windows(content: &str, app_id: &str, windows_depot_ids: &std::collections::HashSet<String>) -> String {
        // windows_depot_ids is every depot SteamCMD says Windows can use (see
        // DepotFacts::windows_ok) — deliberately wider than the set that has
        // manifests, because a depot with no manifest still needs its
        // `addappid` decryption key to survive. For some titles (confirmed: older/
        // delisted games like Need for Speed: Undercover, appid 17430) that
        // API returns literally no data at all, leaving this set empty. That
        // used to mean EVERY depot line got stripped (since "not in an empty
        // set" is true for everything), collapsing the ticket down to a bare
        // addappid() with no depot/manifest data — Steam then has nothing to
        // download, shows the game as instantly "installed" at 0 bytes, and
        // the folder stays empty even after "verify files". With zero
        // reference data to check against, trusting Ryuu's ticket wholesale
        // is far safer than gutting it entirely.
        if windows_depot_ids.is_empty() {
            return content.to_string();
        }
        content
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                if !(trimmed.starts_with("addappid(") || trimmed.starts_with("setManifestid(")) {
                    return true; // comments, blank lines, anything else — keep as-is
                }
                match Self::extract_first_id(trimmed) {
                    Some(id) if id == app_id => true, // the game's own bare addappid(id)
                    Some(id) => windows_depot_ids.contains(id),
                    None => true, // couldn't parse — keep rather than risk dropping something needed
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Collects every depot id this ticket declares a manifest for (via
    /// `setManifestid(depot_id, ...)`), excluding the game's own app id.
    /// The caller uses this to avoid ALSO writing a setManifestid for the
    /// same depot in our own generated SteamTools.lua/RagnarokGames.lua —
    /// two Lua files in the same folder declaring two different manifest
    /// ids for the same depot (this ticket's vs. our SteamCMD-sourced one)
    /// is a real, confirmed way to destabilize Steam's content system,
    /// especially right after a game updates and this pre-generated ticket
    /// hasn't caught up yet with live SteamCMD data.
    fn collect_ticket_depot_ids(content: &str, app_id: &str) -> std::collections::HashSet<String> {
        let mut ids = std::collections::HashSet::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("setManifestid(") {
                continue;
            }
            if let Some(id) = Self::extract_first_id(trimmed) {
                if id != app_id {
                    ids.insert(id.to_string());
                }
            }
        }
        ids
    }

    /// Downloads Ryuu's ticket and installs it. Kept for callers that only
    /// ever talk to Ryuu; `install_game` chooses its source itself now.
    pub async fn download_and_install(
        app_id: &str,
        steam_path: &str,
        ryuu_api_key: Option<&str>,
        windows_depot_ids: Option<&std::collections::HashSet<String>>,
    ) -> Result<std::collections::HashSet<String>, String> {
        let bytes = Self::fetch_ryuu_ticket(app_id, ryuu_api_key).await?;
        Self::install_ticket_bytes(&bytes, app_id, steam_path, windows_depot_ids, "Ryuu")
    }

    /// Ryuu's ticket for `app_id`, as raw bytes. Nothing is written.
    pub async fn fetch_ryuu_ticket(app_id: &str, ryuu_api_key: Option<&str>) -> Result<Vec<u8>, String> {
        let key = ryuu_api_key.unwrap_or("RYUUMANIFESTfqdpr3");
        let url = format!(
            "https://generator.ryuu.lol/secure_download?appid={}&auth_code={}",
            app_id, key
        );

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 Ragnarok-Launcher")
            .build()
            .map_err(|e| e.to_string())?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Download error: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "HTTP {}: manifest zip not found for {}",
                response.status(),
                app_id
            ));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Buffer error: {}", e))?;
        Ok(bytes.to_vec())
    }

    /// True if these bytes open as a zip archive (a normal or an empty one).
    fn is_zip(bytes: &[u8]) -> bool {
        bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
    }

    /// Installs a ticket from any source into Steam:
    ///
    ///   .lua      → steam_path/config/lua/<appid>.lua
    ///   .manifest → steam_path/depotcache/
    ///   anything else → steam_path/steamapps/
    ///
    /// Split out of the Ryuu download so a second source feeds the exact same
    /// install instead of a copy of it. The bytes are sniffed rather than
    /// trusted: Hubcap, like several of the sources LuaTools talks to,
    /// sometimes answers with a bare .lua instead of a zip, and handing that to
    /// the zip reader fails with "invalid Zip archive" for a perfectly usable
    /// ticket.
    ///
    /// The .lua is always written as `<appid>.lua`, whatever it was called
    /// inside the archive. That filename is the convention the rest of Ragnarok
    /// keys games off, and the two sources do not agree on it.
    ///
    /// `windows_depot_ids`: every depot Windows may use (see DepotFacts), used
    /// to sanitize the .lua. `None` skips sanitizing.
    ///
    /// Returns the depot ids the (sanitized) .lua declares a manifest for, so
    /// the caller can skip writing a conflicting setManifestid of its own.
    pub fn install_ticket_bytes(
        bytes: &[u8],
        app_id: &str,
        steam_path: &str,
        windows_depot_ids: Option<&std::collections::HashSet<String>>,
        source: &str,
    ) -> Result<std::collections::HashSet<String>, String> {
        let steamapps_path = PathBuf::from(steam_path).join("steamapps");
        let depotcache_path = PathBuf::from(steam_path).join("depotcache");
        let lua_path = PathBuf::from(steam_path).join("config").join("lua");

        fs::create_dir_all(&depotcache_path).map_err(|e| e.to_string())?;
        fs::create_dir_all(&lua_path).map_err(|e| e.to_string())?;

        crate::diag_log!(
            "[ticket] AppID {}: ticket de {} recibido, {} bytes.",
            app_id,
            source,
            bytes.len()
        );

        let lua_dest = lua_path.join(format!("{}.lua", app_id));
        let write_lua = |raw: &str, ids: &mut std::collections::HashSet<String>| -> Result<(), String> {
            let text = match windows_depot_ids {
                Some(allowlist) => Self::sanitize_lua_for_windows(raw, app_id, allowlist),
                None => raw.to_string(),
            };
            ids.extend(Self::collect_ticket_depot_ids(&text, app_id));
            // Whole or not at all: OpenSteamTool reads this folder while Steam
            // is running, and a half-written ticket is a broken one.
            let tmp = lua_dest.with_extension("lua.tmp");
            fs::write(&tmp, text)
                .and_then(|_| fs::rename(&tmp, &lua_dest))
                .map_err(|e| {
                    let _ = fs::remove_file(&tmp);
                    format!("Cannot write {}: {}", lua_dest.display(), e)
                })
        };

        let mut ticket_depot_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        if !Self::is_zip(bytes) {
            // A bare ticket. Checked for real ticket content first: a source
            // that is down or rate limited tends to answer 200 with an HTML
            // page, and writing that out as <appid>.lua would replace a working
            // ticket with garbage.
            let text = String::from_utf8_lossy(bytes);
            if !text.contains("addappid(") {
                return Err(format!(
                    "{} no devolvió un ticket válido (ni un zip ni un .lua).",
                    source
                ));
            }
            write_lua(&text, &mut ticket_depot_ids)?;
            crate::diag_log!(
                "[ticket] AppID {}: .lua suelto de {}, sin .manifest adjuntos — dependen de lo que hayan dejado los mirrors.",
                app_id,
                source
            );
            crate::managers::install_sources::record(app_id, source);
            return Ok(ticket_depot_ids);
        }

        let mut archive =
            ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;

        // What the ticket actually contained, and where each piece landed.
        //
        // This step used to be completely silent, which made the most
        // consequential question about an install unanswerable afterwards:
        // did the manifest ever arrive? A game whose ticket carries no
        // .manifest and one whose manifest was written and later deleted end
        // up identical on disk, and only the second is worth retrying.
        let mut lua_count = 0usize;
        let mut manifest_count = 0usize;
        let mut other_count = 0usize;
        // Held back until every manifest is on disk. Ryuu's zip lists the .lua
        // first, so it used to land a millisecond or two before the manifest it
        // pins — measured: 21:08:05.2156 for the ticket, .2171 for the
        // manifest. Steam could see the new game with its manifest not there
        // yet, and the install only went through on a second press.
        let mut pending_lua: Option<String> = None;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }

            let entry_name = entry.name().to_string();
            // Only the filename: path components inside the archive are never
            // trusted as a place to write.
            let filename = Path::new(&entry_name)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&entry_name)
                .to_string();

            if filename.ends_with(".lua") {
                lua_count += 1;
                let mut raw = String::new();
                entry.read_to_string(&mut raw).map_err(|e| e.to_string())?;
                pending_lua = Some(raw);
                continue;
            }

            let dest = if filename.ends_with(".manifest") {
                manifest_count += 1;
                crate::diag_log!("[ticket]   {} → depotcache/", filename);
                depotcache_path.join(&filename)
            } else {
                other_count += 1;
                crate::diag_log!("[ticket]   {} → steamapps/ (no reconocido)", filename);
                steamapps_path.join(&filename)
            };

            if dest.exists() {
                crate::managers::manifest_vault::make_writable(&dest);
            }
            let mut outfile = fs::File::create(&dest)
                .map_err(|e| format!("Cannot create {}: {}", dest.display(), e))?;
            io::copy(&mut entry, &mut outfile)
                .map_err(|e| format!("Write error for {}: {}", dest.display(), e))?;
            if filename.ends_with(".manifest") {
                crate::managers::manifest_vault::make_readonly(&dest);
            }
        }

        // The ticket last, once everything it pins is in place.
        if let Some(raw) = pending_lua {
            write_lua(&raw, &mut ticket_depot_ids)?;
        }

        crate::diag_log!(
            "[ticket] AppID {} ({}): {} .lua, {} .manifest, {} otros — depots declarados: {}.",
            app_id,
            source,
            lua_count,
            manifest_count,
            other_count,
            if ticket_depot_ids.is_empty() {
                "ninguno".to_string()
            } else {
                let mut ids: Vec<&str> = ticket_depot_ids.iter().map(|s| s.as_str()).collect();
                ids.sort_unstable();
                ids.join(", ")
            }
        );
        if manifest_count == 0 {
            // Recorded, not diagnosed. A ticket without manifests is only a
            // problem when nothing else supplied them, and whether anything is
            // actually missing is settled after the install by looking at the
            // disk, which is the only check that can know.
            crate::diag_log!(
                "[ticket] AppID {}: el ticket de {} no trae .manifest — dependen de lo que hayan dejado los mirrors.",
                app_id,
                source
            );
        }

        crate::managers::install_sources::record(app_id, source);
        Ok(ticket_depot_ids)
    }
}

#[cfg(test)]
mod lua_sanitizer_tests {
    use super::RustDownloader;
    use std::collections::HashSet;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// Shaped after a real PAYDAY 2 ticket: the game's own line, a content
    /// depot, a DLC depot SteamCMD reports at size 0, and a depot belonging to
    /// another OS.
    const TICKET: &str = concat!(
        "addappid(218620,0,\"aaaa\")\n",
        "addappid(218621,0,\"bbbb\")\n",
        "setManifestid(218621,\"6226081920218254228\")\n",
        "addappid(218623,0,\"cccc\")\n",
        "setManifestid(218623,\"7700458186423454139\")\n",
        "addappid(1868142,0,\"dddd\")\n",
        "setManifestid(1868142,\"999\")\n",
        "addappid(218620)",
    );

    /// The bug this fixes. A DLC depot with no manifest to pin still needs its
    /// decryption key, and dropping it leaves the content owned-but-encrypted —
    /// the state the OpenSteamTool forum diagnoses as "falta la clave".
    #[test]
    fn a_depot_without_a_manifest_keeps_its_key() {
        // 218623 is usable by Windows but has no content to pin, so it belongs
        // in the allowlist even though it never gets a setManifestid of its own.
        let allow = set(&["218621", "218623"]);
        let out = RustDownloader::sanitize_lua_for_windows(TICKET, "218620", &allow);

        assert!(
            out.contains("addappid(218623,0,\"cccc\")"),
            "la clave del DLC tiene que sobrevivir:\n{out}"
        );
        assert!(out.contains("addappid(218621,0,\"bbbb\")"));
        assert!(out.contains("addappid(218620)"), "la línea propia del juego se conserva");
    }

    /// The Dave the Diver crash, still guarded: a depot Windows can never use
    /// has to go entirely, key and pin alike.
    #[test]
    fn a_depot_windows_cannot_use_is_still_dropped() {
        let allow = set(&["218621", "218623"]);
        let out = RustDownloader::sanitize_lua_for_windows(TICKET, "218620", &allow);
        assert!(!out.contains("1868142"), "el depot de otro SO no puede quedar:\n{out}");
    }

    /// With nothing to compare against, gutting the ticket is worse than
    /// trusting it whole.
    #[test]
    fn an_empty_allowlist_leaves_the_ticket_untouched() {
        let out = RustDownloader::sanitize_lua_for_windows(TICKET, "218620", &HashSet::new());
        assert_eq!(out, TICKET);
    }
}

#[cfg(test)]
mod ticket_install_tests {
    use super::RustDownloader;
    use std::io::Write;

    fn steam_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ragnarok_ticket_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::FileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// Hubcap sometimes answers with a bare .lua. It has to install exactly
    /// like the zip would, instead of failing as "invalid Zip archive".
    #[test]
    fn a_bare_lua_ticket_installs() {
        let steam = steam_dir("bare");
        let lua = b"addappid(431960,0,\"aa\")\naddappid(431961,0,\"bb\")\nsetManifestid(431961,\"123\")\naddappid(431960)";
        let ids = RustDownloader::install_ticket_bytes(lua, "431960", &steam.to_string_lossy(), None, "Hubcap").unwrap();

        let written = std::fs::read_to_string(steam.join("config/lua/431960.lua")).unwrap();
        assert!(written.contains("setManifestid(431961"), "{written}");
        assert!(ids.contains("431961"));
        let _ = std::fs::remove_dir_all(&steam);
    }

    /// A down or rate-limited source answering 200 with an HTML page must not
    /// overwrite a working ticket with it.
    #[test]
    fn a_page_that_is_not_a_ticket_is_refused_and_writes_nothing() {
        let steam = steam_dir("html");
        let lua_dir = steam.join("config/lua");
        std::fs::create_dir_all(&lua_dir).unwrap();
        std::fs::write(lua_dir.join("431960.lua"), "addappid(431960) -- el bueno").unwrap();

        let err = RustDownloader::install_ticket_bytes(
            b"<html><body>Too many requests</body></html>",
            "431960",
            &steam.to_string_lossy(),
            None,
            "Hubcap",
        )
        .unwrap_err();
        assert!(err.contains("Hubcap"), "{err}");
        assert_eq!(
            std::fs::read_to_string(lua_dir.join("431960.lua")).unwrap(),
            "addappid(431960) -- el bueno",
            "el ticket que funcionaba no se toca"
        );
        let _ = std::fs::remove_dir_all(&steam);
    }

    /// The ticket goes in last. Ryuu's zip lists the .lua first, and writing
    /// it first let Steam see the game before its manifest existed — the
    /// install then only worked on a second press. Proven through the failure
    /// path: when the manifest cannot be written, no ticket may be left behind.
    #[test]
    fn the_ticket_is_written_only_after_its_manifests() {
        let steam = steam_dir("order");
        // A folder where the manifest file should go makes writing it fail.
        std::fs::create_dir_all(steam.join("depotcache/431961_123.manifest")).unwrap();
        let bytes = zip_with(&[
            ("431960.lua", b"addappid(431960,0,\"aa\")\nsetManifestid(431961,\"123\")"),
            ("431961_123.manifest", b"bytes del manifiesto"),
        ]);

        assert!(RustDownloader::install_ticket_bytes(&bytes, "431960", &steam.to_string_lossy(), None, "Ryuu").is_err());
        assert!(!steam.join("config/lua/431960.lua").exists(), "sin manifiesto no hay ticket");
        assert!(!steam.join("config/lua/431960.lua.tmp").exists());
        let _ = std::fs::remove_dir_all(&steam);
    }

    /// Sources disagree on what the .lua inside the zip is called; Ragnarok
    /// keys every game off `<appid>.lua`, so that is the name it gets.
    #[test]
    fn a_zip_ticket_lands_under_the_app_id_whatever_its_lua_is_called() {
        let steam = steam_dir("zip");
        let bytes = zip_with(&[
            ("game_ticket.lua", b"addappid(431960,0,\"aa\")\nsetManifestid(431961,\"123\")"),
            ("431961_123.manifest", b"bytes del manifiesto"),
        ]);
        RustDownloader::install_ticket_bytes(&bytes, "431960", &steam.to_string_lossy(), None, "Hubcap").unwrap();

        assert!(steam.join("config/lua/431960.lua").is_file(), "el .lua va con el nombre del AppID");
        assert!(!steam.join("config/lua/game_ticket.lua").exists());
        let manifest = steam.join("depotcache/431961_123.manifest");
        assert_eq!(std::fs::read(&manifest).unwrap(), b"bytes del manifiesto");

        // Installed read-only on purpose; undone here only so cleanup works.
        crate::managers::manifest_vault::make_writable(&manifest);
        let _ = std::fs::remove_dir_all(&steam);
    }
}
