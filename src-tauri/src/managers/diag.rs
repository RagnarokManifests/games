//! Where the app's diagnostics actually go.
//!
//! Everything here used to be `eprintln!`. A release build of a Tauri app has
//! no console attached, so every one of those lines was written to a handle
//! nobody was reading — which is the same as not writing it at all. That
//! matters most exactly where it was used: entries dropped from an archive
//! for an unsafe path, files a bypass could not write, a catalog source that
//! timed out. Those are the moments a user asks "why is this not working"
//! and the answer had already been computed and thrown away.
//!
//! `diag_log!` writes the same line to stderr (so `cargo run` and
//! `tauri dev` behave exactly as before) and appends it to a file under the
//! app's own data folder, which the user can open from Ajustes.

use std::io::Write;
use std::path::PathBuf;

/// Rotated at this size so a long-running install loop can't fill a disk.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

pub fn log_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("com.ragnarok.launcher")
            .join("ragnarok.log"),
    )
}

/// Appends one already-formatted line. Called by the `diag_log!` macro; there
/// is no reason to call it directly.
pub fn write_line(msg: &str) {
    // Tests drive the real install and backup paths against throwaway
    // folders, and every line they produced used to land in the user's own
    // ragnarok.log. Fake installs of Wallpaper Engine with 43- and 92-byte
    // tickets sat there beside the real ones, and were mistaken for them while
    // diagnosing a live problem. The console still gets the line.
    if cfg!(test) {
        return;
    }
    let Some(path) = log_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // One previous file is kept: the interesting line is often the one just
    // before things went wrong, and truncating in place would lose it.
    if std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }

    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
}

/// Records a diagnostic line. Same formatting arguments as `println!`.
///
/// Deliberately writes to both places rather than replacing stderr: during
/// development the console output is what people actually watch, and the file
/// is what exists in the build users run.
#[macro_export]
macro_rules! diag_log {
    ($($arg:tt)*) => {{
        let __msg = format!($($arg)*);
        eprintln!("{}", __msg);
        $crate::managers::diag::write_line(&__msg);
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_sits_under_the_app_data_folder() {
        // dirs::data_dir() is always present on the platforms this ships to;
        // the assertion is about where the file lands, not whether it exists.
        let path = log_path().expect("data dir");
        assert!(path.ends_with("com.ragnarok.launcher/ragnarok.log")
            || path.ends_with(r"com.ragnarok.launcher\ragnarok.log"));
    }
}
