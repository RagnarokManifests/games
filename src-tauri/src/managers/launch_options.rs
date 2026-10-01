// Reads/writes a single Steam app's Launch Options — the exact same field
// Properties > General > Launch Options edits in the real Steam client —
// stored in the per-account `userdata/<userid>/config/localconfig.vdf` file,
// under `UserLocalConfigStore > Software > Valve > Steam > apps > <appid> >
// LaunchOptions`. Feature parity with steamedit.tg-software.com and the
// "Edit Launch Options" feature LuaTools v1.2.7 added — this saves the user
// a manual trip through Steam's own Properties dialog (relevant here mainly
// for OnlineFix games, which need `-onlinefix` added by hand today).
//
// This is Valve's plain-TEXT KeyValue format (quote-delimited keys/values,
// brace-nested blocks) — NOT the same as the binary KeyValue format
// `managers::vdf` reads for the achievement schema cache; that's a
// completely different file/encoding, nothing here reuses it.
//
// localconfig.vdf holds a LOT of other real Steam settings beyond launch
// options — corrupting it would be disruptive, so this is deliberately a
// narrow, targeted editor (only ever touches the one `LaunchOptions` leaf
// under one specific app's block), always backs up the original first, and
// verifies its own write by re-reading the new content back before trusting
// it — restoring the backup and failing loudly if that check doesn't pass,
// rather than risking a silently-corrupted file.

use std::path::PathBuf;

fn resolve_vdf_path(steam_path: &str) -> Result<PathBuf, String> {
    let userdata_dir = crate::managers::cloud_saves::resolve_userdata_dir(steam_path)
        .ok_or_else(|| "No se encontró la carpeta userdata de Steam — inicia sesión en Steam al menos una vez antes de usar esto.".to_string())?;
    Ok(userdata_dir.join("config").join("localconfig.vdf"))
}

/// Finds the byte offset of the `}` matching the `{` that was just opened at
/// `open_brace_pos` (i.e. scanning starts at `open_brace_pos + 1`) —
/// quote-aware, so a `{`/`}` character appearing inside a quoted string
/// (escaped as `\"` for literal quotes within it) never miscounts depth.
fn skip_block(bytes: &[u8], open_brace_pos: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut i = open_brace_pos + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' && bytes[i - 1] != b'\\' {
                        break;
                    }
                    i += 1;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

pub(crate) enum VdfEntry {
    /// A `"key" { ... }` block — byte range is its contents, between the
    /// braces (exclusive of both).
    Block(usize, usize),
    /// A `"key" "value"` leaf — byte range is the value's contents
    /// (exclusive of the surrounding quotes).
    Leaf(usize, usize),
}

/// Scans `text[range]` at its own top level only (children of nested blocks
/// are skipped over whole, never mistaken for a sibling at this level) for a
/// key matching `target` (case-insensitive, matching Steam's own VDF key
/// lookup). Returns its block or leaf range along with where the WHOLE
/// entry (key quote through value/block end) started and ended, for
/// in-place replacement.
pub(crate) fn find_entry(text: &str, range: (usize, usize), target: &str) -> Option<(VdfEntry, usize, usize)> {
    let bytes = text.as_bytes();
    let (start, end) = range;
    let mut i = start;
    while i < end {
        if bytes[i] == b'"' {
            let entry_start = i;
            let key_start = i + 1;
            let mut j = key_start;
            while j < end {
                if bytes[j] == b'"' && bytes[j - 1] != b'\\' {
                    break;
                }
                j += 1;
            }
            if j >= end {
                return None; // Unterminated string — malformed file, bail out.
            }
            let key = &text[key_start..j];
            let is_match = key.eq_ignore_ascii_case(target);
            let mut k = j + 1;
            while k < end && (bytes[k] as char).is_whitespace() {
                k += 1;
            }
            if k >= end {
                return None;
            }
            if bytes[k] == b'{' {
                let close = skip_block(bytes, k)?;
                if is_match {
                    return Some((VdfEntry::Block(k + 1, close), entry_start, close + 1));
                }
                i = close + 1;
            } else if bytes[k] == b'"' {
                let val_start = k + 1;
                let mut m = val_start;
                while m < end {
                    if bytes[m] == b'"' && bytes[m - 1] != b'\\' {
                        break;
                    }
                    m += 1;
                }
                if m >= end {
                    return None;
                }
                if is_match {
                    return Some((VdfEntry::Leaf(val_start, m), entry_start, m + 1));
                }
                i = m + 1;
            } else {
                // A bare/unquoted token after a key — not a shape this file
                // uses; skip just past the key and keep scanning.
                i = k;
            }
        } else {
            i += 1;
        }
    }
    None
}

fn unescape_vdf_value(raw: &str) -> String {
    raw.replace("\\\"", "\"").replace("\\\\", "\\")
}

