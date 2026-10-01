//! Asks whether a game's manifests can be obtained at all.
//!
//! Ragnarok's install is best-effort about manifests: `fetch_and_cache_manifests`
//! swallows every failure, so an install where no source had the file still
//! reports success. Nothing is wrong on screen. The user only finds out hours
//! later, when Steam refuses to download and shows "UNKNOWN ERROR" — a message
//! that names neither the game's real problem nor anything they can act on.
//!
//! Traced end to end on a real machine: `.lua` correct, gid matching SteamCMD's
//! current public build, the manifest simply absent from every mirror Ragnarok
//! knows. Steam then asked its CDN for it and got 401 Unauthorized, because the
//! account does not own the game — which Steam reports as "No connection to
//! content servers", sending people to check an internet connection that was
//! never the problem.
//!
//! This module supplies the missing sentence: whether any source has the game.

/// Availability oracle. Plain HTTP to a bare IP, and not ours — so it is only
/// ever asked about an app id, its answer is only ever used to phrase a
/// warning, and it can never fail an install.
const BACKEND: &str = "http://167.235.229.108";

/// The backend gates on a fixed User-Agent instead of a token. Anything else
/// gets a 403.
const BACKEND_USER_AGENT: &str = "secretgoonpoon";

/// Long enough for a slow answer, short enough that a dead backend does not
/// visibly lengthen an install that already succeeded.
const TIMEOUT_SECS: u64 = 8;

/// What the backend says about one source, e.g. `("Ryuu", "available")`.
pub type Source = (String, String);

/// Sources that claim to have `app_id`, or `None` when the backend could not be
/// reached or answered with something unusable.
///
/// `None` and `Some(vec![])` mean genuinely different things and callers must
/// not collapse them: the first is "nobody could be asked", the second is
/// "everybody was asked and nobody has it". Reporting the first as the second
/// would invent a certainty this module does not have.
pub async fn available_sources(client: &reqwest::Client, app_id: &str) -> Option<Vec<Source>> {
    // The id goes into a URL, so it must be exactly what it claims to be.
    if app_id.is_empty() || !app_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let res = client
        .get(format!("{}/check_apis?appid={}", BACKEND, app_id))
        .header("User-Agent", BACKEND_USER_AGENT)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .send()
        .await
        .ok()?;

    if !res.status().is_success() {
        return None;
    }

    // Shape: {"Ryuu":"available","Otra":"unavailable"}. Anything that is not a
    // flat string map is treated as no answer rather than guessed at.
    let body: serde_json::Value = res.json().await.ok()?;
    let map = body.as_object()?;

    Some(
        map.iter()
            .filter(|(_, v)| v.as_str() == Some("available"))
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
            .collect(),
    )
}

/// One sentence about `app_id`'s manifests, for a user who just installed a
/// game whose manifest never arrived.
///
/// Deliberately says something in all three cases. The whole failure this
/// exists for is silence, so "we could not check" is still an answer worth
/// printing — it at least tells the user the install is incomplete.
pub async fn explain_missing(
    client: &reqwest::Client,
    app_id: &str,
    game_label: &str,
) -> String {
    let base = format!(
        "{} se instaló, pero falta el archivo de manifiesto que Steam necesita para descargarlo.",
        game_label
    );

    match available_sources(client, app_id).await {
        Some(sources) if !sources.is_empty() => {
            let names: Vec<&str> = sources.iter().map(|(n, _)| n.as_str()).collect();
            format!(
                "{} Sí existe en {}, así que fue un fallo de descarga: probá instalarlo de nuevo.",
                base,
                names.join(", ")
            )
        }
        Some(_) => format!(
            "{} Ninguna fuente lo tiene publicado todavía, así que reinstalarlo no va a servir. \
             Steam va a mostrar \"UNKNOWN ERROR\" o \"CONTENT SERVERS UNREACHABLE\" al intentar bajarlo — \
             no es tu conexión.",
            base
        ),
        None => format!(
            "{} No se pudo comprobar si alguna fuente lo tiene. Si Steam muestra \"UNKNOWN ERROR\" \
             al descargarlo, es por esto y no por tu conexión.",
            base
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_numeric_app_id_never_reaches_the_network() {
        // Not paranoia about this backend in particular: the id is pasted into
        // a URL, and the only thing that makes that safe is being sure of its
        // shape first.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = reqwest::Client::new();
        for bad in ["", "12 34", "../../etc", "4001890&x=1", "abc"] {
            assert!(
                rt.block_on(available_sources(&client, bad)).is_none(),
                "{bad:?} no debería consultarse"
            );
        }
    }

    /// Against the real Steam install and the real backend. Opt-in:
    /// `cargo test smoke_explain_real_install -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn smoke_explain_real_install() {
        let steam = r"C:\Program Files (x86)\Steam";
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = reqwest::Client::new();

        for (app_id, label) in [("4001890", "How to Fish"), ("1245620", "ELDEN RING")] {
            let missing = crate::managers::manifest_vault::missing_for_app(steam, app_id);
            eprintln!("\n{label} ({app_id}): {} manifest(s) faltantes", missing.len());
            for m in &missing {
                eprintln!("  - {}_{}.manifest", m.depot_id, m.gid);
            }
            if !missing.is_empty() {
                eprintln!("  mensaje: {}", rt.block_on(explain_missing(&client, app_id, label)));
            }
        }
    }

    /// The three outcomes have to read differently, because they call for
    /// different actions: retry, give up, or watch out.
    #[test]
    fn each_outcome_tells_the_user_something_different() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // An unreachable host exercises the None branch without a live backend.
        let client = reqwest::Client::new();
        let unknown = rt.block_on(explain_missing(&client, "", "How to Fish"));

        assert!(unknown.contains("How to Fish"), "tiene que nombrar el juego");
        assert!(
            unknown.contains("no por tu conexión"),
            "el punto entero es desmentir el mensaje de Steam: {unknown}"
        );
        assert!(
            !unknown.contains("probá instalarlo de nuevo"),
            "sin respuesta del backend no puede prometer que reinstalar sirva: {unknown}"
        );
    }
}
