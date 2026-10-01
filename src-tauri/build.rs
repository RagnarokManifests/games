use std::path::Path;
use std::process::Command;

/// Builds `ach_helper` and copies it into `resources/` before Tauri bundles.
///
/// It used to be a hand-copied artifact, and it drifted: the shipped
/// `resources/ach_helper_x64.exe` was six weeks older than its own source, so
/// a round of fixes to how the helper reports Steam's `m_eResult` never
/// reached a single user. Nothing failed — the old binary just kept behaving
/// like the old binary, and anyone reading the diff would have concluded the
/// bug was already fixed.
///
/// The helper is a separate crate on purpose (it loads `steamclient64.dll`
/// and drives it through raw vtable slots, which has no business living in
/// the launcher's own process), so it cannot simply be a workspace member of
/// a Tauri app. Driving `cargo build` from here is the smallest thing that
/// makes "the source changed" and "the shipped binary changed" the same
/// event.
///
/// Deliberately best-effort: a failure here prints a loud warning rather than
/// breaking the whole build, because a launcher that builds without a fresh
/// helper is still useful, and a build that cannot run at all is not.
fn build_ach_helper() {
    let helper_dir = Path::new("ach_helper");
    if !helper_dir.join("Cargo.toml").is_file() {
        println!("cargo:warning=ach_helper/ no encontrado; se conserva el .exe existente en resources/.");
        return;
    }

    // Rerun when the helper's own sources change — without these the whole
    // step would be skipped on an incremental build, which is precisely how
    // the stale binary survived so long.
    println!("cargo:rerun-if-changed=ach_helper/src/main.rs");
    println!("cargo:rerun-if-changed=ach_helper/Cargo.toml");

    let status = Command::new(env!("CARGO"))
        .args(["build", "--release"])
        .current_dir(helper_dir)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            println!("cargo:warning=ach_helper falló al compilar ({s}); se conserva el .exe anterior.");
            return;
        }
        Err(e) => {
            println!("cargo:warning=No se pudo lanzar cargo para ach_helper ({e}); se conserva el .exe anterior.");
            return;
        }
    }

    let built = helper_dir
        .join("target")
        .join("release")
        .join(if cfg!(windows) { "ach_helper.exe" } else { "ach_helper" });
    let dest = Path::new("resources")
        .join(if cfg!(windows) { "ach_helper_x64.exe" } else { "ach_helper_x64" });

    if let Err(e) = std::fs::create_dir_all("resources") {
        println!("cargo:warning=No se pudo crear resources/ ({e}).");
        return;
    }
    match std::fs::copy(&built, &dest) {
        Ok(_) => println!("cargo:warning=ach_helper actualizado en {}.", dest.display()),
        Err(e) => println!(
            "cargo:warning=No se pudo copiar {} a {} ({e}); se conserva el .exe anterior.",
            built.display(),
            dest.display()
        ),
    }
}

fn main() {
    build_ach_helper();
    tauri_build::build()
}
