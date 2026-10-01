// Minimal reader for Valve's binary VDF (KeyValue) format, used only to
// parse `<steam>\appcache\stats\UserGameStatsSchema_<appid>.bin` — the
// achievement/stat schema Steam's own client downloads and caches locally
// for any game it has ever fetched stats for. This is the exact file
// github.com/gibbed/SteamAchievementManager reads (SAM.Game/KeyValue.cs +
// SAM.Game/Manager.cs's LoadUserGameStatsSchema), which is what `ach_helper`
// in this project is already modeled on for the live achievement
// set/clear/get actions — this module covers the schema (name/description/
// icon) side, which SAM gets from this local file instead of any web API.
//
// Byte format (verified against SAM.Game/KeyValue.cs + KeyValueType.cs +
// StreamHelpers.cs, not guessed): a flat sequence of (type byte, name,
// value) triples, where type 0 (None) means "value is a nested sequence of
// the same triples, terminated by a type-8 (End) byte". Every object —
// including the implicit root — is terminated by one End byte. Strings
// (names and String-typed values) are UTF-8, null-terminated.

use std::io::Read;
use std::path::Path;

const TYPE_NONE: u8 = 0;
const TYPE_STRING: u8 = 1;
const TYPE_INT32: u8 = 2;
const TYPE_FLOAT32: u8 = 3;
const TYPE_POINTER: u8 = 4;
const TYPE_WSTRING: u8 = 5;
const TYPE_COLOR: u8 = 6;
const TYPE_UINT64: u8 = 7;
const TYPE_END: u8 = 8;

pub enum Kv {
    Nested(Vec<(String, Kv)>),
    Str(String),
    Int32(i32),
    Float32(f32),
    UInt64(u64),
    UInt32(u32),
}

impl Kv {
    pub fn get(&self, key: &str) -> Option<&Kv> {
        match self {
            Kv::Nested(children) => children.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn children(&self) -> &[(String, Kv)] {
        match self {
            Kv::Nested(c) => c,
            _ => &[],
        }
    }

    pub fn as_string(&self) -> Option<String> {
        match self {
            Kv::Str(s) => Some(s.clone()),
            Kv::Int32(i) => Some(i.to_string()),
            Kv::Float32(f) => Some(f.to_string()),
            Kv::UInt64(u) => Some(u.to_string()),
            Kv::UInt32(u) => Some(u.to_string()),
            Kv::Nested(_) => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Kv::Str(s) => s.trim().parse::<i64>().map(|n| n != 0).unwrap_or(false),
            Kv::Int32(i) => *i != 0,
            Kv::UInt64(u) => *u != 0,
            Kv::UInt32(u) => *u != 0,
            Kv::Float32(f) => *f != 0.0,
            Kv::Nested(_) => false,
        }
    }

    /// Mirrors SAM's GetLocalizedString: try `lang`, then "english", then
    /// treat the node itself as a plain (non-localized) string value.
    pub fn localized(&self, lang: &str) -> Option<String> {
        if let Some(v) = self.get(lang).and_then(|n| n.as_string()) {
            if !v.is_empty() {
                return Some(v);
            }
        }
        if !lang.eq_ignore_ascii_case("english") {
            if let Some(v) = self.get("english").and_then(|n| n.as_string()) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        self.as_string().filter(|v| !v.is_empty())
    }
}

fn read_cstring_utf8(r: &mut impl Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        r.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// How deep a binary VDF is allowed to nest.
///
/// Without a limit this recursion had no floor, and a corrupt file is not
/// hypothetical here: these are Steam's `appcache/stats/*.bin` schemas, and
/// `appcache` is a folder this launcher's own repair button deletes. A run of
/// 0x00 bytes followed by short names produces thousands of nesting levels,
/// each one a stack frame plus a Vec — and a stack overflow is not a catchable
/// panic. The process dies outright, so the user sees the launcher vanish when
/// opening the achievements panel, with no dialog and nothing in any log.
///
/// 32 matches the guard appinfo.rs already uses for the same reason. Real
/// schemas nest three or four levels.
const MAX_DEPTH: u32 = 32;

fn read_children(r: &mut impl Read, depth: u32) -> Result<Vec<(String, Kv)>, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "El archivo VDF anida más de {} niveles — está corrupto.",
            MAX_DEPTH
        ));
    }
    let mut out = Vec::new();
    loop {
        let mut type_byte = [0u8; 1];
        r.read_exact(&mut type_byte).map_err(|e| e.to_string())?;
        let t = type_byte[0];
        if t == TYPE_END {
            break;
        }
        let name = read_cstring_utf8(r)?;
        let value = match t {
            TYPE_NONE => Kv::Nested(read_children(r, depth + 1)?),
            TYPE_STRING => Kv::Str(read_cstring_utf8(r)?),
            TYPE_INT32 => {
                let mut b = [0u8; 4];
                r.read_exact(&mut b).map_err(|e| e.to_string())?;
                Kv::Int32(i32::from_le_bytes(b))
            }
            TYPE_FLOAT32 => {
                let mut b = [0u8; 4];
                r.read_exact(&mut b).map_err(|e| e.to_string())?;
                Kv::Float32(f32::from_le_bytes(b))
            }
            TYPE_POINTER | TYPE_COLOR => {
                let mut b = [0u8; 4];
                r.read_exact(&mut b).map_err(|e| e.to_string())?;
                Kv::UInt32(u32::from_le_bytes(b))
            }
            TYPE_UINT64 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b).map_err(|e| e.to_string())?;
                Kv::UInt64(u64::from_le_bytes(b))
            }
            TYPE_WSTRING => return Err("wstring no soportado en el schema binario".to_string()),
            other => return Err(format!("tipo VDF binario desconocido: {}", other)),
        };
        out.push((name, value));
    }
    Ok(out)
}

pub fn load(path: &Path) -> Result<Kv, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    Ok(Kv::Nested(read_children(&mut file, 0)?))
}
