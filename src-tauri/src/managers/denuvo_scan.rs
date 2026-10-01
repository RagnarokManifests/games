// Static Denuvo detector — ports OpenSteamTool's own ProtectionScan (the
// DenuvoAuth feature's binary scanner, github.com/OpenSteam001/OpenSteamTool,
// src/Pipe/Features/DenuvoAuth/ProtectionScan.cpp) so Ragnarok can flag
// Denuvo compatibility straight from an installed game's .exe on disk,
// without needing the game running (OpenSteamTool's own version only scans
// live process modules). Same two heuristics, same byte patterns:
//   - OepPattern: the entry point's section contains Denuvo's "DODENUVO"
//     64-bit immediate load (`mov rcx, "DODENUVO"` = 48 B9 44 4F 44 45 4E 55
//     56 4F).
//   - LegacySectionString: a section named one of Denuvo's known section
//     names is present, and the literal ASCII string "DENUVO" appears
//     anywhere in the file.
//
// Read-only, single-shot — not meant to run on a poll loop.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MIN_PACKED_MODULE_BYTES: u64 = 80 * 1024 * 1024;
const DENUVO_STRING: &[u8] = b"DENUVO";
const DENUVO_OEP_PATTERN: &[u8] = &[0x48, 0xB9, 0x44, 0x4F, 0x44, 0x45, 0x4E, 0x55, 0x56, 0x4F];
const LEGACY_DENUVO_SECTIONS: &[&str] = &[".arch", ".srdata", ".xpdata", ".xdata", ".xtls"];
const SCAN_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

struct Section {
    name: String,
    virtual_address: u32,
    raw_offset: u32,
    raw_size: u32,
}

pub struct DenuvoScanResult {
    pub detected: bool,
    pub method: Option<&'static str>,
    pub section_name: Option<String>,
}

fn read_pe_sections(file: &mut File) -> Option<(u32, Vec<Section>)> {
    let mut dos_header = [0u8; 64];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut dos_header).ok()?;
    if &dos_header[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(dos_header[60..64].try_into().ok()?);

    file.seek(SeekFrom::Start(e_lfanew as u64)).ok()?;
    let mut pe_sig = [0u8; 4];
    file.read_exact(&mut pe_sig).ok()?;
    if &pe_sig != b"PE\0\0" {
        return None;
    }

    // IMAGE_FILE_HEADER (20 bytes): NumberOfSections @2, SizeOfOptionalHeader @16.
    let mut coff = [0u8; 20];
    file.read_exact(&mut coff).ok()?;
    let num_sections = u16::from_le_bytes(coff[2..4].try_into().ok()?);
    let size_of_optional_header = u16::from_le_bytes(coff[16..18].try_into().ok()?);
    if size_of_optional_header < 20 {
        return None;
    }

    // AddressOfEntryPoint sits at offset 16 in the optional header for both
    // PE32 and PE32+ (the fields before it are identically sized).
    let opt_header_start = e_lfanew as u64 + 4 + 20;
    file.seek(SeekFrom::Start(opt_header_start)).ok()?;
    let mut opt_header = vec![0u8; size_of_optional_header as usize];
    file.read_exact(&mut opt_header).ok()?;
    let entry_point_rva = u32::from_le_bytes(opt_header[16..20].try_into().ok()?);

    let section_table_start = opt_header_start + size_of_optional_header as u64;
    file.seek(SeekFrom::Start(section_table_start)).ok()?;

    let mut sections = Vec::with_capacity(num_sections as usize);
    for _ in 0..num_sections {
        let mut entry = [0u8; 40];
        if file.read_exact(&mut entry).is_err() {
            break;
        }
        let name_end = entry[0..8].iter().position(|&b| b == 0).unwrap_or(8);
        let name = String::from_utf8_lossy(&entry[0..name_end]).to_string();
        let virtual_address = u32::from_le_bytes(entry[12..16].try_into().ok()?);
        let raw_size = u32::from_le_bytes(entry[16..20].try_into().ok()?);
        let raw_offset = u32::from_le_bytes(entry[20..24].try_into().ok()?);
        sections.push(Section { name, virtual_address, raw_offset, raw_size });
    }

    Some((entry_point_rva, sections))
}