fn escape_vdf_value(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Walks a fixed path of block keys from the top of the file, returning the
/// final block's content range.
pub(crate) fn find_block_path(text: &str, path: &[&str]) -> Option<(usize, usize)> {
    let mut range = (0usize, text.len());
    for key in path {
        match find_entry(text, range, key)? {
            (VdfEntry::Block(s, e), ..) => range = (s, e),
            (VdfEntry::Leaf(..), ..) => return None,
        }
    }
    Some(range)
}

pub fn get_launch_options(steam_path: &str, app_id: &str) -> Result<Option<String>, String> {
    let vdf_path = resolve_vdf_path(steam_path)?;
    let text = match std::fs::read_to_string(&vdf_path) {
        Ok(t) => t,
        Err(_) => return Ok(None), // No config yet (fresh account) — nothing to report.
    };
    let Some(apps_range) = find_block_path(&text, &["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]) else {
        return Ok(None);
    };
    let Some((VdfEntry::Block(app_start, app_end), ..)) = find_entry(&text, apps_range, app_id) else {
        return Ok(None);
    };
    match find_entry(&text, (app_start, app_end), "LaunchOptions") {
        Some((VdfEntry::Leaf(vs, ve), ..)) => Ok(Some(unescape_vdf_value(&text[vs..ve]))),
        _ => Ok(None),
    }
}

pub fn set_launch_options(steam_path: &str, app_id: &str, value: &str) -> Result<(), String> {
    if !app_id.chars().all(|c| c.is_ascii_digit()) {
        return Err("AppID inválido.".to_string());
    }
    let vdf_path = resolve_vdf_path(steam_path)?;
    let original = std::fs::read_to_string(&vdf_path)
        .map_err(|e| format!("No se pudo leer localconfig.vdf: {}. ¿Iniciaste sesión en Steam al menos una vez en esta PC?", e))?;

    let backup_path = vdf_path.with_extension("vdf.ragnarok_backup");
    std::fs::write(&backup_path, &original).map_err(|e| e.to_string())?;

    let Some(apps_range) = find_block_path(&original, &["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]) else {
        return Err("No se encontró la sección \"apps\" en localconfig.vdf — el formato puede haber cambiado, o esta cuenta nunca inició Steam.".to_string());
    };

    let escaped = escape_vdf_value(value.trim());

    // Two levels of "find the leaf, replace in place; else insert right
    // after the enclosing block's opening brace" — first for the app's own
    // block within "apps", then for LaunchOptions within the app's block.
    let updated = match find_entry(&original, apps_range, app_id) {
        Some((VdfEntry::Block(app_start, app_end), _, _)) => {
            match find_entry(&original, (app_start, app_end), "LaunchOptions") {
                Some((VdfEntry::Leaf(vs, ve), _, _)) => {
                    format!("{}{}{}", &original[..vs], escaped, &original[ve..])
                }
                _ => {
                    let insertion = format!("\n\t\t\t\t\"LaunchOptions\"\t\t\"{}\"", escaped);
                    format!("{}{}{}", &original[..app_start], insertion, &original[app_start..])
                }
            }
        }
        _ => {
            let (apps_start, _apps_end) = apps_range;
            let insertion = format!(
                "\n\t\t\t\"{}\"\n\t\t\t{{\n\t\t\t\t\"LaunchOptions\"\t\t\"{}\"\n\t\t\t}}",
                app_id, escaped
            );
            format!("{}{}{}", &original[..apps_start], insertion, &original[apps_start..])
        }
    };

    // Sanity checks before trusting this write: braces still balance
    // (outside quoted strings), and reading LaunchOptions back from the new
    // content actually returns what we just set. Either failing means a bug
    // in the scan/insert logic above, not just an unusual file — restore the
    // backup rather than leave a broken localconfig.vdf in place.
    if !braces_balanced(&updated) {
        let _ = std::fs::remove_file(&backup_path);
        return Err("Edición cancelada: el archivo resultante no quedó balanceado. No se modificó nada.".to_string());
    }
    // Write to a sibling and rename into place.
    //
    // A plain `fs::write` truncates the target and then streams into it, so a
    // failure partway — a full disk, a transient lock from Steam — left
    // localconfig.vdf truncated. And the `?` on it returned before reaching
    // the verification below, so nothing restored the backup either: the one
    // path in this function that can actually corrupt the file was the one
    // path that skipped its own safety net, contradicting this module's own
    // documented promise. That file holds the user's friends, controller
    // configs and every game's launch options.
    //
    // Rename is atomic on one volume, and the temp file is a sibling, so a
    // reader sees either the old file or the new one — never half of either.
    let tmp_path = vdf_path.with_extension("vdf.ragnarok-tmp");
    if let Err(e) = std::fs::write(&tmp_path, &updated) {
        let _ = std::fs::remove_file(&tmp_path);
        let _ = std::fs::remove_file(&backup_path);
        return Err(format!("No se pudo escribir el archivo temporal: {}", e));
    }
    if let Err(e) = std::fs::rename(&tmp_path, &vdf_path) {
        let _ = std::fs::remove_file(&tmp_path);
        let _ = std::fs::remove_file(&backup_path);
        return Err(format!(
            "No se pudo reemplazar localconfig.vdf: {}. El archivo original quedó intacto.",
            e
        ));
    }

    match get_launch_options(steam_path, app_id) {
        Ok(Some(v)) if v == value.trim() => {
            let _ = std::fs::remove_file(&backup_path);
            Ok(())
        }
        _ => {
            // Read-your-write failed — restore the pre-edit file immediately.
            let _ = std::fs::write(&vdf_path, &original);
            let _ = std::fs::remove_file(&backup_path);
            Err("Edición cancelada: no se pudo verificar el cambio después de escribirlo. Se restauró el archivo original — nada quedó modificado.".to_string())
        }
    }
}

fn braces_balanced(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' && bytes[i - 1] != b'\\' {
                        break;
                    }
                    i += 1;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    depth == 0
}
