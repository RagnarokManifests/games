//! Ragnarok's own copy of the depot manifests it installs, plus the two
//! integrity checks that catch the ways Steam breaks a spoofed install.
//!
//! `Steam\depotcache\<depot>_<gid>.manifest` is what lets a game install
//! without the account owning it. Steam reads the depot's file list from that
//! file instead of asking the CDN for it, and the CDN is precisely the part
//! that answers 401 Unauthorized for an app you don't own — the access token
//! it wants is issued per (depot, manifest) and only to an owner. The chunks
//! themselves download normally: they are encrypted blobs, and the key comes
//! from `addappid(depot, 0, "<hex>")` in the game's .lua.
//!
//! Until now the launcher wrote those manifests straight into Steam's
//! depotcache and kept no copy of its own. That folder is Steam's cache,
//! Steam owns it, and Steam prunes it. Confirmed on a user's machine:
//!
//! * Uninstalling "How to Fish" (4001890) removed `4001891_3889140805796509645`.
//!   The game had installed cleanly forty minutes earlier off that exact file.
//! * ELDEN RING's three base-game manifests were replaced when its build
//!   changed, leaving the ACF pointing at gids no longer on disk.
//!
//! Manifests for depots Steam never mounted survive untouched, so this is not
//! a blind wipe — it is Steam tidying up what its own bookkeeping considers
//! finished. Either way the file is gone, and without it the game cannot be
//! set up again without going back to the network.
//!
//! Both resulting failures are checked here, and both are repairable from
//! disk alone:
//!
//! * **missing** — the manifest a game's .lua pins is not in depotcache.
//!   Steam asks the CDN, gets 401, and reports it as "UNKNOWN ERROR" or "No
//!   connection to content servers". That wording sends everyone hunting for
//!   a network problem; the connection is fine.
//! * **drift** — the ACF's `InstalledDepots` records a different gid than the
//!   .lua pins. Steam asks the CDN for the *recorded* one to work out the
//!   delta to the new build, gets 401, cancels, and re-queues 30 seconds
//!   later. Forever. The download sits at "Starting Download", 0 bps.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::manifest::ManifestManager;
use crate::diag_log;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// 0x4 in an ACF's StateFlags. Repairs are refused without it — rewriting the
/// depot list of a game Steam does not consider installed would be forging an
/// install, not fixing one.
const STATE_FULLY_INSTALLED: u32 = 0x4;

/// Where Ragnarok keeps its own copy. Deliberately outside the Steam folder:
/// the whole point is to survive Steam deciding the file is no longer needed.
pub fn vault_dir() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("com.ragnarok.launcher")
            .join("depotcache"),
    )
}

fn steam_depotcache(steam_path: &str) -> PathBuf {
    PathBuf::from(steam_path).join("depotcache")
}

fn is_manifest_name(name: &str) -> bool {
    name.ends_with(".manifest") && name.contains('_')
}

/// Sets a file to Read-Only so external cleanup routines (such as Steam's
/// uninstaller removing cache) fail with Access Denied and leave the manifest intact.
/// NOTE: We only use the ReadOnly attribute (no NTFS deny ACLs) because deny ACLs
/// block Steam itself from reading/processing the manifest, breaking detection.
pub fn make_readonly(path: &Path) {
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        if !permissions.readonly() {
            permissions.set_readonly(true);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

/// Clears the Read-Only attribute if the file needs to be modified, updated, or removed by Ragnarok.
pub fn make_writable(path: &Path) {
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        if permissions.readonly() {
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

/// Safely writes bytes to a file that may be currently read-only, ensuring it ends up read-only.
pub fn write_protected_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if path.exists() {
        make_writable(path);
    }
    std::fs::write(path, bytes)?;
    make_readonly(path);
    Ok(())
}

/// Scans `Steam/depotcache` and ensures all `.manifest` files are marked Read-Only
/// to prevent Steam from purging them on game uninstall. Returns the count of protected files.
pub fn protect_depotcache_manifests(steam_path: &str) -> usize {
    let depotcache = steam_depotcache(steam_path);
    let Ok(entries) = std::fs::read_dir(depotcache) else { return 0 };
    let mut protected_count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if is_manifest_name(name) {
                    make_readonly(&path);
                    protected_count += 1;
                }
            }
        }
    }
    if protected_count > 0 {
        diag_log!("[vault] {} manifest(s) protegidos contra eliminación en depotcache.", protected_count);
    }
    protected_count
}

/// One thing wrong with one depot, in a shape the Fix tab can render directly.
#[derive(Serialize, Deserialize, Clone)]
pub struct ManifestIssue {
    pub app_id: String,
    pub app_name: String,
    pub depot_id: String,
    /// `"missing"` or `"drift"`.
    pub kind: String,
    /// The gid the game's .lua pins. This is the one that has to win: it is
    /// the manifest the launcher actually downloaded content against.
    pub expected: String,
    /// What the ACF records instead. Empty for a `"missing"` issue.
    pub found: String,
    pub repaired: bool,
    /// Whether Steam currently has this game installed.
    ///
    /// A missing manifest for a game that is not installed is not a fault yet
    /// — it only bites at install time, and the launcher re-downloads the
    /// ticket then anyway. Without this the check reported 30 problems on a
    /// healthy machine when 3 of them were real, which is the fastest way to
    /// train someone to ignore it.
    pub installed: bool,
    pub detail: String,
}

// ── Vault ───────────────────────────────────────────────────────────────────

/// Copies every manifest Steam currently holds into the vault.
///
/// Returns `(copiados, vistos)`. The gid in the filename identifies an exact
/// manifest version, so a name already in the vault is byte-identical and is
/// skipped rather than re-copied.
pub fn back_up(steam_path: &str) -> (usize, usize) {
    let Some(vault) = vault_dir() else { return (0, 0) };
    if std::fs::create_dir_all(&vault).is_err() {
        return (0, 0);
    }
    let Ok(entries) = std::fs::read_dir(steam_depotcache(steam_path)) else {
        return (0, 0);
    };

    let mut copied = 0usize;
    let mut seen = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_manifest_name(&name) {
            continue;
        }
        seen += 1;
        let dest = vault.join(&name);
        if dest.exists() {
            continue;
        }
        if std::fs::copy(entry.path(), &dest).is_ok() {
            copied += 1;
        }
    }

    if copied > 0 {
        diag_log!(
            "[vault] {} manifest(s) respaldados ({} presentes en depotcache).",
            copied,
            seen
        );
    }
    (copied, seen)
}

/// Puts one manifest back into Steam's depotcache from the vault.
///
/// Only ever fills a gap: an existing file is left alone, since Steam's copy
/// is the one it is already using.
fn restore_one(steam_path: &str, depot_id: &str, gid: &str) -> bool {
    let Some(vault) = vault_dir() else { return false };
    let name = format!("{}_{}.manifest", depot_id, gid);
    let src = vault.join(&name);
    if !src.is_file() {
        return false;
    }
    let dest_dir = steam_depotcache(steam_path);
    if std::fs::create_dir_all(&dest_dir).is_err() {
        return false;
    }
    let dest = dest_dir.join(&name);
    if dest.exists() {
        make_readonly(&dest);
        return true;
    }
    make_writable(&dest);
    match std::fs::copy(&src, &dest) {
        Ok(_) => {
            make_readonly(&dest);
            diag_log!("[vault] Restaurado {} desde el respaldo (protegido).", name);
            true
        }
        Err(e) => {
            diag_log!("[vault] No se pudo restaurar {}: {}", name, e);
            false
        }
    }
}