/// Chunked byte-pattern search over `[offset, offset+len)` of `file`, with
/// enough overlap between chunks that a match straddling a chunk boundary
/// isn't missed.
fn find_bytes_in_range(file: &mut File, offset: u64, len: u64, pattern: &[u8]) -> bool {
    if pattern.is_empty() || len == 0 {
        return false;
    }
    let overlap = (pattern.len() as u64).saturating_sub(1);
    let end = offset + len;
    let mut pos = offset;
    let mut buf = vec![0u8; (SCAN_CHUNK_BYTES + overlap) as usize];

    while pos < end {
        let want = std::cmp::min(SCAN_CHUNK_BYTES + overlap, end - pos) as usize;
        if file.seek(SeekFrom::Start(pos)).is_err() {
            return false;
        }
        let read = match file.read(&mut buf[..want]) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if read == 0 {
            break;
        }
        if buf[..read].windows(pattern.len()).any(|w| w == pattern) {
            return true;
        }
        pos += SCAN_CHUNK_BYTES;
    }
    false
}

/// Scans a single executable on disk for Denuvo, mirroring
/// ProtectionScan.cpp's `DetectModule` (OEP pattern first, legacy section
/// string as fallback). Files under 80MB are skipped outright — packed
/// Denuvo binaries are always well above that.
pub fn scan_file_for_denuvo(path: &Path) -> DenuvoScanResult {
    let not_detected = DenuvoScanResult { detected: false, method: None, section_name: None };

    let Ok(metadata) = std::fs::metadata(path) else {
        return not_detected;
    };
    if metadata.len() < MIN_PACKED_MODULE_BYTES {
        return not_detected;
    }

    let Ok(mut file) = File::open(path) else {
        return not_detected;
    };
    let Some((entry_point_rva, sections)) = read_pe_sections(&mut file) else {
        return not_detected;
    };

    if entry_point_rva != 0 {
        if let Some(section) = sections.iter().find(|s| {
            entry_point_rva >= s.virtual_address
                && entry_point_rva < s.virtual_address.saturating_add(s.raw_size.max(1))
        }) {
            if section.raw_size > 0
                && find_bytes_in_range(
                    &mut file,
                    section.raw_offset as u64,
                    section.raw_size as u64,
                    DENUVO_OEP_PATTERN,
                )
            {
                return DenuvoScanResult {
                    detected: true,
                    method: Some("OepPattern"),
                    section_name: Some(section.name.clone()),
                };
            }
        }
    }

    if let Some(section) = sections.iter().find(|s| LEGACY_DENUVO_SECTIONS.contains(&s.name.as_str())) {
        if find_bytes_in_range(&mut file, 0, metadata.len(), DENUVO_STRING) {
            return DenuvoScanResult {
                detected: true,
                method: Some("LegacySectionString"),
                section_name: Some(section.name.clone()),
            };
        }
    }

    not_detected
}

/// Best-effort verdict on whether this Denuvo game is likely to work with
/// Ragnarok's ownership/achievement spoofing, inferred from OpenSteamTool's
/// own ipc.log. Grounded in what we confirmed by hand while debugging Planet
/// Zoo (appid 703080) in this same session:
///   - Games that call `GetAppOwnershipTicketExtendedData` go through the
///     legacy ticket path, which has a working forge fallback — likely OK.
///   - Games that only ever hit `RequestEncryptedAppTicket`/
///     `GetEncryptedAppTicket` and log "no cached eticket, skip" use the
///     encrypted ticket path, which has no forge mechanism at all and
///     requires real Steam-issued data this account won't have.
/// Which API a game calls can only be observed once it's actually been
/// launched through Ragnarok, so this returns "unknown" until then.
pub fn infer_ticket_compat_from_logs(steam_path: &str, app_id: &str) -> String {
    let log_path = Path::new(steam_path).join("opensteamtool").join("ipc.log");
    let Ok(content) = std::fs::read_to_string(&log_path) else {
        return "unknown".to_string();
    };

    let appid_marker = format!("AppId={}", app_id);
    let mut saw_legacy = false;
    let mut saw_encrypted_fail = false;

    for line in content.lines() {
        if !line.contains(&appid_marker) {
            continue;
        }
        if line.contains("GetAppOwnershipTicketExtendedData:") {
            saw_legacy = true;
        }
        if line.contains("no cached eticket, skip") {
            saw_encrypted_fail = true;
        }
    }

    if saw_legacy {
        "legacy_ok".to_string()
    } else if saw_encrypted_fail {
        "encrypted_unsupported".to_string()
    } else {
        "unknown".to_string()
    }
}
