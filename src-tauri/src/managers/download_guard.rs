//! Integrity gate for the third-party binaries this app downloads and then
//! runs, or loads into another process.
//!
//! Ragnarok pulls about a dozen executables and archives from other people's
//! servers — SteamCMD from Valve's CDN, DepotDownloader and gbe_fork from
//! GitHub releases, CreamAPI's DLLs and the plugin zip from raw.github,
//! ProdKeys from its own host. Until now every one of them was written to
//! disk and executed on the strength of "the request returned 200", which is
//! also what a captive portal, a corporate proxy, a "file not found" page and
//! a half-finished transfer all return.
//!
//! What this module can and cannot do is worth stating plainly. Most of these
//! URLs deliberately track upstream (`/releases/latest`, a branch tip), so
//! their contents change legitimately and a pinned hash would break the
//! feature on the next upstream release. For those, `verify_download` checks
//! the transport and the shape of what arrived: HTTPS, an expected host, and
//! bytes that really are the kind of file we asked for. That closes the
//! failure modes users actually hit — an error page saved as
//! `DepotDownloader.exe`, a truncated archive, a redirect landing elsewhere —
//! and it is not a substitute for a signature. Where a URL names an exact
//! version, `expected_sha256` pins it properly — and where upstream publishes
//! a checksum next to the file, fetching that and passing it here is stronger
//! still than anything this module can infer on its own.

/// The kind of file a download is supposed to be, checked against the leading
/// bytes rather than the filename or the server's Content-Type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Payload {
    Zip,
    SevenZ,
    /// A Windows executable or DLL.
    Pe,
}

impl Payload {
    fn magic(self) -> &'static [u8] {
        match self {
            // "PK" 03 04. An empty archive starts "PK" 05 06, which is never
            // what we want here.
            Payload::Zip => &[0x50, 0x4B, 0x03, 0x04],
            Payload::SevenZ => &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
            Payload::Pe => &[0x4D, 0x5A],
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Payload::Zip => "un archivo ZIP",
            Payload::SevenZ => "un archivo 7z",
            Payload::Pe => "un ejecutable de Windows",
        }
    }
}

/// Anything smaller than this is a status page or a stub, never one of the
/// binaries this guards (the smallest, a CreamAPI DLL, is over 100 KB).
const MIN_PLAUSIBLE_BYTES: usize = 1024;

/// True when the URL is HTTPS and its host is one of `allowed_hosts` — an
/// exact match, or a subdomain of an entry written with a leading dot.
///
/// Checked before the request goes out, so a constant that gets edited to
/// point somewhere else — or a URL assembled from a value that came from
/// outside — fails here rather than at the point where its contents run.
pub fn is_allowed_url(url: &str, allowed_hosts: &[&str]) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else { return false };
    let host = host.to_ascii_lowercase();
    allowed_hosts.iter().any(|allowed| {
        let allowed = allowed.to_ascii_lowercase();
        match allowed.strip_prefix('.') {
            Some(bare) => host == bare || host.ends_with(&allowed),
            None => host == allowed,
        }
    })
}