/// One manifest a ticket pins but Steam does not have on disk.
#[derive(Debug)]
pub struct MissingManifest {
    pub app_id: String,
    pub depot_id: String,
    pub gid: String,
}

impl MissingManifest {
    fn filename(&self) -> String {
        format!("{}_{}.manifest", self.depot_id, self.gid)
    }
}

/// Puts back what the vault has, and reports what it could not cover.
///
/// Kept separate from the network step so the common case — Steam pruned a
/// file this launcher already saved — needs no connection at all.
pub fn restore_from_vault(steam_path: &str) -> (Vec<String>, Vec<MissingManifest>) {
    let depotcache = steam_depotcache(steam_path);
    let mut restored = Vec::new();
    let mut still_missing = Vec::new();

    for (app_id, pins) in pinned_by_app(steam_path) {
        for (depot, gid) in pins {
            if depotcache
                .join(format!("{}_{}.manifest", depot, gid))
                .is_file()
            {
                continue;
            }
            if restore_one(steam_path, &depot, &gid) {
                restored.push(format!("{}_{}.manifest", depot, gid));
            } else {
                still_missing.push(MissingManifest {
                    app_id: app_id.clone(),
                    depot_id: depot,
                    gid,
                });
            }
        }
    }
    (restored, still_missing)
}

/// Puts back every manifest a ticket pins that Steam no longer has.
///
/// The counterpart to `back_up`, and the half that actually saves an install.
/// Backing the files up is worth nothing on its own: Steam prunes one, the
/// user reinstalls, and the download dies on a 401 with the copy sitting in
/// the vault the whole time because nothing ever put it back.
///
/// Two sources, cheapest first: this launcher's own vault, then the public
/// manifest mirror. The mirror matters more than it looks — measured on a real
/// machine, 20 of the 31 manifests missing from that install were sitting in
/// the mirror the whole time, for games the vault had never seen because they
/// were already gone when it was first filled. A vault that only ever reads
/// itself can never recover those, which reads to the user as the feature
/// simply not working.
///
/// A mirror hit is written into the vault as well, so the next time it is
/// needed no connection is required.
///
/// Only fills gaps — an existing file is Steam's own and is left alone — and
/// only touches manifests some `.lua` actually pins, so a game Steam removed
/// on purpose does not get its old depots quietly re-seeded.
/// The manifests one game's `.lua` pins that are not in depotcache.
///
/// Checked after an install rather than predicted before one: the question
/// that matters is whether the file actually landed, and only the filesystem
/// can answer that. An install can report success with the manifest missing,
/// and until now nothing looked.
pub fn missing_for_app(steam_path: &str, app_id: &str) -> Vec<MissingManifest> {
    let depotcache = steam_depotcache(steam_path);
    pinned_by_app(steam_path)
        .get(app_id)
        .map(|pins| {
            pins.iter()
                .filter(|(depot, gid)| {
                    !depotcache
                        .join(format!("{}_{}.manifest", depot, gid))
                        .is_file()
                })
                .map(|(depot, gid)| MissingManifest {
                    app_id: app_id.to_string(),
                    depot_id: depot.clone(),
                    gid: gid.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How many pinned manifests each ticketed game is missing, in one pass —
/// the Library asks about every game at once, every few seconds.
pub fn missing_counts(steam_path: &str) -> HashMap<String, usize> {
    let depotcache = steam_depotcache(steam_path);
    pinned_by_app(steam_path)
        .into_iter()
        .map(|(app_id, pins)| {
            let missing = pins
                .iter()
                .filter(|(depot, gid)| !depotcache.join(format!("{}_{}.manifest", depot, gid)).is_file())
                .count();
            (app_id, missing)
        })
        .collect()
}

pub async fn restore_missing(steam_path: &str) -> Vec<String> {
    let (mut restored, still_missing) = restore_from_vault(steam_path);

    if !still_missing.is_empty() {
        if let Ok(client) = crate::make_client() {
            let depotcache = steam_depotcache(steam_path);
            let vault = vault_dir();
            for item in &still_missing {
                let filename = item.filename();
                let Some(bytes) =
                    crate::fetch_manifest_from_mirrors(&client, &filename).await
                else {
                    diag_log!(
                        "[vault] Falta {} (AppID {}) y no está ni en el respaldo ni en el mirror — Steam no va a poder instalar ni verificar este juego.",
                        filename, item.app_id
                    );
                    continue;
                };
                let dest = depotcache.join(&filename);
                if write_protected_file(&dest, &bytes).is_err() {
                    continue;
                }
                // Also into the vault, so this round trip happens once.
                if let Some(v) = vault.as_ref() {
                    let _ = std::fs::create_dir_all(v);
                    let _ = std::fs::write(v.join(&filename), &bytes);
                }
                diag_log!("[vault] {} recuperado del mirror y guardado en el respaldo.", filename);
                restored.push(filename);
            }
        }
    }

    if !restored.is_empty() {
        diag_log!(
            "[vault] {} manifest(s) devueltos a depotcache.",
            restored.len()
        );
    }
    restored
}

// ── Recovering one game at install time ─────────────────────────────────────

/// A depot whose pinned build could not be found anywhere, moved to a build
/// the vault does have.
#[derive(Debug, Clone, PartialEq)]
pub struct Repin {
    pub depot_id: String,
    pub wanted: String,
    pub used: String,
}

/// Puts back, from the vault and then the mirror, the exact manifests one
/// game's `.lua` pins. Returns how many came back.
///
/// An install used to back manifests up and check what was missing, and
/// never put anything back in between. A game removed and downloaded again
/// was reported as missing its manifest with the file sitting in the vault;
/// only the next launch, or the guardian seconds later, restored it — after
/// the install had already failed, and after Hubcap had been spent trying to
/// rescue it.
pub async fn restore_exact_for_app(steam_path: &str, app_id: &str) -> usize {
    let mut restored = 0usize;
    let mut need_mirror = Vec::new();
    for m in missing_for_app(steam_path, app_id) {
        if restore_one(steam_path, &m.depot_id, &m.gid) {
            restored += 1;
        } else {
            need_mirror.push(m);
        }
    }
    if need_mirror.is_empty() {
        return restored;
    }

    let Ok(client) = crate::make_client() else { return restored };
    let depotcache = steam_depotcache(steam_path);
    for m in need_mirror {
        let filename = m.filename();
        let Some(bytes) = crate::fetch_manifest_from_mirrors(&client, &filename).await else {
            continue;
        };
        if write_protected_file(&depotcache.join(&filename), &bytes).is_err() {
            continue;
        }
        if let Some(v) = vault_dir() {
            let _ = std::fs::create_dir_all(&v);
            let _ = std::fs::write(v.join(&filename), &bytes);
        }
        diag_log!("[vault] {} recuperado del mirror para AppID {}.", filename, app_id);
        restored += 1;
    }
    restored
}

/// Everything an install can do from what this machine already has: the
/// exact builds first, then an earlier build the vault kept.
pub async fn recover_for_app(steam_path: &str, app_id: &str) -> Vec<Repin> {
    restore_exact_for_app(steam_path, app_id).await;
    let Some(vault) = vault_dir() else { return Vec::new() };
    fall_back_to_vaulted_builds(steam_path, app_id, &vault)
}

/// Writes a game's ticket again, unchanged, as the last step of an install.
///
/// A user had to press Install twice to get a game going, and the second press
/// did nothing new: same ticket, same manifest, both rewritten. What differed
/// was timing — the second time, every manifest the ticket pins was already on
/// disk when the ticket was written. Manifests can still arrive after the
/// ticket here (restored from the vault, fetched from the mirror), so the
/// ticket is written once more after all of them, which is exactly what the
/// second press did.
pub fn reannounce_ticket(steam_path: &str, app_id: &str) {
    let lua = PathBuf::from(steam_path)
        .join("config")
        .join("lua")
        .join(format!("{}.lua", app_id));
    let Ok(content) = std::fs::read_to_string(&lua) else { return };
    let tmp = lua.with_extension("lua.tmp");
    if std::fs::write(&tmp, content).is_err() || std::fs::rename(&tmp, &lua).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Every gid the vault holds for one depot, most recently saved first.
fn vaulted_gids(vault: &Path, depot_id: &str) -> Vec<String> {
    let prefix = format!("{}_", depot_id);
    let mut found: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(vault)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let gid = name.strip_prefix(&prefix)?.strip_suffix(".manifest")?.to_string();
            if gid.is_empty() || !gid.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let saved = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            Some((saved, gid))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, gid)| gid).collect()
}

/// Which vaulted build to fall back to: the one the ACF says is already on
/// disk when the vault has it — the game's files match that build — and
/// otherwise the most recently saved.
fn pick_fallback(vaulted: &[String], installed_gid: Option<&str>) -> Option<String> {
    installed_gid
        .and_then(|g| vaulted.iter().find(|v| v.as_str() == g))
        .or_else(|| vaulted.first())
        .cloned()
}

/// Rewrites one depot's `setManifestid` to `gid`, or `None` when the text
/// does not pin that depot.
///
/// A third argument, the size some tickets carry, is dropped: it described
/// the build being replaced, and the two-argument form is what the other
/// tickets already use.
fn repin_depot(content: &str, depot_id: &str, gid: &str) -> Option<String> {
    const CALL: &str = "setManifestid(";
    let mut changed = false;
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        let replaced = line.find(CALL).and_then(|start| {
            let args = start + CALL.len();
            let close = args + line[args..].find(')')?;
            let depot = line[args..close].split(',').next()?.trim();
            (depot == depot_id).then(|| {
                format!("{}{}{}, \"{}\"){}", &line[..start], CALL, depot_id, gid, &line[close + 1..])
            })
        });
        match replaced {
            Some(l) => {
                changed = true;
                out.push_str(&l);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    if !content.ends_with('\n') {
        out.pop();
    }
    changed.then_some(out)
}

/// Last resort for a depot whose pinned build is in neither the vault nor the
/// mirror, while the vault holds an earlier build of that same depot.
///
/// The case a user hit: the vault kept the manifest a game installed with,
/// the game was removed, and downloading it again brought a new ticket pinned
/// to a newer build nobody had. The launcher only ever looked for that exact
/// newer file, so the copy it had saved was never used and the install died on
/// Steam's 401. Pinning the depot to the saved build lets it install — the
/// depot's decryption key does not change between builds.
///
/// Only depots with nothing else available are moved. The rest of the game
/// keeps the builds its ticket pins.
fn fall_back_to_vaulted_builds(steam_path: &str, app_id: &str, vault: &Path) -> Vec<Repin> {
    let missing = missing_for_app(steam_path, app_id);
    if missing.is_empty() {
        return Vec::new();
    }

    let installed: HashMap<String, String> = find_acf(steam_path, app_id)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|acf| installed_depots(&acf).into_iter().map(|(d, g, _)| (d, g)).collect())
        .unwrap_or_default();
    let config = PathBuf::from(steam_path).join("config");
    let luas: Vec<PathBuf> = [config.join("lua"), config.join("stplug-in")]
        .iter()
        .map(|dir| dir.join(format!("{}.lua", app_id)))
        .filter(|p| p.is_file())
        .collect();
    let depotcache = steam_depotcache(steam_path);

    let mut repins = Vec::new();
    for m in missing {
        let vaulted = vaulted_gids(vault, &m.depot_id);
        let Some(used) = pick_fallback(&vaulted, installed.get(&m.depot_id).map(String::as_str)) else {
            continue;
        };

        let name = format!("{}_{}.manifest", m.depot_id, used);
        let dest = depotcache.join(&name);
        if !dest.is_file() {
            let _ = std::fs::create_dir_all(&depotcache);
            if let Err(e) = std::fs::copy(vault.join(&name), &dest) {
                diag_log!("[vault] No se pudo copiar {} desde el respaldo: {}", name, e);
                continue;
            }
        }
        make_readonly(&dest);

        let mut pinned = false;
        for lua in &luas {
            let Ok(content) = std::fs::read_to_string(lua) else { continue };
            let Some(updated) = repin_depot(&content, &m.depot_id, &used) else { continue };
            make_writable(lua);
            match std::fs::write(lua, updated) {
                Ok(()) => pinned = true,
                Err(e) => diag_log!("[vault] No se pudo actualizar {}: {}", lua.display(), e),
            }
        }
        if !pinned {
            continue;
        }

        diag_log!(
            "[vault] AppID {}: la build {} del depot {} no está ni en el respaldo ni en el mirror; se usa la {} que guardó la bóveda.",
            app_id, m.gid, m.depot_id, used
        );
        repins.push(Repin { depot_id: m.depot_id, wanted: m.gid, used });
    }
    repins
}

// ── Reading what the .lua files pin ─────────────────────────────────────────

/// Pulls `setManifestid(<depot>, "<gid>")` out of a .lua's text.
///
/// Tolerant about spacing and about the gid being quoted or bare, because both
/// spellings are produced: the ticket generator writes it quoted, and
/// `lock_build_version` writes it bare.
fn parse_set_manifest_ids(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in content.lines() {
        let Some(rest) = line.trim_start().strip_prefix("setManifestid") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('(') else {
            continue;
        };
        let Some(end) = rest.find(')') else { continue };
        let mut args = rest[..end].split(',');
        let (Some(depot), Some(gid)) = (args.next(), args.next()) else {
            continue;
        };
        let clean = |s: &str| s.trim().trim_matches('"').trim().to_string();
        let (depot, gid) = (clean(depot), clean(gid));
        let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
        if numeric(&depot) && numeric(&gid) {
            out.push((depot, gid));
        }
    }
    out
}

/// Every game with a ticket, mapped to the depot→gid pairs it pins.
///
/// Keyed off the .lua filename rather than any `addappid` line in it: a single
/// ticket also declares its DLC and depot ids through that same call, so
/// reading the contents would report dozens of ids that are not games. Same
/// convention `lua_configured_app_ids` uses in main.rs.
fn pinned_by_app(steam_path: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let config = PathBuf::from(steam_path).join("config");
    for dir in [config.join("lua"), config.join("stplug-in")] {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".lua") else { continue };
            if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let pins = parse_set_manifest_ids(&content);
            if pins.is_empty() {
                continue;
            }
            out.entry(stem.to_string()).or_default().extend(pins);
        }
    }
    out
}

// ── Reading and rewriting an ACF ────────────────────────────────────────────

/// Reads one top-level `"Key"\t\t"value"` out of an ACF.
fn read_key(acf: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = acf.find(&needle)? + needle.len();
    let bytes = acf.as_bytes();
    let mut i = pos;
    while i < bytes.len() && (bytes[i] == b'\t' || bytes[i] == b' ') {
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    let start = i + 1;
    let end = start + acf[start..].find('"')?;
    Some(acf[start..end].to_string())
}

/// Replaces one top-level value in place. Returns whether the key was found.
fn set_key(acf: &mut String, key: &str, value: &str) -> bool {
    let needle = format!("\"{}\"", key);
    let Some(found) = acf.find(&needle) else { return false };
    let mut i = found + needle.len();
    {
        let bytes = acf.as_bytes();
        while i < bytes.len() && (bytes[i] == b'\t' || bytes[i] == b' ') {
            i += 1;
        }
        if bytes.get(i) != Some(&b'"') {
            return false;
        }
    }
    let start = i + 1;
    let Some(rel) = acf[start..].find('"') else { return false };
    acf.replace_range(start..start + rel, value);
    true
}

/// The `InstalledDepots` block, as `(depot, gid, byte range of the gid)`.
///
/// The ranges are what makes an in-place repair possible without a full VDF
/// round-trip — an ACF carries fields this launcher has no business
/// reformatting, and rewriting the whole file to change three numbers is how
/// one of them quietly gets dropped.
fn installed_depots(acf: &str) -> Vec<(String, String, Range<usize>)> {
    let mut out = Vec::new();
    let Some(start) = acf.find("\"InstalledDepots\"") else {
        return out;
    };
    let bytes = acf.as_bytes();
    let mut i = start + "\"InstalledDepots\"".len();

    let mut depth = 0i32;
    let mut current_depot: Option<String> = None;
    let mut pending_key: Option<String> = None;
    let mut last_string: Option<String> = None;

    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                if depth == 2 {
                    current_depot = last_string.clone();
                }
                pending_key = None;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
                if depth == 1 {
                    current_depot = None;
                }
                pending_key = None;
                i += 1;
            }
            b'"' => {
                let s0 = i + 1;
                let Some(rel) = acf[s0..].find('"') else { break };
                let end = s0 + rel;
                let text = acf[s0..end].to_string();
                if depth == 2 {
                    match pending_key.take() {
                        Some(k) if k.eq_ignore_ascii_case("manifest") => {
                            if let Some(depot) = current_depot.clone() {
                                out.push((depot, text.clone(), s0..end));
                            }
                        }
                        Some(_) => {}
                        None => pending_key = Some(text.clone()),
                    }
                }
                last_string = Some(text);
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Temp file plus rename, same reasoning as `save_manifest_exclusions`: a
/// truncated ACF is a game Steam refuses to launch, and `fs::write` truncates
/// before it writes.
fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("acf.ragnarok-tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.to_string())
        }
    }
}

/// Locates the ACF for an app across every library.
fn find_acf(steam_path: &str, app_id: &str) -> Option<PathBuf> {
    for lib in ManifestManager::get_library_folders(steam_path) {
        let path = PathBuf::from(&lib)
            .join("steamapps")
            .join(format!("appmanifest_{}.acf", app_id));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Makes the ACF agree with the .lua. Returns `(depot, antes, después)` per
/// change.
fn repair_drift(
    path: &Path,
    pins: &HashMap<String, String>,
) -> Result<Vec<(String, String, String)>, String> {
    let mut content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;

    let flags = read_key(&content, "StateFlags")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    if flags & STATE_FULLY_INSTALLED == 0 {
        return Err("el juego no figura como instalado".to_string());
    }

    // Back to front so every remaining range stays valid as the string shifts.
    let mut changes = Vec::new();
    for (depot, acf_gid, span) in installed_depots(&content).into_iter().rev() {
        let Some(want) = pins.get(&depot) else { continue };
        if want == &acf_gid {
            continue;
        }
        content.replace_range(span, want);
        changes.push((depot, acf_gid, want.clone()));
    }
    if changes.is_empty() {
        return Ok(changes);
    }
    changes.reverse();

    // Correcting the depot list alone is not enough: while 0x2 (UpdateRequired)
    // is still set Steam re-queues the update regardless of what the manifests
    // say, and the loop continues. Clearing it — and the schedule that drives
    // the 30-second retry — is the half that actually stops it.
    set_key(&mut content, "StateFlags", "4");
    if let Some(target) = read_key(&content, "TargetBuildID") {
        if !target.is_empty() && target != "0" {
            set_key(&mut content, "buildid", &target);
        }
    }
    set_key(&mut content, "AutoUpdateBehavior", "1");
    set_key(&mut content, "ScheduledAutoUpdate", "0");
    set_key(&mut content, "UpdateResult", "0");

    write_atomic(path, &content)?;
    Ok(changes)
}

// ── The check itself ────────────────────────────────────────────────────────

/// Verifies every ticketed game against its .lua, optionally repairing.
///
/// Reads only local files — no network, no Steam client. Safe to run at
/// startup.
pub fn check(steam_path: &str, repair: bool) -> Vec<ManifestIssue> {
    if repair {
        protect_depotcache_manifests(steam_path);
    }
    let depotcache = steam_depotcache(steam_path);
    let mut issues = Vec::new();

    for (app_id, pins) in pinned_by_app(steam_path) {
        let acf_path = find_acf(steam_path, &app_id);
        let acf = acf_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        let app_name = acf
            .as_deref()
            .and_then(|c| read_key(c, "name"))
            .unwrap_or_else(|| format!("App {}", app_id));
        let installed = acf
            .as_deref()
            .and_then(|c| read_key(c, "StateFlags"))
            .and_then(|v| v.parse::<u32>().ok())
            .map(|f| f & STATE_FULLY_INSTALLED != 0)
            .unwrap_or(false);

        // ── missing ────────────────────────────────────────────────────────
        for (depot, gid) in &pins {
            if depotcache
                .join(format!("{}_{}.manifest", depot, gid))
                .is_file()
            {
                continue;
            }
            let repaired = repair && restore_one(steam_path, depot, gid);
            issues.push(ManifestIssue {
                app_id: app_id.clone(),
                app_name: app_name.clone(),
                depot_id: depot.clone(),
                kind: "missing".to_string(),
                expected: gid.clone(),
                found: String::new(),
                repaired,
                installed,
                detail: if repaired {
                    "Restaurado desde el respaldo de Ragnarok.".to_string()
                } else if installed {
                    "El manifest no está en depotcache ni en el respaldo. Steam no va a poder actualizar ni verificar este juego — reinstalalo desde Ragnarok.".to_string()
                } else {
                    "El manifest no está en depotcache, pero el juego tampoco está instalado. Se va a volver a bajar solo cuando lo instales.".to_string()
                },
            });
        }

        // ── drift ──────────────────────────────────────────────────────────
        let (Some(acf_path), Some(acf)) = (acf_path, acf) else {
            continue;
        };
        let drifted: Vec<(String, String)> = installed_depots(&acf)
            .into_iter()
            .filter_map(|(depot, acf_gid, _)| {
                let want = pins.get(&depot)?;
                if want == &acf_gid {
                    None
                } else {
                    Some((depot, acf_gid))
                }
            })
            .collect();
        if drifted.is_empty() {
            continue;
        }

        let mut repaired_depots: HashMap<String, String> = HashMap::new();
        let mut failure: Option<String> = None;
        if repair {
            match repair_drift(&acf_path, &pins) {
                Ok(changes) => {
                    diag_log!(
                        "[vault] {} ({}): {} manifest(s) del ACF re-sincronizados con el .lua.",
                        app_name,
                        app_id,
                        changes.len()
                    );
                    for (depot, _, new) in changes {
                        repaired_depots.insert(depot, new);
                    }
                }
                Err(e) => {
                    diag_log!("[vault] No se pudo reparar el ACF de {}: {}", app_id, e);
                    failure = Some(e);
                }
            }
        }

        for (depot, acf_gid) in drifted {
            let ok = repaired_depots.contains_key(&depot);
            let expected = pins.get(&depot).cloned().unwrap_or_default();
            issues.push(ManifestIssue {
                app_id: app_id.clone(),
                app_name: app_name.clone(),
                depot_id: depot,
                kind: "drift".to_string(),
                expected,
                found: acf_gid,
                repaired: ok,
                installed,
                detail: if ok {
                    "El ACF ahora apunta al manifest del .lua.".to_string()
                } else if let Some(e) = &failure {
                    format!("No se pudo reparar: {}", e)
                } else {
                    "El ACF pide un manifest que no está en disco — Steam va a intentar bajarlo y recibir 401.".to_string()
                },
            });
        }
    }

    issues
}

/// Starts a permanent background thread that watches `Steam/depotcache` and
/// immediately restores any `.manifest` file that Steam (or SteamService running
/// as SYSTEM) deletes. The guardian runs for the lifetime of the launcher process.
///
/// Strategy: every `interval_secs` seconds we iterate over every (depot, gid)
/// pair that a pinned `.lua` references. If the expected file is absent from
/// `depotcache` **and** the vault has a copy, we restore it. This is O(pinned
/// manifests) — typically < 200 files — and takes < 1 ms on an SSD.
///
/// Returns immediately; the watcher lives in its own `std::thread`.
pub fn start_depotcache_guardian(steam_path: String, interval_secs: u64) {
    std::thread::spawn(move || {
        diag_log!("[guardian] Depotcache guardian arrancado (intervalo: {}s).", interval_secs);
        let sleep_dur = std::time::Duration::from_secs(interval_secs);
        loop {
            std::thread::sleep(sleep_dur);
            let depotcache = steam_depotcache(&steam_path);
            if !depotcache.is_dir() {
                continue; // Steam not found yet, keep waiting
            }
            let pinned = pinned_by_app(&steam_path);
            let mut restored_count = 0usize;
            for (_app_id, pins) in &pinned {
                for (depot, gid) in pins {
                    let name = format!("{}_{}.manifest", depot, gid);
                    let dest = depotcache.join(&name);
                    if dest.is_file() {
                        // Still present — ensure ReadOnly is still set
                        // (Steam may have cleared it before deleting on next cycle)
                        make_readonly(&dest);
                        continue;
                    }
                    // File is gone — restore immediately from vault
                    if restore_one(&steam_path, depot, gid) {
                        let restored_path = depotcache.join(&name);
                        make_readonly(&restored_path);
                        diag_log!("[guardian] Restaurado: {}", name);
                        restored_count += 1;
                    }
                }
            }
            if restored_count > 0 {
                diag_log!("[guardian] {} manifest(s) restaurados desde vault.", restored_count);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real ticket for ELDEN RING, trimmed to the three base-game depots.
    /// Quoted gids, which is how the generator writes them.
    const ELDEN_LUA: &str = r#"addappid(1245621,0,"5f560fca048a138487e1e634a1e60693a7ad3ae7318c31731d147f16e093f10b")
setManifestid(1245621,"3886039026718227526")
addappid(1245623,0,"b27090a72caa5f3cf2e1f41a81c4bc1c422a6ac277e1cd3a6f1ea68200288d4b")
setManifestid(1245623,"3635787787034027333")
addappid(1245620)"#;

    /// The shape `lock_build_version` writes: bare gid, spaces after commas.
    const LOCKED_LUA: &str = "-- Generated by LuaTools\naddappid(480, 1)\nsetManifestid(480, 1234567890)\n";

    /// The ACF as it actually looked while stuck at "Starting Download":
    /// StateFlags 6, and depot gids from the previous build.
    fn stuck_acf() -> String {
        "\"AppState\"\n{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\"\t\t\"ELDEN RING\"\n\t\"StateFlags\"\t\t\"6\"\n\t\"buildid\"\t\t\"23850278\"\n\t\"UpdateResult\"\t\t\"6\"\n\t\"TargetBuildID\"\t\t\"25080141\"\n\t\"AutoUpdateBehavior\"\t\t\"0\"\n\t\"ScheduledAutoUpdate\"\t\t\"1788936430\"\n\t\"InstalledDepots\"\n\t{\n\t\t\"1245621\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"8950790353897081369\"\n\t\t\t\"size\"\t\t\"54874745987\"\n\t\t}\n\t\t\"1245623\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"8755867293550015179\"\n\t\t\t\"size\"\t\t\"87205590\"\n\t\t}\n\t\t\"1896300\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"7130086783926325209\"\n\t\t\t\"size\"\t\t\"1004172702\"\n\t\t\t\"dlcappid\"\t\t\"1896300\"\n\t\t}\n\t}\n\t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"english\"\n\t}\n}\n".to_string()
    }

    fn elden_pins() -> HashMap<String, String> {
        [
            ("1245621", "3886039026718227526"),
            ("1245623", "3635787787034027333"),
            // Unchanged in the real incident: the DLC depots always matched.
            ("1896300", "7130086783926325209"),
        ]
        .iter()
        .map(|(d, g)| (d.to_string(), g.to_string()))
        .collect()
    }

    #[test]
    fn parses_quoted_and_bare_manifest_ids() {
        assert_eq!(
            parse_set_manifest_ids(ELDEN_LUA),
            vec![
                ("1245621".to_string(), "3886039026718227526".to_string()),
                ("1245623".to_string(), "3635787787034027333".to_string()),
            ]
        );
        assert_eq!(
            parse_set_manifest_ids(LOCKED_LUA),
            vec![("480".to_string(), "1234567890".to_string())]
        );
    }

    /// `addappid` also carries numeric ids; only `setManifestid` may be read.
    #[test]
    fn ignores_addappid_lines() {
        assert!(parse_set_manifest_ids("addappid(1245620)\naddappid(730, 1)\n").is_empty());
    }

    #[test]
    fn reads_every_depot_and_skips_size() {
        let acf = stuck_acf();
        let found: Vec<(String, String)> = installed_depots(&acf)
            .into_iter()
            .map(|(d, g, _)| (d, g))
            .collect();
        assert_eq!(
            found,
            vec![
                ("1245621".to_string(), "8950790353897081369".to_string()),
                ("1245623".to_string(), "8755867293550015179".to_string()),
                ("1896300".to_string(), "7130086783926325209".to_string()),
            ]
        );
    }

    /// The spans have to point at the gid itself, not the key or the size.
    #[test]
    fn spans_cover_exactly_the_gid() {
        let acf = stuck_acf();
        for (_, gid, span) in installed_depots(&acf) {
            assert_eq!(&acf[span], gid);
        }
    }

    #[test]
    fn reads_and_writes_top_level_keys() {
        let mut acf = stuck_acf();
        assert_eq!(read_key(&acf, "StateFlags").as_deref(), Some("6"));
        assert_eq!(read_key(&acf, "TargetBuildID").as_deref(), Some("25080141"));
        assert!(set_key(&mut acf, "StateFlags", "4"));
        assert_eq!(read_key(&acf, "StateFlags").as_deref(), Some("4"));
        assert!(!set_key(&mut acf, "NoSuchKey", "x"));
    }

    /// `"size"` lives inside a depot block and must never be reachable from
    /// the top-level setter, or a repair would corrupt the depot list.
    #[test]
    fn set_key_leaves_depot_sizes_alone() {
        let mut acf = stuck_acf();
        set_key(&mut acf, "StateFlags", "4");
        assert!(acf.contains("\"size\"\t\t\"54874745987\""));
    }

    #[test]
    fn repair_rewrites_drifted_gids_and_clears_the_update_flag() {
        let path = std::env::temp_dir().join("ragnarok_vault_test_repair.acf");
        std::fs::write(&path, stuck_acf()).unwrap();

        let changes = repair_drift(&path, &elden_pins()).unwrap();
        assert_eq!(changes.len(), 2, "solo los dos depots base derivaron");

        let after = std::fs::read_to_string(&path).unwrap();
        let gids: Vec<String> = installed_depots(&after)
            .into_iter()
            .map(|(_, g, _)| g)
            .collect();
        assert_eq!(
            gids,
            vec![
                "3886039026718227526".to_string(),
                "3635787787034027333".to_string(),
                "7130086783926325209".to_string(),
            ]
        );

        // Without these the loop survives the manifest fix.
        assert_eq!(read_key(&after, "StateFlags").as_deref(), Some("4"));
        assert_eq!(read_key(&after, "buildid").as_deref(), Some("25080141"));
        assert_eq!(read_key(&after, "AutoUpdateBehavior").as_deref(), Some("1"));
        assert_eq!(read_key(&after, "ScheduledAutoUpdate").as_deref(), Some("0"));
        assert_eq!(read_key(&after, "UpdateResult").as_deref(), Some("0"));

        // Untouched fields survive the in-place edit.
        assert_eq!(read_key(&after, "name").as_deref(), Some("ELDEN RING"));
        assert!(after.contains("\"language\"\t\t\"english\""));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn repair_is_a_no_op_when_nothing_drifted() {
        let path = std::env::temp_dir().join("ragnarok_vault_test_noop.acf");
        std::fs::write(&path, stuck_acf()).unwrap();
        let pins: HashMap<String, String> = [
            ("1245621", "8950790353897081369"),
            ("1245623", "8755867293550015179"),
        ]
        .iter()
        .map(|(d, g)| (d.to_string(), g.to_string()))
        .collect();

        assert!(repair_drift(&path, &pins).unwrap().is_empty());
        // A file that needed nothing must come back byte-identical, flags
        // included — StateFlags 6 here is Steam's business, not ours.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), stuck_acf());
        let _ = std::fs::remove_file(&path);
    }

    /// StateFlags 514 (Update Required + Update Paused) is exactly the state
    /// "How to Fish" was left in. Rewriting its depot list would claim an
    /// install that is not there.
    #[test]
    fn repair_refuses_a_game_that_is_not_installed() {
        let path = std::env::temp_dir().join("ragnarok_vault_test_uninstalled.acf");
        let acf = stuck_acf().replace("\"StateFlags\"\t\t\"6\"", "\"StateFlags\"\t\t\"514\"");
        std::fs::write(&path, &acf).unwrap();

        assert!(repair_drift(&path, &elden_pins()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), acf);
        let _ = std::fs::remove_file(&path);
    }

    /// Runs the real check against a real Steam install, reporting only —
    /// never repairing. Ignored by default because it depends on the machine
    /// it runs on; point `RAGNAROK_STEAM_PATH` at an install and run with
    /// `cargo test --bin Ragnarok-launcher -- --ignored --nocapture`.
    /// The whole point of the vault, end to end: Steam deletes a manifest,
    /// the next launch puts it back, and no network is involved.
    ///
    /// Uses a throwaway Steam layout under the temp dir, but the real vault
    /// directory — so it is skipped unless the vault is reachable.
    #[test]
    fn a_pruned_manifest_comes_back_on_the_next_start() {
        let Some(vault) = vault_dir() else { return };
        if std::fs::create_dir_all(&vault).is_err() {
            return;
        }

        // A depot id no real game uses, so this never collides with the
        // machine's actual vault contents.
        let depot = "999000111";
        let gid = "4242424242424242424";
        let name = format!("{}_{}.manifest", depot, gid);

        let steam = std::env::temp_dir().join("ragnarok_vault_roundtrip");
        let depotcache = steam.join("depotcache");
        let lua_dir = steam.join("config").join("lua");
        std::fs::create_dir_all(&depotcache).unwrap();
        std::fs::create_dir_all(&lua_dir).unwrap();
        std::fs::write(
            lua_dir.join("4001890.lua"),
            format!("addappid({depot},0,\"ab\")
setManifestid({depot},\"{gid}\")
addappid(4001890)"),
        )
        .unwrap();
        std::fs::write(depotcache.join(&name), b"fake manifest bytes").unwrap();

        let steam_str = steam.to_string_lossy().to_string();

        // 1. A normal run backs it up.
        back_up(&steam_str);
        assert!(vault.join(&name).is_file(), "el respaldo tiene que quedar guardado");

        // 2. Steam prunes it, exactly as it did to How to Fish on uninstall.
        std::fs::remove_file(depotcache.join(&name)).unwrap();
        assert!(!depotcache.join(&name).is_file());

        // 3. The next start puts it back, and says which one.
        let (restored, missing) = restore_from_vault(&steam_str);
        assert_eq!(restored, vec![name.clone()]);
        assert!(missing.is_empty(), "no debe quedar nada pendiente de red");
        assert!(depotcache.join(&name).is_file(), "el manifest tiene que volver");
        assert_eq!(
            std::fs::read(depotcache.join(&name)).unwrap(),
            b"fake manifest bytes",
            "y volver intacto"
        );

        // 4. A second start has nothing to do — no churn on every launch.
        assert!(restore_from_vault(&steam_str).0.is_empty());

        let _ = std::fs::remove_file(vault.join(&name));
        make_writable(&depotcache.join(&name));
        let _ = std::fs::remove_dir_all(&steam);
    }

    /// Steam's own copy always wins: the vault fills gaps, it does not
    /// overwrite. A newer manifest Steam fetched itself must survive.
    #[test]
    fn restoring_never_overwrites_what_steam_already_has() {
        let Some(vault) = vault_dir() else { return };
        if std::fs::create_dir_all(&vault).is_err() {
            return;
        }
        let depot = "999000222";
        let gid = "1313131313131313131";
        let name = format!("{}_{}.manifest", depot, gid);

        let steam = std::env::temp_dir().join("ragnarok_vault_nooverwrite");
        let depotcache = steam.join("depotcache");
        let lua_dir = steam.join("config").join("lua");
        std::fs::create_dir_all(&depotcache).unwrap();
        std::fs::create_dir_all(&lua_dir).unwrap();
        std::fs::write(
            lua_dir.join("4001891.lua"),
            format!("setManifestid({depot},\"{gid}\")"),
        )
        .unwrap();

        std::fs::write(vault.join(&name), b"copia vieja del respaldo").unwrap();
        std::fs::write(depotcache.join(&name), b"la de Steam, mas nueva").unwrap();

        let steam_str = steam.to_string_lossy().to_string();
        assert!(restore_from_vault(&steam_str).0.is_empty(), "no hay hueco que llenar");
        assert_eq!(
            std::fs::read(depotcache.join(&name)).unwrap(),
            b"la de Steam, mas nueva"
        );

        let _ = std::fs::remove_file(vault.join(&name));
        let _ = std::fs::remove_dir_all(&steam);
    }

    /// Runs the real restore — vault first, then the mirror — against a real
    /// install, and actually writes the files it recovers.
    ///
    /// Ignored by default because it touches the network and the machine's own
    /// Steam folder. Point `RAGNAROK_STEAM_PATH` at an install and run with
    /// `cargo test --bin Ragnarok-launcher -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn smoke_restore_a_real_install() {
        let Ok(steam_path) = std::env::var("RAGNAROK_STEAM_PATH") else {
            return;
        };
        let (from_vault, still_missing) = restore_from_vault(&steam_path);
        eprintln!(
            "desde el respaldo: {} | pendientes para el mirror: {}",
            from_vault.len(),
            still_missing.len()
        );

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let restored = rt.block_on(restore_missing(&steam_path));
        eprintln!("recuperados en total: {}", restored.len());
        for r in &restored {
            eprintln!("  + {}", r);
        }
    }

    /// What the post-install check reports: only this app's pins, only the
    /// ones actually absent from disk.
    #[test]
    fn missing_for_app_reports_only_this_games_absent_pins() {
        let steam = std::env::temp_dir().join("ragnarok_missing_for_app");
        let _ = std::fs::remove_dir_all(&steam);
        let depotcache = steam.join("depotcache");
        let lua_dir = steam.join("config").join("lua");
        std::fs::create_dir_all(&depotcache).unwrap();
        std::fs::create_dir_all(&lua_dir).unwrap();

        // Our game pins two depots; only one of them is on disk.
        std::fs::write(
            lua_dir.join("4001890.lua"),
            "setManifestid(4001891,\"111\")
setManifestid(4001892,\"222\")",
        )
        .unwrap();
        std::fs::write(depotcache.join("4001891_111.manifest"), b"presente").unwrap();

        // A different game, also missing one — must not appear in our answer.
        std::fs::write(lua_dir.join("730.lua"), "setManifestid(731,\"333\")").unwrap();

        let steam_str = steam.to_string_lossy().to_string();
        let missing = missing_for_app(&steam_str, "4001890");

        assert_eq!(missing.len(), 1, "sólo el que falta: {missing:?}");
        assert_eq!(missing[0].depot_id, "4001892");
        assert_eq!(missing[0].gid, "222");
        assert_eq!(missing[0].app_id, "4001890");

        // Nothing pinned, or nothing missing, is an empty answer — never a
        // false alarm on a healthy install.
        assert!(missing_for_app(&steam_str, "999999").is_empty());
        std::fs::write(depotcache.join("4001892_222.manifest"), b"ya presente").unwrap();
        assert!(missing_for_app(&steam_str, "4001890").is_empty());

        let _ = std::fs::remove_dir_all(&steam);
    }

    /// The user's actual workflow, which is what exposed the gap: install a
    /// game, remove it again a minute later to see whether the error is gone,
    /// never restarting the launcher in between.
    ///
    /// Backing up only at startup lost the manifest every single time, because
    /// it arrived and was destroyed inside one session. The vault has to
    /// capture it when it lands and again before an uninstall removes it.
    #[test]
    fn a_manifest_that_arrives_and_is_deleted_in_one_session_survives() {
        let Some(vault) = vault_dir() else { return };
        if std::fs::create_dir_all(&vault).is_err() {
            return;
        }

        let depot = "999000333";
        let gid = "7777777777777777777";
        let name = format!("{}_{}.manifest", depot, gid);
        let _ = std::fs::remove_file(vault.join(&name));

        let steam = std::env::temp_dir().join("ragnarok_vault_same_session");
        let depotcache = steam.join("depotcache");
        let lua_dir = steam.join("config").join("lua");
        std::fs::create_dir_all(&depotcache).unwrap();
        std::fs::create_dir_all(&lua_dir).unwrap();
        let steam_str = steam.to_string_lossy().to_string();

        // Startup: nothing installed yet, so the vault has nothing to take.
        back_up(&steam_str);
        assert!(
            !vault.join(&name).is_file(),
            "no puede respaldar algo que todavía no existe"
        );

        // The install writes the ticket and the manifest.
        std::fs::write(
            lua_dir.join("4001890.lua"),
            format!("addappid({depot},0,\"ab\")
setManifestid({depot},\"{gid}\")"),
        )
        .unwrap();
        std::fs::write(depotcache.join(&name), b"manifest recien instalado").unwrap();

        // This is the call install_game now makes before returning.
        back_up(&steam_str);
        assert!(
            vault.join(&name).is_file(),
            "el respaldo tiene que ocurrir al instalar, no en el próximo arranque"
        );

        // Uninstall wipes it from depotcache — no restart in between.
        std::fs::remove_file(depotcache.join(&name)).unwrap();

        // And the vault can hand it straight back — from disk, with no
        // network involved, which is the point: the mirror does not carry
        // every manifest and Steam's CDN answers 401 without ownership.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(rt.block_on(restore_missing(&steam_str)), vec![name.clone()]);
        assert_eq!(
            std::fs::read(depotcache.join(&name)).unwrap(),
            b"manifest recien instalado"
        );

        let _ = std::fs::remove_file(vault.join(&name));
        make_writable(&depotcache.join(&name));
        let _ = std::fs::remove_dir_all(&steam);
    }

    /// Fills the vault from a real install. Same opt-in as the check above.
    #[test]
    #[ignore]
    fn smoke_back_up_a_real_install() {
        let Ok(steam_path) = std::env::var("RAGNAROK_STEAM_PATH") else {
            return;
        };
        let (copied, seen) = back_up(&steam_path);
        eprintln!(
            "respaldados {} de {} manifest(s) -> {:?}",
            copied,
            seen,
            vault_dir()
        );
        assert_eq!(seen > 0, vault_dir().map(|v| v.is_dir()).unwrap_or(false));
    }

    #[test]
    #[ignore]
    fn smoke_check_against_a_real_install() {
        let Ok(steam_path) = std::env::var("RAGNAROK_STEAM_PATH") else {
            eprintln!("RAGNAROK_STEAM_PATH sin definir — nada que revisar.");
            return;
        };
        let mut issues = check(&steam_path, false);
        // Los que afectan a un juego instalado primero: son los unicos que
        // rompen algo hoy.
        issues.sort_by_key(|i| (!i.installed, i.app_id.clone()));
        let real = issues.iter().filter(|i| i.installed).count();
        eprintln!(
            "{} problema(s), {} sobre juegos instalados:",
            issues.len(),
            real
        );
        for i in &issues {
            eprintln!(
                "  {} [{}] {} ({}) depot {} — espera {} / ACF {}",
                if i.installed { "INSTALADO" } else { "         " },
                i.kind,
                i.app_name,
                i.app_id,
                i.depot_id,
                i.expected,
                if i.found.is_empty() { "-" } else { &i.found }
            );
        }
    }

    #[test]
    fn missing_installed_depots_block_yields_nothing() {
        assert!(installed_depots("\"AppState\"\n{\n\t\"appid\"\t\t\"1\"\n}\n").is_empty());
    }

    #[test]
    fn protect_depotcache_manifests_marks_manifests_readonly() {
        let steam = std::env::temp_dir().join("ragnarok_test_protect_manifests");
        let depotcache = steam.join("depotcache");
        let _ = std::fs::create_dir_all(&depotcache);
        let m1 = depotcache.join("100_200.manifest");
        let m2 = depotcache.join("not_a_manifest.txt");
        std::fs::write(&m1, b"manifest data").unwrap();
        std::fs::write(&m2, b"other data").unwrap();

        let count = protect_depotcache_manifests(&steam.to_string_lossy());
        assert_eq!(count, 1);

        let perm1 = std::fs::metadata(&m1).unwrap().permissions();
        assert!(perm1.readonly(), "El manifiesto debe quedar como solo lectura");

        let perm2 = std::fs::metadata(&m2).unwrap().permissions();
        assert!(!perm2.readonly(), "Otros archivos no deben afectarse");

        // write_protected_file can overwrite a readonly file without error
        write_protected_file(&m1, b"updated manifest").unwrap();
        assert_eq!(std::fs::read(&m1).unwrap(), b"updated manifest");
        assert!(std::fs::metadata(&m1).unwrap().permissions().readonly());

        make_writable(&m1);
        let _ = std::fs::remove_dir_all(&steam);
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;

    #[test]
    fn repinning_touches_only_that_depot() {
        let lua = "addappid(3751951,0,\"ab\")\nsetManifestid(3751951, \"4397710407098141927\")\nsetManifestid(3751953, \"3022415893432196011\")\n";
        let out = repin_depot(lua, "3751951", "5857658919173276733").unwrap();
        assert_eq!(
            out,
            "addappid(3751951,0,\"ab\")\nsetManifestid(3751951, \"5857658919173276733\")\nsetManifestid(3751953, \"3022415893432196011\")\n"
        );
        assert_eq!(parse_set_manifest_ids(&out)[0].1, "5857658919173276733");

        // Bare gid, and a size argument that described the other build.
        assert_eq!(
            repin_depot("setManifestid(2661301, \"1826397799927201919\", 39210508054)", "2661301", "7").unwrap(),
            "setManifestid(2661301, \"7\")"
        );
        assert_eq!(repin_depot("setManifestid(480, 123)", "480", "9").unwrap(), "setManifestid(480, \"9\")");

        // A depot id that merely starts the same is a different depot.
        assert!(repin_depot("setManifestid(37519510, \"1\")", "3751951", "2").is_none());
        assert!(repin_depot("addappid(3751951)", "3751951", "2").is_none());
    }

    #[test]
    fn reannouncing_a_ticket_leaves_it_exactly_as_it_was() {
        let steam = std::env::temp_dir().join("ragnarok_vault_reannounce");
        let _ = std::fs::remove_dir_all(&steam);
        let lua_dir = steam.join("config").join("lua");
        std::fs::create_dir_all(&lua_dir).unwrap();
        let ticket = "addappid(2916430)\nsetManifestid(2916431,\"222271375584572596\")\n";
        std::fs::write(lua_dir.join("2916430.lua"), ticket).unwrap();

        let steam_str = steam.to_string_lossy().to_string();
        reannounce_ticket(&steam_str, "2916430");
        assert_eq!(std::fs::read_to_string(lua_dir.join("2916430.lua")).unwrap(), ticket);
        assert!(!lua_dir.join("2916430.lua.tmp").exists());

        // No ticket, nothing written.
        reannounce_ticket(&steam_str, "999");
        assert!(!lua_dir.join("999.lua").exists());
        let _ = std::fs::remove_dir_all(&steam);
    }

    #[test]
    fn the_build_already_on_disk_is_preferred() {
        let vaulted = vec!["30".to_string(), "20".to_string(), "10".to_string()];
        assert_eq!(pick_fallback(&vaulted, Some("20")).as_deref(), Some("20"));
        assert_eq!(pick_fallback(&vaulted, Some("99")).as_deref(), Some("30"), "si no, la más reciente");
        assert_eq!(pick_fallback(&vaulted, None).as_deref(), Some("30"));
        assert_eq!(pick_fallback(&[], Some("20")), None);
    }

    /// The reported case, end to end: the vault kept the build the game was
    /// installed with, and the new ticket pins one nobody has.
    #[test]
    fn a_new_ticket_falls_back_to_the_build_the_vault_kept() {
        let root = std::env::temp_dir().join("ragnarok_vault_fallback");
        let _ = std::fs::remove_dir_all(&root);
        let steam = root.join("steam");
        let vault = root.join("vault");
        let depotcache = steam.join("depotcache");
        let lua_dir = steam.join("config").join("lua");
        for d in [&vault, &depotcache, &lua_dir] {
            std::fs::create_dir_all(d).unwrap();
        }

        let lua = lua_dir.join("3751950.lua");
        std::fs::write(
            &lua,
            "addappid(3751950)\nsetManifestid(3751951, \"4397710407098141927\")\nsetManifestid(3751953, \"222\")\nsetManifestid(3751954, \"333\")\n",
        )
        .unwrap();
        // 3751953 arrived with the new ticket; 3751954 exists nowhere at all.
        std::fs::write(depotcache.join("3751953_222.manifest"), b"nuevo").unwrap();
        std::fs::write(vault.join("3751951_5857658919173276733.manifest"), b"la de la boveda").unwrap();

        let steam_str = steam.to_string_lossy().to_string();
        let repins = fall_back_to_vaulted_builds(&steam_str, "3751950", &vault);

        assert_eq!(
            repins,
            vec![Repin {
                depot_id: "3751951".into(),
                wanted: "4397710407098141927".into(),
                used: "5857658919173276733".into(),
            }]
        );
        let dest = depotcache.join("3751951_5857658919173276733.manifest");
        assert_eq!(std::fs::read(&dest).unwrap(), b"la de la boveda");

        let pins: HashMap<String, String> = parse_set_manifest_ids(&std::fs::read_to_string(&lua).unwrap())
            .into_iter()
            .collect();
        assert_eq!(pins["3751951"], "5857658919173276733", "el .lua pide ahora lo que hay");
        assert_eq!(pins["3751953"], "222", "lo que sí llegó no se toca");
        assert_eq!(pins["3751954"], "333", "sin copia en ningún lado, queda como estaba");

        let still: Vec<String> = missing_for_app(&steam_str, "3751950").into_iter().map(|m| m.depot_id).collect();
        assert_eq!(still, ["3751954"]);

        // A second run has nothing left it can do.
        assert!(fall_back_to_vaulted_builds(&steam_str, "3751950", &vault).is_empty());

        make_writable(&dest);
        let _ = std::fs::remove_dir_all(&root);
    }
}