/// Checks a downloaded blob before it is written anywhere it could be
/// executed. `label` names the component in the message the user sees.
pub fn verify_download(
    label: &str,
    bytes: &[u8],
    kind: Payload,
    expected_sha256: Option<&str>,
) -> Result<(), String> {
    if bytes.len() < MIN_PLAUSIBLE_BYTES {
        return Err(format!(
            "La descarga de {} llegó incompleta ({} bytes). No se instaló nada.",
            label,
            bytes.len()
        ));
    }

    // Named separately from the magic-byte check below because it is by far
    // the most common real cause — a captive portal, a corporate proxy or a
    // "file not found" page answered with HTTP 200 — and saying it is a web
    // page tells the user what to do about it.
    let head = &bytes[..bytes.len().min(512)];
    let head_lower = String::from_utf8_lossy(head).to_ascii_lowercase();
    let head_trimmed = head_lower.trim_start();
    if head_trimmed.starts_with("<!doctype")
        || head_trimmed.starts_with("<html")
        || head_trimmed.starts_with("<?xml")
    {
        return Err(format!(
            "El servidor devolvió una página web en lugar de {} para {}. Puede ser un portal de red o un bloqueo del proveedor.",
            kind.describe(),
            label
        ));
    }

    if !bytes.starts_with(kind.magic()) {
        return Err(format!(
            "Lo que se descargó para {} no es {}. No se instaló nada.",
            label,
            kind.describe()
        ));
    }

    if let Some(expected) = expected_sha256 {
        let actual = sha256_hex(bytes);
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            return Err(format!(
                "{} no coincide con la versión esperada (SHA-256 {} en lugar de {}). No se instaló nada.",
                label, actual, expected
            ));
        }
    }

    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(prefix: &[u8]) -> Vec<u8> {
        let mut v = prefix.to_vec();
        v.resize(MIN_PLAUSIBLE_BYTES + 16, 0);
        v
    }

    const ZIP: &[u8] = &[0x50, 0x4B, 0x03, 0x04];
    const SEVENZ: &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
    const PE: &[u8] = &[0x4D, 0x5A];

    #[test]
    fn accepts_the_right_shape() {
        assert!(verify_download("x", &pad(ZIP), Payload::Zip, None).is_ok());
        assert!(verify_download("x", &pad(PE), Payload::Pe, None).is_ok());
        assert!(verify_download("x", &pad(SEVENZ), Payload::SevenZ, None).is_ok());
    }

    #[test]
    fn rejects_an_error_page() {
        let page = pad(b"<!DOCTYPE html><html><body>404 Not Found</body></html>");
        let err = verify_download("SteamCMD", &page, Payload::Zip, None).unwrap_err();
        assert!(err.contains("página web"), "{}", err);
    }

    #[test]
    fn rejects_a_truncated_download() {
        let err = verify_download("SteamCMD", ZIP, Payload::Zip, None).unwrap_err();
        assert!(err.contains("incompleta"), "{}", err);
    }

    #[test]
    fn rejects_the_wrong_kind() {
        // A zip served where an exe was expected — e.g. a redirect that
        // landed on the wrong asset.
        assert!(verify_download("x", &pad(ZIP), Payload::Pe, None).is_err());
        assert!(verify_download("x", &pad(PE), Payload::Zip, None).is_err());
    }

    #[test]
    fn pins_a_hash_when_one_is_known() {
        let bytes = pad(PE);
        let good = sha256_hex(&bytes);
        assert!(verify_download("x", &bytes, Payload::Pe, Some(&good)).is_ok());
        assert!(verify_download("x", &bytes, Payload::Pe, Some(&good.to_uppercase())).is_ok());
        let bad = "0".repeat(64);
        assert!(verify_download("x", &bytes, Payload::Pe, Some(&bad)).is_err());
    }

    #[test]
    fn host_allowlist_is_not_a_substring_match() {
        let hosts = ["github.com", ".githubusercontent.com"];
        assert!(is_allowed_url("https://github.com/a/b.zip", &hosts));
        assert!(is_allowed_url("https://raw.githubusercontent.com/a/b.zip", &hosts));
        assert!(is_allowed_url("https://githubusercontent.com/a/b.zip", &hosts));
        // Downgrades and lookalikes.
        assert!(!is_allowed_url("http://github.com/a/b.zip", &hosts));
        assert!(!is_allowed_url("https://github.com.evil.tld/a.zip", &hosts));
        assert!(!is_allowed_url("https://evil.tld/github.com/a.zip", &hosts));
        assert!(!is_allowed_url("https://github.com@evil.tld/a.zip", &hosts));
        assert!(!is_allowed_url("https://notgithubusercontent.com/a.zip", &hosts));
        assert!(!is_allowed_url("nonsense", &hosts));
    }
}
