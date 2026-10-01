// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod managers;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use once_cell::sync::Lazy;

use tauri::{command, Manager, SystemTray, SystemTrayEvent, WindowEvent};
use managers::download_guard;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use crate::managers::steam::RustSteamManager;
use crate::managers::game::RustGameManager;
use crate::managers::activation::ActivationManager;
use crate::managers::manifest::ManifestManager;
use crate::managers::cloud_saves::{self, BackupInfo as CloudBackupInfo, SaveConflict};
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

// Holds the Discord IPC client across commands (None = not connected)
struct DiscordState(Mutex<Option<DiscordIpcClient>>);

const DISCORD_CLIENT_ID: &str = "1526476344727441449";

/// Discord REST API v10 base URL — all support calls go here directly.
/// The bot token is injected per-request in `make_discord_bot_client`.
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";

/// Manifest mirrors, tried in order until one actually has the file.
///
/// This used to be a single URL pointing at `nicklvsa/steam-manifests`. That
/// repository has since been deleted — GitHub answers 404 for the repo itself,
/// not just for individual files — so every manifest fetch had been failing.
/// Both call sites swallow fetch errors, so nothing ever said so; the only
/// visible symptom was that installs never got a manifest and Versiones
/// Estáticas appeared to do nothing. A list means one mirror going away costs
/// coverage instead of costing everything.
const MANIFEST_MIRRORS: &[&str] = &[
    "https://raw.githubusercontent.com/qwe213312/k25FCdfEOoEJ42S6/main",
    // `nicklvsa/steam-manifests` used to be the second entry. GitHub answers
    // 404 for the repository itself, so it was costing a wasted request per
    // manifest while contributing no coverage at all — and, worse, making the
    // list look like a two-mirror fallback when it has been a single point of
    // failure for a while. Removed rather than left as decoration.
];

/// True if these bytes really are a Steam depot manifest.
///
/// A deleted or rate-limited mirror answers with an HTML error page, and some
/// answer 200 while doing it. Writing that body into depotcache under a
/// `.manifest` name makes Steam fail to parse it later, which surfaces as a
/// broken install pointing nowhere near the download that caused it.
fn looks_like_manifest(bytes: &[u8]) -> bool {
    // 0x71F617D0 (little-endian on disk) is Steam's protobuf payload magic;
    // some mirrors keep the same payload zipped.
    matches!(
        bytes.get(..4),
        Some([0xD0, 0x17, 0xF6, 0x71]) | Some([b'P', b'K', 0x03, 0x04])
    )
}

/// Downloads `<depot>_<manifest>.manifest` from the first mirror that has it.
///
/// A miss here is not a small thing, so it is recorded. Without the manifest
/// on disk Steam has to ask the CDN for it, and the CDN answers 401 for an app
/// the account does not own — which Steam then reports as "UNKNOWN ERROR" or
/// "No connection to content servers", hours later, with nothing tying it back
/// to the install that quietly came up short. Confirmed on a real machine with
/// "How to Fish": depot 4001891 pins a gid the surviving mirror simply does
/// not carry, so every reinstall failed the same way with no explanation.
async fn fetch_manifest_from_mirrors(client: &reqwest::Client, filename: &str) -> Option<Vec<u8>> {
    for base in MANIFEST_MIRRORS {
        let url = format!("{}/{}", base, filename);
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        if !(resp.status().is_success() || resp.status().as_u16() == 206) {
            continue;
        }
        let Ok(bytes) = resp.bytes().await else {
            continue;
        };
        if looks_like_manifest(&bytes) {
            return Some(bytes.to_vec());
        }
    }
    // Informative, not a verdict.
    //
    // This used to announce Steam's 401 and its "UNKNOWN ERROR" as though they
    // were already happening, which read as a failed install every time it
    // appeared. It is not: the mirrors are only the FIRST of two sources, and
    // the Ryuu ticket downloaded moments later carries the same manifest and
    // usually supplies it. Traced on a real machine — this line printed twice
    // for How to Fish, the ticket wrote the file anyway, and Steam downloaded
    // the game twelve seconds later.
    //
    // Whether a manifest is genuinely missing is settled after the install by
    // looking at the disk (`manifest_vault::missing_for_app`), which is the
    // only check that can actually know. This one just records which source
    // did not have it.
    diag_log!(
        "[manifest] Los mirrors no tienen {} — se intentará con el ticket de Ryuu.",
        filename
    );
    None
}

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_API: &str = "https://api.github.com/repos/RagnarokManifests/games/releases/latest";

/// Hosts GitHub serves this project's own assets from — the API, the raw
/// blobs, the release files, and the CDN releases redirect to. Used to check
/// a URL before anything is downloaded from it and run.
const GITHUB_HOSTS: &[&str] = &["github.com", "api.github.com", ".githubusercontent.com"];

#[derive(Serialize, Deserialize)]
struct UpdateInfo {
    has_update: bool,
    current_version: String,
    latest_version: String,
    download_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SteamMedia {
    screenshots: Vec<String>,
    /// Same shots at 600x338 for the grid. Parallel to `screenshots`, so index
    /// `i` in one is index `i` in the other.
    #[serde(default)]
    screenshots_thumbs: Vec<String>,
    trailer_mp4: Option<String>,
    trailer_thumbnail: Option<String>,
    trailer_webm: Option<String>,
    name: Option<String>,
    metacritic_score: Option<i32>,
    minimum_reqs: Option<String>,
    recommended_reqs: Option<String>,
    short_description: Option<String>,
    /// The trailer as Steam serves it now: an HLS stream. Steam stopped
    /// listing plain MP4/WebM files, and the addresses guessed for them in
    /// their place answer 404, which is why every game page fell through to a
    /// YouTube search.
    #[serde(default)]
    trailer_hls: Option<String>,
    #[serde(default)]
    developers: Vec<String>,
    #[serde(default)]
    publishers: Vec<String>,
    /// As Steam writes it ("Oct 27, 2016").
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    languages: Vec<SupportedLanguage>,
    #[serde(default)]
    reviews: Option<ReviewSummary>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct SupportedLanguage {
    name: String,
    /// Voice acting, not just text.
    audio: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct ReviewSummary {
    /// Steam's own wording, in English ("Very Positive").
    description: String,
    positive: u64,
    total: u64,
}

/// Steam's `supported_languages`: a comma list where an asterisk marks full
/// audio, followed by a footnote after `<br>` that is not a language —
/// `English<strong>*</strong>, Traditional Chinese<br><strong>*</strong>languages with full audio support`.
fn parse_supported_languages(html: &str) -> Vec<SupportedLanguage> {
    let list = html.split("<br").next().unwrap_or("");
    list.split(',')
        .filter_map(|item| {
            let audio = item.contains('*');
            let mut name = String::new();
            let mut in_tag = false;
            for c in item.chars() {
                match c {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    '*' => {}
                    c if !in_tag => name.push(c),
                    _ => {}
                }
            }
            let name = name.trim().to_string();
            (!name.is_empty()).then_some(SupportedLanguage { name, audio })
        })
        .collect()
}

#[cfg(test)]
mod steam_details_tests {
    use super::{parse_supported_languages, SupportedLanguage};

    #[test]
    fn languages_and_voice_acting_are_read_from_steams_list() {
        // Skyrim Special Edition's real value.
        let skyrim = "English<strong>*</strong>, French<strong>*</strong>, Spanish - Spain<strong>*</strong>, Traditional Chinese, Russian<strong>*</strong><br><strong>*</strong>languages with full audio support";
        let langs = parse_supported_languages(skyrim);
        assert_eq!(langs.len(), 5, "la nota al pie no es un idioma: {langs:?}");
        assert_eq!(langs[2], SupportedLanguage { name: "Spanish - Spain".into(), audio: true });
        assert_eq!(langs[3], SupportedLanguage { name: "Traditional Chinese".into(), audio: false });
        assert!(parse_supported_languages("").is_empty());
    }
}

fn extract_timestamp(url: &str) -> Option<&str> {
    url.find("?t=").map(|i| &url[i + 3..])
}

/// The one HTTP client this process uses.
///
/// This function used to build a brand-new `reqwest::Client` on every call, and
/// every Client carries its own connection pool — so nothing was ever reused.
/// Painting a grid of thirty covers meant thirty DNS lookups and thirty TLS
/// handshakes against the same CDN host, in parallel, competing for the same
/// link. On satellite at 600-800 ms round trip that is seconds spent before the
/// first byte of the first image, to move exactly the same payload.
///
/// A `Client` is a handle around a shared pool, so cloning it is cheap and the
/// signature stays as it was for the ~40 call sites.
static HTTP_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn make_client() -> Result<reqwest::Client, String> {
    if let Some(existing) = HTTP_CLIENT.get() {
        return Ok(existing.clone());
    }
    let built = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        // Keep connections warm between images: a catalog grid asks for its
        // covers in bursts, and re-handshaking each time is the whole cost.
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        // A cap on establishing the connection only — never on the transfer.
        //
        // Without it a host that simply never answers left the OS to give up on
        // its own schedule (~21 s per SYN retry sequence, longer with several
        // candidate hosts in a row), which is where "the tab spins forever"
        // came from. This is safe for the multi-GB downloads that share this
        // client precisely because it does not bound the body — that mistake
        // is what made a slow satellite link fail on payload size earlier.
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    // No global timeout on purpose — callers set their own per request, and
    // several here legitimately run for hours (bypass downloads).
    Ok(HTTP_CLIENT.get_or_init(|| built).clone())
}

/// Bot token for the Discord REST API.
/// In open-source builds, set via DISCORD_BOT_TOKEN environment variable.
const DISCORD_BOT_TOKEN: &str = "";

fn get_discord_bot_token() -> String {
    std::env::var("DISCORD_BOT_TOKEN").unwrap_or_else(|_| DISCORD_BOT_TOKEN.to_string())
}

/// Whether the Discord bot is configured.
fn support_relay_is_configured() -> bool {
    !get_discord_bot_token().is_empty()
}

/// HTTP client for Discord's bot REST API.
/// Uses DiscordBot User-Agent and injects the Authorization header.
fn make_discord_bot_client() -> Result<reqwest::Client, String> {
    let token = get_discord_bot_token();
    if token.is_empty() {
        return Err("El token del bot de Discord no está configurado.".to_string());
    }

    let mut headers = reqwest::header::HeaderMap::new();
    let auth_value = format!("Bot {}", token);
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&auth_value)
            .map_err(|e| e.to_string())?,
    );

    reqwest::Client::builder()
        .user_agent("DiscordBot (https://github.com/RagnarokManifests, 1.0)")
        .default_headers(headers)
        .build()
        .map_err(|e| e.to_string())
}

/// The address of the few-second silent clip the Steam store plays when the
/// pointer rests on a game — the same effect for catalog cards.
///
/// Steam no longer lists trailers as MP4, only as DASH and HLS streams, which
/// the webview cannot play by itself. The short preview still sits beside
/// those streams as `microtrailer.mp4` (checked on Victoria 3 and on a 2026
/// trailer), so its address is built from theirs.
fn microtrailer_from_movies(data: &serde_json::Value) -> Option<String> {
    let movies = data.get("movies")?.as_array()?;
    let movie = movies
        .iter()
        .find(|m| m["highlight"].as_bool() == Some(true))
        .or_else(|| movies.first())?;
    for key in ["dash_h264", "hls_h264", "dash_av1"] {
        let Some(url) = movie[key].as_str() else { continue };
        let path = url.split('?').next().unwrap_or(url);
        if let Some((folder, _)) = path.rsplit_once('/') {
            return Some(format!("{}/microtrailer.mp4", folder.replacen("http://", "https://", 1)));
        }
    }
    // A trailer from before the switch still lists plain files.
    movie["mp4"]["480"].as_str().map(|s| s.replacen("http://", "https://", 1))
}

/// Answers per game, kept for the session: a card is hovered many times, and
/// Steam's store API rate-limits anyone who asks too often.
static HOVER_TRAILERS: std::sync::OnceLock<Mutex<std::collections::HashMap<String, Option<String>>>> =
    std::sync::OnceLock::new();

#[command]
async fn get_hover_trailer(app_id: String) -> Result<Option<String>, String> {
    if app_id.is_empty() || !app_id.chars().all(|c| c.is_ascii_digit()) {
        return Ok(None);
    }
    let cache = HOVER_TRAILERS.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&app_id).cloned()) {
        return Ok(hit);
    }

    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={}&filters=movies",
        app_id
    );
    let json: serde_json::Value = make_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let found = microtrailer_from_movies(&json[app_id.as_str()]["data"]);

    // Only a real answer is remembered; a network failure is tried again on
    // the next hover.
    if let Ok(mut c) = cache.lock() {
        c.insert(app_id, found.clone());
    }
    Ok(found)
}

#[cfg(test)]
mod hover_trailer_tests {
    use super::microtrailer_from_movies;
    use serde_json::json;

    /// The shape Steam sends today, trimmed from Victoria 3's real answer.
    #[test]
    fn the_preview_sits_beside_the_stream() {
        let data = json!({ "movies": [{
            "id": 256918897,
            "dash_h264": "https://video.akamai.steamstatic.com/store_trailers/529340/518465/64359b1c/1750544718/dash_h264.mpd?t=1669908749",
            "hls_h264": "https://video.akamai.steamstatic.com/store_trailers/529340/518465/64359b1c/1750544718/hls_264_master.m3u8?t=1669908749",
            "highlight": true
        }]});
        assert_eq!(
            microtrailer_from_movies(&data).as_deref(),
            Some("https://video.akamai.steamstatic.com/store_trailers/529340/518465/64359b1c/1750544718/microtrailer.mp4")
        );
    }

    #[test]
    fn the_highlighted_trailer_wins_and_old_shapes_still_work() {
        let data = json!({ "movies": [
            { "hls_h264": "https://v/a/1/hls.m3u8" },
            { "dash_h264": "https://v/b/2/dash.mpd", "highlight": true }
        ]});
        assert_eq!(microtrailer_from_movies(&data).as_deref(), Some("https://v/b/2/microtrailer.mp4"));

        let old = json!({ "movies": [{ "mp4": { "480": "http://cdn/steam/apps/1/movie480.mp4" } }] });
        assert_eq!(microtrailer_from_movies(&old).as_deref(), Some("https://cdn/steam/apps/1/movie480.mp4"));

        assert_eq!(microtrailer_from_movies(&json!({})), None);
        assert_eq!(microtrailer_from_movies(&json!({ "movies": [] })), None);
    }
}

/// The entry describing `app_id` inside an `appdetails` answer.
///
/// The answer is **not** always keyed by the id that was asked for. Steam
/// redirects some apps to another store item and keys the reply by that one
/// instead: Forza Horizon 6 (2483190) answers under 5250040, Control Resonant
/// (3669870) under 4760190. Every lookup here indexed the reply by the id it
/// had asked about, found nothing, and reported it as "this game is
/// region-locked or unavailable" — which is what left new games with no cover
/// art, no name and an empty DLC list.
///
/// `data.steam_appid` is the app the entry really describes, so that is what
/// decides. The plain key is still tried first for the ordinary case, and a
/// lone successful entry is accepted last: a single-id request has only one
/// game it could be about.
fn appdetails_entry<'a>(json: &'a serde_json::Value, app_id: &str) -> Option<&'a serde_json::Value> {
    let map = json.as_object()?;
    let wanted = app_id.parse::<u64>().ok();
    let succeeded = |v: &serde_json::Value| v["success"].as_bool().unwrap_or(false);

    if let Some(direct) = map.get(app_id).filter(|v| succeeded(v)) {
        return Some(direct);
    }
    if let Some(by_appid) = wanted.and_then(|id| {
        map.values()
            .find(|v| succeeded(v) && v["data"]["steam_appid"].as_u64() == Some(id))
    }) {
        return Some(by_appid);
    }
    let mut successful = map.values().filter(|v| succeeded(v));
    let only = successful.next()?;
    successful.next().is_none().then_some(only)
}

#[cfg(test)]
mod appdetails_entry_tests {
    use super::appdetails_entry;
    use serde_json::json;

    #[test]
    fn finds_the_entry_under_its_own_id() {
        let answer = json!({ "730": { "success": true, "data": { "steam_appid": 730, "name": "CS" } } });
        assert_eq!(appdetails_entry(&answer, "730").unwrap()["data"]["name"], "CS");
    }

    /// The bug this exists for: Forza Horizon 6 asked as 2483190, answered
    /// under 5250040.
    #[test]
    fn finds_an_entry_steam_keyed_by_another_store_id() {
        let answer = json!({ "5250040": { "success": true, "data": { "steam_appid": 2483190, "name": "Forza" } } });
        assert_eq!(appdetails_entry(&answer, "2483190").unwrap()["data"]["name"], "Forza");
    }

    #[test]
    fn a_failed_or_empty_answer_has_no_entry() {
        assert!(appdetails_entry(&json!({ "730": { "success": false } }), "730").is_none());
        assert!(appdetails_entry(&json!({}), "730").is_none());
        assert!(appdetails_entry(&json!("not an object"), "730").is_none());
    }

    /// With several games in one reply, guessing is not allowed: only the one
    /// that names this app id counts.
    #[test]
    fn never_guesses_between_several_games() {
        let answer = json!({
            "730":  { "success": true, "data": { "steam_appid": 730, "name": "CS" } },
            "570":  { "success": true, "data": { "steam_appid": 570, "name": "Dota" } },
        });
        assert!(appdetails_entry(&answer, "2483190").is_none());
        assert_eq!(appdetails_entry(&answer, "570").unwrap()["data"]["name"], "Dota");
    }
}

#[command]
async fn fetch_steam_data(app_id: String, lang: Option<String>) -> Result<SteamMedia, String> {
    let client = make_client()?;

    // Kept in English regardless of `lang` — the game `name` extracted below
    // feeds the catalog's name-resolution cache, and mixing English/Spanish
    // names across sessions would make matching/caching inconsistent. Only
    // the short_description gets a separate, Spanish-specific fetch below.
    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={}&filters=screenshots,movies,basic,metacritic,release_date,developers,publishers&l=english",
        app_id
    );

    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    let text = res.text().await.map_err(|e| e.to_string())?;
    let data: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let app_data = appdetails_entry(&data, &app_id)
        .map(|e| &e["data"])
        .unwrap_or(&serde_json::Value::Null);

    let to_https = |s: &str| s.replace("http://", "https://");

    // Steam hands back both sizes in the same response, and the grid only ever
    // draws them at 220x124. Taking `path_full` for that meant downloading a
    // 1920x1080 JPEG — measured at ~350 KB apiece, 7.7 MB across a game like
    // Cyberpunk's 22 shots — to paint a thumbnail. `path_thumbnail` is 600x338
    // and ~53 KB: still more than the box needs, and an eighth of the bytes.
    //
    // The full-size list is kept for the lightbox, which is the one place the
    // pixels are actually used, and is now only fetched when someone opens it.
    let screenshots: Vec<String> = app_data["screenshots"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s["path_full"].as_str().map(|s| to_https(s)))
                .collect()
        })
        .unwrap_or_default();

    let screenshots_thumbs: Vec<String> = app_data["screenshots"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    // Fall back to the full path when a shot has no thumbnail,
                    // so the grid never ends up with a hole in it.
                    s["path_thumbnail"]
                        .as_str()
                        .or_else(|| s["path_full"].as_str())
                        .map(|s| to_https(s))
                })
                .collect()
        })
        .unwrap_or_default();

    let (trailer_mp4, trailer_webm, trailer_thumbnail) =
        if let Some(movies) = app_data["movies"].as_array() {
            // Prefer highlight trailers over any first movie
            let movie = movies.iter()
                .find(|m| m["highlight"].as_bool().unwrap_or(false))
                .or_else(|| movies.first());

            if let Some(movie) = movie {
                let thumb = movie["thumbnail"].as_str().map(|s| to_https(s));

                // Direct URL fields (most games since 2019)
                let mp4_hd  = movie["mp4"]["max"].as_str().map(|s| to_https(s));
                let mp4_sd  = movie["mp4"]["480"].as_str().map(|s| to_https(s));
                let webm_hd = movie["webm"]["max"].as_str().map(|s| to_https(s));
                let webm_sd = movie["webm"]["480"].as_str().map(|s| to_https(s));

                let mp4  = mp4_hd.or(mp4_sd);
                let webm = webm_hd.or(webm_sd);

                if mp4.is_some() || webm.is_some() {
                    (mp4, webm, thumb)
                } else {
                    // New format: no direct URLs — build from movie ID.
                    // Steam video CDN is video.akamai.steamstatic.com/store_trailers/{movie_id}/
                    let movie_id = movie["id"].as_u64().map(|id| id.to_string());
                    let thumb_str = thumb.as_deref().unwrap_or("");
                    let ts = extract_timestamp(thumb_str);

                    let (hd_mp4, hd_webm) = if let Some(ref mid) = movie_id {
                        let base = format!(
                            "https://video.akamai.steamstatic.com/store_trailers/{}", mid
                        );
                        let (mp4_url, webm_url) = if let Some(t) = ts {
                            (
                                format!("{}/movie_max.mp4?t={}", base, t),
                                format!("{}/movie_max.webm?t={}", base, t),
                            )
                        } else {
                            (
                                format!("{}/movie_max.mp4", base),
                                format!("{}/movie_max.webm", base),
                            )
                        };
                        (Some(mp4_url), Some(webm_url))
                    } else if let Some(dash) = movie["dash_h264"]
                        .as_str()
                        .or_else(|| movie["dash_av1"].as_str())
                    {
                        // Derive base path from DASH manifest URL
                        if let Some(idx) = dash.find("/dash_") {
                            let base = to_https(&dash[..idx]);
                            (
                                Some(format!("{}/movie_max.mp4", base)),
                                Some(format!("{}/movie_max.webm", base)),
                            )
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    };

                    (hd_mp4, hd_webm, thumb)
                }
            } else {
                (None, None, None)
            }
        } else {
            (None, None, None)
        };

    // What Steam actually serves today (see `trailer_hls`). The window plays it
    // through hls.js.
    let trailer_hls = app_data["movies"].as_array().and_then(|movies| {
        let movie = movies
            .iter()
            .find(|m| m["highlight"].as_bool().unwrap_or(false))
            .or_else(|| movies.first())?;
        movie["hls_h264"].as_str().map(|s| to_https(s))
    });

    let str_list = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let developers = str_list(&app_data["developers"]);
    let publishers = str_list(&app_data["publishers"]);
    let release_date = app_data["release_date"]["date"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let languages = app_data["supported_languages"]
        .as_str()
        .map(parse_supported_languages)
        .unwrap_or_default();

    let name = app_data["name"].as_str().map(|s| s.to_string());
    let metacritic_score = app_data["metacritic"]["score"]
        .as_i64()
        .map(|s| s as i32);
    let minimum_reqs = app_data["pc_requirements"]["minimum"].as_str().map(|s| s.to_string());
    let recommended_reqs = app_data["pc_requirements"]["recommended"].as_str().map(|s| s.to_string());
    // Already present in the same appdetails response (via the "basic" filter
    // group) that's fetched for the trailer/screenshots/metacritic above —
    // was just never read out into SteamMedia.
    let mut short_description = app_data["short_description"].as_str().map(|s| s.to_string());

    // When the launcher is set to Spanish, re-fetch just the description in
    // Spanish (small, separate request — filters=basic only) instead of
    // switching the whole request above, which would also flip `name` to
    // Spanish and destabilize the catalog's name-resolution cache.
    if lang.as_deref() == Some("es") {
        let es_url = format!(
            "https://store.steampowered.com/api/appdetails?appids={}&filters=basic&l=spanish",
            app_id
        );
        if let Ok(res) = client.get(&es_url).send().await {
            if let Ok(json) = res.json::<serde_json::Value>().await {
                if let Some(desc) = appdetails_entry(&json, &app_id)
                    .and_then(|e| e["data"]["short_description"].as_str())
                {
                    if !desc.is_empty() {
                        short_description = Some(desc.to_string());
                    }
                }
            }
        }
    }

    // Steam's own review summary. appdetails does not carry it; the reviews
    // API does, and asking it for zero reviews returns just the summary.
    let reviews = async {
        let url = format!(
            "https://store.steampowered.com/appreviews/{}?json=1&language=all&purchase_type=all&num_per_page=0",
            app_id
        );
        let v: serde_json::Value = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let q = &v["query_summary"];
        let total = q["total_reviews"].as_u64()?;
        (total > 0).then(|| ReviewSummary {
            description: q["review_score_desc"].as_str().unwrap_or("").to_string(),
            positive: q["total_positive"].as_u64().unwrap_or(0),
            total,
        })
    }
    .await;

    Ok(SteamMedia {
        screenshots,
        screenshots_thumbs,
        trailer_mp4,
        trailer_webm,
        trailer_thumbnail,
        name,
        metacritic_score,
        minimum_reqs,
        recommended_reqs,
        short_description,
        trailer_hls,
        developers,
        publishers,
        release_date,
        languages,
        reviews,
    })
}

/// Searches YouTube for a game trailer and returns the first video ID found.
#[command]
async fn search_youtube_trailer(game_name: String) -> Result<Option<String>, String> {
    let client = make_client()?;

    // Encode query: spaces → +, keep alphanumerics safe
    let query_raw = format!("{} official trailer", game_name);
    let encoded: String = query_raw.chars().map(|c| {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c.to_string() }
        else if c == ' ' { "+".to_string() }
        else { format!("%{:02X}", c as u8) }
    }).collect();

    // sp=EgIQAQ%3D%3D = "Videos only" filter
    let url = format!(
        "https://www.youtube.com/results?search_query={}&sp=EgIQAQ%3D%3D",
        encoded
    );

    let html = client
        .get(&url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    // YouTube embeds initial data with "videoId":"XXXXXXXXXXX" patterns
    let mut found_ids: Vec<String> = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = html[search_from..].find("\"videoId\":\"") {
        let start = search_from + pos + 11;
        if let Some(end_offset) = html[start..].find('"') {
            let id = &html[start..start + end_offset];
            if id.len() == 11 && !found_ids.contains(&id.to_string()) {
                found_ids.push(id.to_string());
                if found_ids.len() >= 3 { break; }
            }
        }
        search_from = start;
    }

    Ok(found_ids.into_iter().next())
}

/// Lightweight batch name fetch — only calls filters=basic, runs in parallel.
///
/// Also reports whether each game uses Denuvo, reusing the same response
/// (filters=basic already includes drm_notice — confirmed against Steam's
/// real API, not assumed) — this is a single combined call rather than a
/// second request per game just for DRM status.
///
/// The frontend has always expected `[name, has_denuvo]` tuples here — this
/// previously returned bare `HashMap<String, String>` (name only), so on the
/// JS side `resolved[id]` was actually just the name STRING, and code doing
/// `res[0]` / `const [name, hasDenuvo] = res` was indexing into that string
/// by CHARACTER rather than destructuring a tuple: names silently got
/// truncated to their first letter, and the Denuvo badge could never be true
/// (a garbled second character always fails a strict `=== true` check).
/// "Denuvo" cuando el juego lo trae, cadena vacia en cualquier otro caso.
///
/// Deliberadamente solo Denuvo. Steam declara en `drm_notice` cualquier
/// anti-tamper de terceros --Elden Ring Nightreign trae GuardIT, de Arxan--
/// y marcarlos todos hacia que el cartel apareciera en juegos donde no
/// aporta nada. Denuvo es el que decide si un juego se puede usar o no, asi
/// que es el unico que se avisa.
///
/// Los lanzadores externos (Ubisoft Connect, Rockstar) viven en
/// `ext_user_account_notice` y quedan fuera por la misma razon.
fn drm_label(drm_notice: &str) -> String {
    if drm_notice.to_lowercase().contains("denuvo") {
        "Denuvo".to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod drm_label_tests {
    use super::drm_label;

    /// Cadenas exactas que devolvio la API de Steam para estos juegos.
    #[test]
    fn only_denuvo_is_flagged() {
        assert_eq!(
            drm_label("Denuvo Anti-tamper<br>5 different PC within a day machine activation limit"),
            "Denuvo"
        );
        // Elden Ring Nightreign. GuardIT es anti-tamper de Arxan y Steam lo
        // declara igual que Denuvo, pero aca no se avisa a proposito.
        assert_eq!(drm_label("GuardIT"), "");
        assert_eq!(drm_label(""), "");
    }

    #[test]
    fn case_does_not_matter() {
        assert_eq!(drm_label("DENUVO ANTI-TAMPER"), "Denuvo");
    }
}

#[command]
async fn fetch_game_names(app_ids: Vec<String>) -> Result<HashMap<String, (String, String)>, String> {
    use tokio::sync::Semaphore;
    use std::sync::Arc;

    let client = make_client()?;
    // Max 5 concurrent requests to avoid Steam rate limiting
    let sem = Arc::new(Semaphore::new(5));

    let handles: Vec<_> = app_ids.into_iter().map(|id| {
        let c = client.clone();
        let sem = sem.clone();
        tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            let url = format!(
                "https://store.steampowered.com/api/appdetails?appids={}&filters=basic&l=english",
                id
            );
            // Small delay to be gentle on Steam API
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if let Ok(res) = c.get(&url).send().await {
                if let Ok(data) = res.json::<serde_json::Value>().await {
                    if let Some(entry) = appdetails_entry(&data, &id) {
                        if let Some(name) = entry["data"]["name"].as_str() {
                            if !name.trim().is_empty() {
                                let label = drm_label(
                                    entry["data"]["drm_notice"].as_str().unwrap_or(""),
                                );
                                return Some((id, (name.to_string(), label)));
                            }
                        }
                    }
                }
            }
            None
        })
    }).collect();

    let mut map = HashMap::new();
    for r in futures::future::join_all(handles).await.into_iter().flatten() {
        if let Some((id, entry)) = r {
            map.insert(id, entry);
        }
    }
    Ok(map)
}

// ── Manifest pinning (Static Versions) ──────────────────────────────────
// A game's AppID landing here marks it as deliberately version-locked via
// the Static Versions gallery. Used for the "FIJO" badge in the UI.
// Stored separately from ragnarok_apps.json (Steam-facing sidecar) since
// this is purely a Ragnarok-side preference.
fn manifest_exclusions_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("pinned_manifests.json"))
}

fn load_manifest_exclusions() -> HashSet<String> {
    let Some(path) = manifest_exclusions_path() else { return HashSet::new() };
    // Not existing is the normal state on a fresh install: no pins yet.
    let Ok(content) = std::fs::read_to_string(&path) else { return HashSet::new() };

    match serde_json::from_str(&content) {
        Ok(set) => set,
        Err(e) => {
            // Distinguished from "no file" on purpose. Falling back to an
            // empty set here means every pinned game becomes eligible for the
            // background update that the pin existed to prevent, and until
            // now that happened without a trace anywhere.
            diag_log!(
                "[pins] pinned_manifests.json ilegible ({}): los juegos anclados quedan sin protección hasta que se vuelva a anclar alguno.",
                e
            );
            HashSet::new()
        }
    }
}

/// Written atomically, because this one file is the only thing standing
/// between a deliberately version-locked game and the background updater.
///
/// `fs::write` truncates before it writes, so an interruption at the wrong
/// moment leaves a half-written pinned_manifests.json — and
/// `load_manifest_exclusions` treats an unparseable file as "no pins at all",
/// silently unlocking every pinned game at once. Temp file plus rename means
/// the old list survives any failure.
fn save_manifest_exclusions(set: &HashSet<String>) -> Result<(), String> {
    let path = manifest_exclusions_path().ok_or("No se pudo resolver AppData")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(set).map_err(|e| e.to_string())?;

    let tmp = path.with_extension("json.ragnarok-tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.to_string())
        }
    }
}

/// Copies whatever Steam currently holds in `depotcache` into Ragnarok's own
/// vault, so a later Steam uninstall or build change cannot destroy it.
///
/// Returns `(copiados, vistos)`.
#[command]
async fn backup_depot_manifests(steam_path: String) -> Result<(usize, usize), String> {
    Ok(crate::managers::manifest_vault::back_up(&steam_path))
}

/// Checks every ticketed game's .lua against `depotcache` and its ACF.
///
/// With `repair`, restores missing manifests from the vault and re-syncs a
/// drifted ACF. Touches no network and does not need Steam running — but Steam
/// must be CLOSED for a repair to stick, since it rewrites the ACF from memory
/// on exit.
#[command]
async fn check_manifest_integrity(
    steam_path: String,
    repair: Option<bool>,
) -> Result<Vec<crate::managers::manifest_vault::ManifestIssue>, String> {
    Ok(crate::managers::manifest_vault::check(
        &steam_path,
        repair.unwrap_or(false),
    ))
}

#[command]
async fn get_pinned_manifests() -> Result<Vec<String>, String> {
    Ok(load_manifest_exclusions().into_iter().collect())
}

#[command]
async fn set_manifest_pinned(app_id: String, pinned: bool) -> Result<(), String> {
    let mut set = load_manifest_exclusions();
    if pinned {
        set.insert(app_id);
    } else {
        set.remove(&app_id);
    }
    save_manifest_exclusions(&set)
}

/// Scans every Steam library for `appmanifest_<appid>.acf` and pulls out its
/// "buildid" field. Shared by the `get_build_version` command and by
/// `cache_lua_ticket_to_github` (which best-effort tags a cached ticket with
/// whatever build the game happens to be on locally at cache time).
fn read_local_build_id(steam_path: &str, app_id: &str) -> Result<String, String> {
    let library_paths = ManifestManager::get_library_folders(steam_path);
    let library_paths = if library_paths.is_empty() { vec![steam_path.to_string()] } else { library_paths };

    for lib in &library_paths {
        let acf = std::path::PathBuf::from(lib)
            .join("steamapps")
            .join(format!("appmanifest_{}.acf", app_id));
        if acf.exists() {
            let content = std::fs::read_to_string(&acf).map_err(|e| e.to_string())?;
            for line in content.lines() {
                let t = line.trim();
                if t.to_lowercase().starts_with("\"buildid\"") {
                    // Format: "buildid"		"12345678"
                    let parts: Vec<&str> = t.splitn(2, char::is_whitespace).collect();
                    if let Some(val) = parts.get(1) {
                        return Ok(val.trim().trim_matches('"').to_string());
                    }
                }
            }
            return Ok(String::new()); // ACF exists but buildid line missing
        }
    }
    Err(format!("No ACF found for AppID {}", app_id))
}

/// Reads the current "buildid" value from an existing ACF manifest.
/// Returns an empty string if the field is missing or the ACF doesn't exist.
#[command]
#[allow(dead_code)]
async fn get_build_version(steam_path: String, app_id: String) -> Result<String, String> {
    read_local_build_id(&steam_path, &app_id)
}

/// Writes a specific BuildID into the ACF manifest for a game.
/// Also sets AutoUpdateBehavior to "1" (only update on launch) so Steam
/// won't silently overwrite the pinned version on its own.
/// If no ACF exists yet, creates a minimal stub manifest with the given BuildID.
#[command]
#[allow(dead_code)]
async fn lock_build_version(
    steam_path: String,
    app_id: String,
    build_id: String,
    app_name: Option<String>,
    install_dir: Option<String>,
    manifest_id: Option<String>,
) -> Result<(), String> {
    let library_paths = ManifestManager::get_library_folders(&steam_path);
    let library_paths = if library_paths.is_empty() { vec![steam_path.clone()] } else { library_paths };

    let manifest = manifest_id.unwrap_or_default();

    // 1. Create/Update <Steam>/config/lua/<appid>.lua license ticket
    // Include setManifestid so SteamTools redirects to the pinned manifest
    // instead of letting Steam query content servers (which causes CONTENT STILL ENCRYPTED).
    let lua_dir = std::path::PathBuf::from(&steam_path).join("config").join("lua");
    if std::fs::create_dir_all(&lua_dir).is_ok() {
        let lua_file = lua_dir.join(format!("{}.lua", app_id));
        let mut lua_content = format!("-- Generated by LuaTools\naddappid({}, 1)\n", app_id);
        if !manifest.trim().is_empty() {
            // setManifestid tells SteamTools which manifest to use — prevents download attempts
            lua_content.push_str(&format!("setManifestid({}, {})\n", app_id, manifest.trim()));
        }
        let _ = std::fs::write(&lua_file, lua_content);

        if !build_id.trim().is_empty() {
            let build_lua = lua_dir.join(format!("{}_{}.lua", app_id, build_id.trim()));
            let mut build_content = format!("-- Generated by LuaTools BuildLock\naddappid({}, 1)\n", app_id);
            if !manifest.trim().is_empty() {
                build_content.push_str(&format!("setManifestid({}, {})\n", app_id, manifest.trim()));
            }
            let _ = std::fs::write(&build_lua, build_content);
        }
    }

    // 2. Remove any existing ACF so we write a clean stub
    for lib in &library_paths {
        let acf = std::path::PathBuf::from(lib)
            .join("steamapps")
            .join(format!("appmanifest_{}.acf", app_id));
        if acf.exists() {
            let _ = std::fs::remove_file(&acf);
        }
    }

    // 3. Create ACF stub with StateFlags = 4 (Fully Installed)
    // StateFlags=4 tells Steam the game is already on disk — prevents download queue.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name_str = app_name.clone().unwrap_or_else(|| format!("App {}", app_id));

    // Use provided install_dir, or fall back to cleaned game name
    let acf_install_dir = if let Some(ref dir) = install_dir {
        let p = std::path::PathBuf::from(dir);
        // Create directory if it doesn't exist so Steam can verify it
        let _ = std::fs::create_dir_all(&p);
        p.to_string_lossy().to_string()
    } else {
        name_str.chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { '_' })
            .collect::<String>()
    };

    let stub = format!(
        "\"AppState\"\n{{\n\
        \t\"appid\"\t\t\"{appid}\"\n\
        \t\"Universe\"\t\t\"1\"\n\
        \t\"name\"\t\t\"{name}\"\n\
        \t\"StateFlags\"\t\t\"4\"\n\
        \t\"installdir\"\t\t\"{dir}\"\n\
        \t\"LastUpdated\"\t\t\"{ts}\"\n\
        \t\"SizeOnDisk\"\t\t\"1\"\n\
        \t\"buildid\"\t\t\"{buildid}\"\n\
        \t\"LastOwner\"\t\t\"0\"\n\
        \t\"BytesToDownload\"\t\t\"0\"\n\
        \t\"BytesDownloaded\"\t\t\"0\"\n\
        \t\"AutoUpdateBehavior\"\t\t\"1\"\n\
        \t\"AllowOtherDownloadsWhileRunning\"\t\t\"0\"\n\
        \t\"ScheduledAutoUpdate\"\t\t\"0\"\n\
        \t\"UserConfig\"\n\
        \t{{\n\
        \t}}\n\
        \t\"MountedDepots\"\n\
        \t{{\n\
        \t}}\n\
        }}\n",
        appid = app_id,
        name = name_str,
        dir = acf_install_dir,
        buildid = build_id,
        ts = ts,
    );

    // Write the ACF to a library that can actually hold the game.
    let primary_lib = pick_install_library(&library_paths, &steam_path);
    let primary_lib = primary_lib.as_str();
    let steamapps = std::path::PathBuf::from(primary_lib).join("steamapps");
    std::fs::create_dir_all(&steamapps).map_err(|e| e.to_string())?;
    let acf_path = steamapps.join(format!("appmanifest_{}.acf", app_id));
    std::fs::write(&acf_path, stub).map_err(|e| e.to_string())?;
    // One game claimed by two libraries leaves Steam with two answers about
    // where it lives; the one pointing at nothing has to go.
    clear_orphan_manifests(&library_paths, &acf_path, &app_id);
    Ok(())
}

/// The `installdir` an ACF declares.
fn acf_install_dir_of(content: &str) -> Option<String> {
    content.lines().find_map(|l| {
        let l = l.trim();
        if !l.starts_with("\"installdir\"") {
            return None;
        }
        let parts: Vec<&str> = l.splitn(4, '"').collect();
        parts.get(3).map(|s| s.trim_end_matches('"').to_string())
    })
}

/// Whether a library folder holds any actual file, a few levels down.
fn folder_has_any_file(dir: &Path, depth: u32) -> bool {
    if depth > 3 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            return true;
        }
        if path.is_dir() && folder_has_any_file(&path, depth + 1) {
            return true;
        }
    }
    false
}

/// Whether this manifest describes an install that is not on disk.
///
/// The launcher writes ACFs with `StateFlags = 4` — fully installed — so Steam
/// will not queue a download for a game it is being handed. When the files
/// never arrive, or land in a different library, that stub is left telling
/// Steam a game is installed in a folder that does not exist. Pressing Play
/// then does nothing visible: Steam checks, finds nothing, and puts the Play
/// button back a few seconds later. It also hides the game from
/// `find_game_folder`, which is why the bypass stops offering to install
/// itself for exactly the games in this state.
///
/// A missing `installdir` is treated as *not* orphaned: better to leave a
/// manifest we cannot judge than to delete a real install.
fn manifest_is_orphaned(library: &Path, content: &str) -> bool {
    let Some(dir) = acf_install_dir_of(content) else { return false };
    if dir.trim().is_empty() {
        return false;
    }
    let folder = library.join("steamapps").join("common").join(dir);
    !folder.is_dir() || !folder_has_any_file(&folder, 0)
}

/// Removes manifests for `app_id` that point at nothing, in every library but
/// `keep`. Called before writing a new one so one game never ends up claimed
/// by two libraries at once — which became easier to hit once the install
/// library stopped always being the first in the list.
fn clear_orphan_manifests(library_paths: &[String], keep: &Path, app_id: &str) -> Vec<String> {
    let mut removed = Vec::new();
    for lib in library_paths {
        let acf = Path::new(lib)
            .join("steamapps")
            .join(format!("appmanifest_{}.acf", app_id));
        if acf == keep || !acf.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&acf) else { continue };
        if manifest_is_orphaned(Path::new(lib), &content) && std::fs::remove_file(&acf).is_ok() {
            removed.push(acf.to_string_lossy().to_string());
        }
    }
    removed
}

/// One entry the 0 B sweep removed, named so the UI can show what it did
/// rather than just a count.
#[derive(Serialize)]
struct RemovedManifest {
    app_id: String,
    name: String,
    library: String,
}

/// Drops `"<appid>"  "<bytes>"` rows from every `apps` block of
/// libraryfolders.vdf.
///
/// Deleting the ACF is only half of it, and the half people were left to do by
/// hand. Steam indexes these rows separately: leave one behind and Almacenamiento
/// keeps listing the game at 0 B with no manifest left to explain where the
/// entry came from — which is the exact symptom being fixed here.
///
/// The pattern matches a whole line of exactly two quoted numbers. The only
/// other numeric keys in the file are the `"0"`/`"1"` library headers, and each
/// of those is followed by a brace rather than a second quoted number, so they
/// cannot match.
fn unregister_apps_from_libraryfolders(steam_path: &str, app_ids: &[String]) -> usize {
    if app_ids.is_empty() {
        return 0;
    }
    let path = Path::new(steam_path)
        .join("steamapps")
        .join("libraryfolders.vdf");
    let Ok(content) = std::fs::read_to_string(&path) else { return 0 };

    let wanted: HashSet<&str> = app_ids.iter().map(|s| s.as_str()).collect();
    let row = regex::Regex::new(r#"^\s*"(\d+)"\s+"\d+"\s*$"#).expect("patrón literal");

    let mut dropped = 0usize;
    let kept: Vec<&str> = content
        .lines()
        .filter(|line| match row.captures(line) {
            Some(c) if wanted.contains(&c[1]) => {
                dropped += 1;
                false
            }
            _ => true,
        })
        .collect();

    if dropped == 0 {
        return 0;
    }

    // Temp file plus rename. A half-written libraryfolders.vdf costs the user
    // every library Steam knows about, which is a far worse outcome than the
    // 0 B rows this is here to remove.
    let tmp = path.with_extension("vdf.ragnarok-tmp");
    let mut body = kept.join("\n");
    body.push('\n');
    if std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
        dropped
    } else {
        let _ = std::fs::remove_file(&tmp);
        0
    }
}

/// Sweeps every library for manifests whose game folder is not there, and
/// unregisters them from libraryfolders.vdf as well.
///
/// These are the entries Steam shows at `0 B` in Almacenamiento and keeps
/// offering Play for, doing nothing when it is pressed. Only manifests pointing
/// at a missing or empty folder are touched, so a real install is never
/// removed — the folder check is the whole safety argument, and it is the same
/// predicate `install_game` uses to decide an ACF has gone stale.
#[command]
fn clean_orphan_manifests(steam_path: String) -> Result<Vec<RemovedManifest>, String> {
    // Steam rewrites libraryfolders.vdf from memory when it exits, which would
    // put every row back and make the sweep look like it silently did nothing.
    if RustSteamManager::is_running() {
        return Err(
            "Cerrá Steam antes de limpiar. Steam reescribe libraryfolders.vdf al salir y volvería a dejar las entradas de 0 B."
                .to_string(),
        );
    }

    let libs = crate::managers::manifest::ManifestManager::get_library_folders(&steam_path);
    let mut removed: Vec<RemovedManifest> = Vec::new();

    for lib in &libs {
        let steamapps = Path::new(lib).join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let filename = entry.file_name().to_string_lossy().into_owned();
            if !filename.starts_with("appmanifest_") || !filename.ends_with(".acf") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            if !manifest_is_orphaned(Path::new(lib), &content) {
                continue;
            }
            let app_id = crate::managers::manifest::ManifestManager::acf_value(&content, "appid")
                .unwrap_or_else(|| {
                    filename
                        .trim_start_matches("appmanifest_")
                        .trim_end_matches(".acf")
                        .to_string()
                });
            let name = crate::managers::manifest::ManifestManager::acf_value(&content, "name")
                .unwrap_or_else(|| format!("AppID {}", app_id));
            if std::fs::remove_file(&path).is_ok() {
                diag_log!("[0b] Eliminado {} ({}) en {}", filename, name, lib);
                removed.push(RemovedManifest {
                    app_id,
                    name,
                    library: lib.clone(),
                });
            }
        }
    }

    let ids: Vec<String> = removed.iter().map(|r| r.app_id.clone()).collect();
    let rows = unregister_apps_from_libraryfolders(&steam_path, &ids);
    diag_log!(
        "[0b] {} manifiesto(s) huérfanos eliminados y {} fila(s) quitadas de libraryfolders.vdf.",
        removed.len(),
        rows
    );

    Ok(removed)
}

#[cfg(test)]
mod orphan_manifest_tests {
    use super::{clear_orphan_manifests, manifest_is_orphaned, unregister_apps_from_libraryfolders};

    /// The half of the 0 B fix that nothing did until now. Removing the ACF
    /// alone leaves Steam still listing the game in Almacenamiento, because
    /// these rows are indexed separately.
    #[test]
    fn unregistering_drops_only_the_named_apps() {
        let root = std::env::temp_dir().join("ragnarok_libfolders_strip");
        let _ = fs::remove_dir_all(&root);
        let steamapps = root.join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        let vdf = steamapps.join("libraryfolders.vdf");
        fs::write(
            &vdf,
            "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Steam\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"730\"\t\t\"1234\"\n\t\t\t\"4001890\"\t\t\"0\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"1245620\"\t\t\"0\"\n\t\t}\n\t}\n}\n",
        )
        .unwrap();

        let steam = root.to_string_lossy().to_string();
        let dropped = unregister_apps_from_libraryfolders(
            &steam,
            &["4001890".to_string(), "1245620".to_string()],
        );
        assert_eq!(dropped, 2);

        let after = fs::read_to_string(&vdf).unwrap();
        assert!(!after.contains("4001890"), "la fila pedida tiene que irse");
        assert!(!after.contains("1245620"), "también en la segunda biblioteca");
        assert!(after.contains("\"730\""), "un juego real no se toca: {after}");
        // The library headers are numeric keys too — losing them would cost
        // the user every library Steam knows about.
        assert!(after.contains("\"0\"\n\t{"), "la cabecera \"0\" sobrevive: {after}");
        assert!(after.contains("\"1\"\n\t{"), "la cabecera \"1\" sobrevive: {after}");
        assert!(after.contains("C:\\\\Steam") && after.contains("D:\\\\SteamLibrary"));

        // Nothing to drop must not rewrite the file at all.
        assert_eq!(unregister_apps_from_libraryfolders(&steam, &["999999".to_string()]), 0);
        assert_eq!(unregister_apps_from_libraryfolders(&steam, &[]), 0);

        let _ = fs::remove_dir_all(&root);
    }
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rl_orphan_{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn acf(dir: &str) -> String {
        format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"1\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"{}\"\n}}\n", dir)
    }

    /// The state a user hits: Steam says installed, the folder is not there.
    #[test]
    fn a_missing_folder_is_orphaned() {
        let lib = scratch("missing");
        assert!(manifest_is_orphaned(&lib, &acf("Sparta")));
        let _ = fs::remove_dir_all(&lib);
    }

    /// An empty folder counts too — that is what a download that never
    /// started leaves behind.
    #[test]
    fn an_empty_folder_is_orphaned() {
        let lib = scratch("empty");
        fs::create_dir_all(lib.join("steamapps/common/Sparta")).unwrap();
        assert!(manifest_is_orphaned(&lib, &acf("Sparta")));
        let _ = fs::remove_dir_all(&lib);
    }

    /// A real install must never be swept away.
    #[test]
    fn a_real_install_is_kept() {
        let lib = scratch("real");
        let game = lib.join("steamapps/common/Sparta");
        fs::create_dir_all(game.join("bin")).unwrap();
        fs::write(game.join("bin/game.exe"), b"x").unwrap();
        assert!(!manifest_is_orphaned(&lib, &acf("Sparta")));
        let _ = fs::remove_dir_all(&lib);
    }

    /// Nothing to judge on: leave it alone rather than guess.
    #[test]
    fn a_manifest_without_installdir_is_kept() {
        let lib = scratch("nodir");
        assert!(!manifest_is_orphaned(&lib, "\"AppState\"\n{\n\t\"appid\"\t\t\"1\"\n}\n"));
        let _ = fs::remove_dir_all(&lib);
    }

    /// The duplicate case: same app claimed by two libraries, one of them a
    /// stub pointing nowhere. The stub goes, the real one stays.
    #[test]
    fn clears_the_stub_and_keeps_the_install() {
        let root = scratch("dupe");
        let a = root.join("libA");
        let b = root.join("libB");
        for lib in [&a, &b] {
            fs::create_dir_all(lib.join("steamapps")).unwrap();
        }
        // libA: a stub for a folder that does not exist.
        fs::write(a.join("steamapps/appmanifest_7.acf"), acf("Ghost")).unwrap();
        // libB: the real thing.
        let game = b.join("steamapps/common/Real");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("game.exe"), b"x").unwrap();
        let keep = b.join("steamapps/appmanifest_7.acf");
        fs::write(&keep, acf("Real")).unwrap();

        let libs = vec![a.to_string_lossy().to_string(), b.to_string_lossy().to_string()];
        let removed = clear_orphan_manifests(&libs, &keep, "7");

        assert_eq!(removed.len(), 1, "solo el stub deberia irse");
        assert!(!a.join("steamapps/appmanifest_7.acf").exists());
        assert!(keep.exists(), "el install real debe quedarse");
        let _ = fs::remove_dir_all(&root);
    }
}

/// Free bytes on the volume a path lives on.
fn free_space_at(path: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        // Deepest matching mount point wins: "C:\\" is a prefix of
        // "C:\\Program Files" too, and on Linux "/" prefixes everything.
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

/// Where the launcher keeps the user's chosen install library.
fn preferred_library_file() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("install_library.txt"))
}

/// The library the user picked, if it still exists.
fn preferred_library() -> Option<String> {
    let raw = std::fs::read_to_string(preferred_library_file()?).ok()?;
    let path = raw.trim().to_string();
    (!path.is_empty() && Path::new(&path).is_dir()).then_some(path)
}

/// Which Steam library a game should be registered in.
///
/// This used to be `library_paths.first()`, which is Steam's own folder on the
/// system drive. So every game Ragnarok added was assigned to C: no matter how
/// full it was — and once Steam has an app assigned to a library, its install
/// dialog reports "NOT ENOUGH SPACE" and offers no way to move it. Reported by
/// a user trying to install a 66 GB game with 19 GB free on C: and over 100 GB
/// free on two other drives.
///
/// The user's own choice wins when they have made one. Otherwise the roomiest
/// library is picked, which is the answer that is right by default: the
/// system drive is usually the tightest, and the big library is the one people
/// added a second drive for.
fn pick_install_library(library_paths: &[String], steam_path: &str) -> String {
    if let Some(chosen) = preferred_library() {
        // Only honour it if Steam still knows about it; a library removed from
        // Steam would leave the game invisible.
        if library_paths.iter().any(|l| l == &chosen) {
            return chosen;
        }
    }

    library_paths
        .iter()
        .filter_map(|lib| free_space_at(Path::new(lib)).map(|free| (lib.clone(), free)))
        .max_by_key(|(_, free)| *free)
        .map(|(lib, _)| lib)
        // Nothing readable: keep the old behaviour rather than inventing a path.
        .or_else(|| library_paths.first().cloned())
        .unwrap_or_else(|| steam_path.to_string())
}

#[derive(Serialize)]
struct SteamLibrary {
    path: String,
    free_bytes: u64,
    is_selected: bool,
}

/// The libraries Steam knows about, with what is left on each.
#[command]
#[allow(dead_code)]
fn list_steam_libraries(steam_path: String) -> Vec<SteamLibrary> {
    let libs = crate::managers::manifest::ManifestManager::get_library_folders(&steam_path);
    let chosen = pick_install_library(&libs, &steam_path);
    libs.into_iter()
        .map(|path| SteamLibrary {
            free_bytes: free_space_at(Path::new(&path)).unwrap_or(0),
            is_selected: path == chosen,
            path,
        })
        .collect()
}

/// Remembers where new games should be installed. An empty string clears it,
/// putting the choice back to "whichever library has the most room".
#[command]
#[allow(dead_code)]
fn set_install_library(path: String) -> Result<(), String> {
    let file = preferred_library_file().ok_or("No se pudo resolver la carpeta de datos")?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, path.trim()).map_err(|e| e.to_string())
}

/// Automated build downloader & lock installer for specific AppID + BuildID + ManifestID
#[command]
#[allow(dead_code)]
async fn download_build_manifest(
    steam_path: String,
    app_id: String,
    build_id: String,
    manifest_id: Option<String>,
    app_name: Option<String>,
    install_dir: Option<String>,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // 1. Fetch game depots info via steamcmd public API
    let api_url = format!("https://api.steamcmd.net/v1/info/{}", app_id);
    let mut depots: Vec<String> = Vec::new();
    // Known Steam infrastructure depots to exclude (Steam client, redistributables, etc.)
    let steam_infra_depots: std::collections::HashSet<&str> = [
        "228989", "228990", "228988", "1004", "1005", "1006",
        "228980", "228981", "228982", "228983", "228984",
    ].iter().cloned().collect();
    if let Ok(res) = client.get(&api_url).send().await {
        if let Ok(json) = res.json::<serde_json::Value>().await {
            if let Some(depot_obj) = json["data"][&app_id]["depots"].as_object() {
                for (key, val) in depot_obj {
                    if key.chars().all(|c| c.is_ascii_digit()) && !steam_infra_depots.contains(key.as_str()) {
                        let oslist = val["config"]["oslist"].as_str().unwrap_or("");
                        if oslist.is_empty() || oslist.to_lowercase().contains("windows") {
                            depots.push(key.clone());
                        }
                    }
                }
            }
        }
    }
    // Sort depots numerically — app-specific depots (e.g. appId+1) come before shared ones
    depots.sort_by_key(|d| d.parse::<u64>().unwrap_or(u64::MAX));

    // If API returned nothing, default to appId+1 (Steam convention for primary content depot)
    let primary_depot = depots.first().cloned().unwrap_or_else(|| format!("{}1", app_id));
    let manifest = manifest_id.unwrap_or_default();

    // 2. Write <appid>_<buildid>.lua and <appid>.lua tickets including depot level license
    let lua_dir = std::path::PathBuf::from(&steam_path).join("config").join("lua");
    if let Ok(_) = std::fs::create_dir_all(&lua_dir) {
        let lua_file = lua_dir.join(format!("{}.lua", app_id));
        let lua_content = format!(
            "-- Auto-generated by LuaTools\naddappid({}, 1)\naddappid({}, 1)\n",
            app_id, primary_depot
        );
        let _ = std::fs::write(&lua_file, lua_content);

        if !build_id.trim().is_empty() {
            let build_lua = lua_dir.join(format!("{}_{}.lua", app_id, build_id.trim()));
            let build_content = format!(
                "-- Auto-generated BuildLock\naddappid({}, 1)\naddappid({}, 1)\n",
                app_id, primary_depot
            );
            let _ = std::fs::write(&build_lua, build_content);
        }
    }

    // 3. Create or update ACF manifest stub with AutoUpdateBehavior = 1
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name_str = app_name.unwrap_or_else(|| format!("App {}", app_id));
    let clean_dir = name_str
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { '_' })
        .collect::<String>();

    let stub = format!(
        "\"AppState\"\n{{\n\
        \t\"appid\"\t\t\"{appid}\"\n\
        \t\"Universe\"\t\t\"1\"\n\
        \t\"name\"\t\t\"{name}\"\n\
        \t\"StateFlags\"\t\t\"4\"\n\
        \t\"installdir\"\t\t\"{dir}\"\n\
        \t\"LastUpdated\"\t\t\"{ts}\"\n\
        \t\"SizeOnDisk\"\t\t\"1\"\n\
        \t\"buildid\"\t\t\"{buildid}\"\n\
        \t\"LastOwner\"\t\t\"0\"\n\
        \t\"BytesToDownload\"\t\t\"0\"\n\
        \t\"BytesDownloaded\"\t\t\"0\"\n\
        \t\"AutoUpdateBehavior\"\t\t\"1\"\n\
        \t\"AllowOtherDownloadsWhileRunning\"\t\t\"0\"\n\
        \t\"ScheduledAutoUpdate\"\t\t\"0\"\n\
        \t\"UserConfig\"\n\
        \t{{\n\
        \t}}\n\
        \t\"MountedDepots\"\n\
        \t{{\n\
        \t}}\n\
        }}\n",
        appid = app_id,
        name = name_str,
        dir = clean_dir,
        buildid = build_id,
        ts = ts,
    );

    // Same reasoning as the other ACF write: not the Steam folder by default,
    // whichever library can hold the game.
    let libs = crate::managers::manifest::ManifestManager::get_library_folders(&steam_path);
    let target_lib = pick_install_library(&libs, &steam_path);
    let steamapps = std::path::PathBuf::from(&target_lib).join("steamapps");
    let _ = std::fs::create_dir_all(&steamapps);
    let acf_path = steamapps.join(format!("appmanifest_{}.acf", app_id));
    let _ = std::fs::write(&acf_path, stub);

    // 4. Use DepotDownloader to download the specific manifest (no Steam license required)
    if !manifest.trim().is_empty() {
        let dd_dir = std::path::PathBuf::from(&steam_path).join("ragnarok_tools");
        let dd_exe = dd_dir.join("DepotDownloader.exe");

        // Download DepotDownloader from GitHub if not present
        if !dd_exe.exists() {
            let _ = std::fs::create_dir_all(&dd_dir);
            let dd_url = "https://github.com/SteamRE/DepotDownloader/releases/latest/download/DepotDownloader-windows-x64.zip";
            let zip_path = dd_dir.join("DepotDownloader.zip");
            // The result of this download is handed straight to Expand-Archive
            // and then executed, so an error page written out as
            // DepotDownloader.zip used to leave a broken half-install behind
            // with nothing explaining it.
            if !download_guard::is_allowed_url(dd_url, GITHUB_HOSTS) {
                diag_log!("[depotdownloader] URL de descarga no confiable: {}", dd_url);
            } else if let Ok(resp) = client.get(dd_url).timeout(std::time::Duration::from_secs(60)).send().await {
                if let Ok(bytes) = resp.bytes().await {
                    match download_guard::verify_download(
                        "DepotDownloader",
                        &bytes,
                        download_guard::Payload::Zip,
                        None,
                    ) {
                        Err(e) => diag_log!("[depotdownloader] {}", e),
                        Ok(()) => {
                            let _ = std::fs::write(&zip_path, &bytes);
                            let _ = std::process::Command::new("powershell")
                                .args([
                                    "-NoProfile", "-NonInteractive", "-Command",
                                    &format!(
                                        "Expand-Archive -Force -Path '{}' -DestinationPath '{}'",
                                        zip_path.display(),
                                        dd_dir.display()
                                    ),
                                ])
                                .creation_flags(0x08000000)
                                .output();
                            let _ = std::fs::remove_file(&zip_path);
                        }
                    }
                }
            }
        }

        if dd_exe.exists() {
            let out_dir = steamapps.join("content").join(&app_id);
            let _ = std::fs::create_dir_all(&out_dir);
            // Spawn DepotDownloader directly with a new console so user sees prompts
            let _ = std::process::Command::new(&dd_exe)
                .args([
                    "-app", &app_id,
                    "-depot", &primary_depot,
                    "-manifest", manifest.trim(),
                    "-dir", out_dir.to_str().unwrap_or("."),
                ])
                .creation_flags(0x00000010) // CREATE_NEW_CONSOLE — shows login prompts
                .spawn()
                .ok();

            Ok(format!(
                "✅ ACF y licencia .lua creados.\n\n\
                🚀 DepotDownloader iniciado — revisa la ventana de consola que se abrió.\n\
                Ingresa tu usuario y contraseña de Steam cuando te los pida.\n\n\
                📁 Los archivos se descargarán en:\n{}\n\n\
                ℹ️ Depot: {}  |  Manifest: {}",
                out_dir.display(), primary_depot, manifest.trim()
            ))
        } else {
            Ok(format!(
                "✅ ACF y licencia .lua creados.\n\n\
                ⚠️ No se pudo descargar DepotDownloader automáticamente.\n\
                Descárgalo manualmente desde:\nhttps://github.com/SteamRE/DepotDownloader/releases\n\
                Y colócalo en: {}\n\n\
                Luego ejecuta:\nDepotDownloader.exe -app {} -depot {} -manifest {}",
                dd_dir.display(), app_id, primary_depot, manifest.trim()
            ))
        }
    } else {
        Ok(format!(
            "✅ Licencia .lua y manifiesto ACF creados para BuildID {} (Depot principal: {}).\n\
            Para descargar archivos, proporciona también el Manifest ID.",
            build_id, primary_depot
        ))
    }
}



#[command]
async fn launch_game(app_id: String, steam_path: String, app_name: String) -> Result<String, String> {
    RustSteamManager::launch_game(&app_id, &steam_path, &app_name)
        .map(|_| "Game launched successfully".to_string())
        .map_err(|e| e)
}

// Returns the on-disk catalog cache path with no freshness check at all —
// unlike sync_catalog's own 1-hour TTL gate, this exists purely so the
// frontend can paint the Library/Home instantly from whatever was last
// successfully fetched (even if days old) before the real network sync
// (sync_games) has a chance to run. A stale-but-present cache beats an
// empty screen while sync_games' Ryuu fetch retries (up to 30s+60s+90s)
// against a slow or absent connection.
#[command]
async fn get_cached_catalog_path(catalog_source: Option<String>) -> Result<Option<String>, String> {
    // The instant paint has to show the catalog the user chose. Hardcoded to
    // Ryuu's file, a Hubcap user saw Ryuu's list at every launch until the
    // network sync finished — which reads as the switch never having worked.
    let cache_file = if catalog_source.as_deref() == Some("hubcap") {
        crate::managers::hubcap::cache_path()
    } else {
        std::env::temp_dir().join("ragnarok_catalog_cache_v6.json")
    };
    if cache_file.exists() {
        Ok(Some(cache_file.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

/// Steam's 500 best sellers and 500 most played, for the catalog's ranking
/// filters. Same lists for both catalogs: they rank Steam, not a catalog.
#[command]
async fn get_steam_rankings(force: Option<bool>) -> Result<crate::managers::rankings::Rankings, String> {
    crate::managers::rankings::get(force.unwrap_or(false)).await
}

#[command]
async fn sync_games(
    app_handle: tauri::AppHandle,
    ryuu_api_key: Option<String>,
    github_url: String,
    force: Option<bool>,
    catalog_source: Option<String>,
    hubcap_api_key: Option<String>,
) -> Result<String, String> {
    // The Hubcap catalog is a separate list with its own cache, and never
    // touches Ryuu's. Kept apart so switching back and forth does not blend two
    // catalogs, and so it is never published to the GitHub mirror further
    // down, which exists specifically to stand in for Ryuu.
    if catalog_source.as_deref() == Some("hubcap") {
        use crate::managers::hubcap::{sync_catalog, CatalogProgress};
        let key = hubcap_api_key.unwrap_or_default();
        let handle = app_handle.clone();
        let emit = move |p: CatalogProgress| {
            let _ = handle.emit_all("catalog_progress", p);
        };
        let result = sync_catalog(&key, force.unwrap_or(false), &emit).await;
        // Always sent, success or not, so the on-screen indicator can never be
        // left spinning after the load has ended.
        let _ = app_handle.emit_all(
            "catalog_progress",
            CatalogProgress { games: 0, page: 0, waiting_secs: 0, checkpoint: false, done: true },
        );
        return result.map(|path| path.to_string_lossy().to_string());
    }

    // "Sincronizar catálogo" has to clear the cache it actually reads.
    //
    // This deleted v4 and v2 — both legacy names that sync_catalog already
    // cleans up on its own — while the file it honours is v5, behind a
    // one-hour TTL. So force did nothing: press the button within an hour of
    // the last sync and you get the identical cached catalog back, with no
    // error and no sign that anything was skipped. The auto-refresh that fires
    // when GitHub reports more games than the cache holds runs through the
    // same path, so the one route built to correct a stale catalog was the one
    // that could not.
    if force.unwrap_or(false) {
        for stale in [
            "ragnarok_catalog_cache_v6.json",
            "ragnarok_catalog_cache_v4.json",
            "ragnarok_catalog_cache_v2.json",
        ] {
            let _ = std::fs::remove_file(std::env::temp_dir().join(stale));
        }
    }
    let manager = RustGameManager::new();
    let catalog = manager.sync_catalog(&github_url, ryuu_api_key.as_deref()).await?;

    // Hand back the cache sync_catalog just wrote instead of writing a second
    // copy of it.
    //
    // This used to serialise the whole catalog again into
    // `ragnarok_catalog_ipc.json` purely to have a path to give the frontend —
    // the identical bytes sync_catalog had already written to
    // `ragnarok_catalog_cache_v6.json` a moment earlier. Measured on this
    // machine: 17.1 MB each, so every sync did two 17 MB serialisations and
    // two 17 MB writes, and left 34 MB on disk for one dataset.
    //
    // get_cached_catalog_path already hands the frontend that same file, so
    // there was never a second format or a second consumer to justify it.
    let cache_path = std::env::temp_dir().join("ragnarok_catalog_cache_v6.json");
    let out_path = if cache_path.is_file() {
        // The stale duplicate is dead weight the moment this ships.
        let _ = std::fs::remove_file(std::env::temp_dir().join("ragnarok_catalog_ipc.json"));
        cache_path
    } else {
        // sync_catalog only skips the write when it returned nothing, which is
        // an error above — but a missing cache must not mean an empty Library.
        let fallback = std::env::temp_dir().join("ragnarok_catalog_ipc.json");
        let json = serde_json::to_string(&catalog)
            .map_err(|e| format!("Failed to serialize catalog: {}", e))?;
        std::fs::write(&fallback, &json)
            .map_err(|e| format!("Failed to write catalog file: {}", e))?;
        fallback
    };

    let steam_path = match get_steam_path().await {
        Ok(p) => p,
        Err(_) => "C:\\Program Files (x86)\\Steam".to_string(),
    };

    // Owner-only (inert without a GitHub token): keep the GitHub-hosted
    // catalog mirror in sync so users whose ISP blocks generator.ryuu.lol
    // still get a working catalog. Backgrounded so the upload never delays
    // returning the catalog to the frontend.
    let mirror_catalog = catalog.clone();
    tokio::spawn(async move {
        publish_catalog_mirror_to_github(&mirror_catalog).await;
    });

    Ok(out_path.to_string_lossy().to_string())

}



// Ya no se invoca desde el frontend: el catálogo dejó de refrescarse solo, así
// que nadie pregunta cuántos juegos tiene el repo. Se deja la función (y fuera
// del invoke_handler) por si vuelve a hacer falta.
#[command]
#[allow(dead_code)]
async fn get_github_game_count() -> Result<usize, String> {
    let client = crate::make_client()?;
    // Git Trees API returns ALL files without the 1000-item limit of Contents API
    let res = client
        .get("https://api.github.com/repos/RagnarokManifests/games/git/trees/HEAD?recursive=1")
        .header("User-Agent", "Ragnarok-Launcher-App")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let tree_resp: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let count = tree_resp["tree"].as_array().map(|tree| {
        tree.iter().filter(|entry| {
            entry["path"].as_str()
                .map(|p| p.starts_with("files/") && p.ends_with(".zip"))
                .unwrap_or(false)
        }).count()
    }).unwrap_or(0);
    Ok(count)
}

#[command]
#[allow(dead_code)]
async fn check_installed(app_id: String, steam_path: String) -> Result<bool, String> {
    // Primary: check our JSON sidecar (covers games added without a fake ACF)
    let map = load_apps_map(&steam_path);
    if map.contains_key(&app_id) {
        return Ok(true);
    }
    // Fallback: check for ACF created by Steam itself (StateFlags 1 or 4)
    let library_paths = ManifestManager::get_library_folders(&steam_path);
    Ok(ManifestManager::is_properly_installed(&library_paths, &app_id))
}

#[command]
async fn is_steam_running() -> Result<bool, String> {
    Ok(RustSteamManager::is_running())
}

#[derive(Serialize)]
struct DownloadState {
    state_flags: u64,
    bytes_downloaded: u64,
    bytes_to_download: u64,
}

#[derive(Serialize)]
struct LibrarySpace {
    path: String,
    free_bytes: u64,
}

#[derive(Serialize)]
struct InstallReadiness {
    steam_running: bool,
    plugin_installed: bool,
    /// `config/lua/<appid>.lua` is in place.
    ticket_installed: bool,
    /// Manifests the ticket pins that are not in depotcache.
    manifests_missing: usize,
    /// Steam's own record of the install, once it has one.
    download: Option<DownloadState>,
    /// Every Steam library, roomiest first.
    libraries: Vec<LibrarySpace>,
}

/// Where one game's install stands, read from disk only — cheap enough for
/// the game page to ask every few seconds while it is open.
///
/// The page used to show three fixed sentences ("make sure Steam is open…")
/// whether or not any of it was already true; every answer here is something
/// this launcher could always read and never showed.
#[command]
async fn install_readiness(steam_path: String, app_id: String) -> Result<InstallReadiness, String> {
    let steam = std::path::Path::new(&steam_path);
    let libs = ManifestManager::get_library_folders(&steam_path);

    let download = libs.iter().find_map(|lib| {
        let acf = std::fs::read_to_string(
            std::path::Path::new(lib)
                .join("steamapps")
                .join(format!("appmanifest_{}.acf", app_id)),
        )
        .ok()?;
        let num = |k: &str| {
            ManifestManager::acf_value(&acf, k)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        };
        Some(DownloadState {
            state_flags: num("StateFlags"),
            bytes_downloaded: num("BytesDownloaded"),
            bytes_to_download: num("BytesToDownload"),
        })
    });

    let mut libraries: Vec<LibrarySpace> = libs
        .iter()
        .filter_map(|lib| {
            Some(LibrarySpace {
                path: lib.clone(),
                free_bytes: free_space_at(std::path::Path::new(lib))?,
            })
        })
        .collect();
    libraries.sort_by(|a, b| b.free_bytes.cmp(&a.free_bytes));

    Ok(InstallReadiness {
        steam_running: RustSteamManager::is_running(),
        plugin_installed: steam.join("OpenSteamTool.dll").is_file(),
        ticket_installed: steam
            .join("config")
            .join("lua")
            .join(format!("{}.lua", app_id))
            .is_file(),
        manifests_missing: crate::managers::manifest_vault::missing_for_app(&steam_path, &app_id).len(),
        download,
        libraries,
    })
}

/// Per-game facts for the Library cards.
#[derive(Serialize, Default, Debug, PartialEq)]
struct LibraryGameStats {
    /// Unix seconds; 0 when Steam has no record of it being played.
    last_played: u64,
    playtime_minutes: u64,
    downloading: bool,
    /// 0..1 while downloading.
    progress: f64,
    /// Steam has an update queued (StateFlags 0x2) that is not running.
    update_pending: bool,
    /// Manifests the ticket pins that are not in depotcache.
    manifests_missing: usize,
    size_on_disk: u64,
}

/// `LastPlayed` and `Playtime` per app from a `localconfig.vdf`, which lives
/// under `UserLocalConfigStore/Software/Valve/Steam/apps/<appid>`.
///
/// Text VDF, unlike the binary files `managers::vdf` reads, so it gets its own
/// small reader: quoted strings and braces, nothing else. Key case varies
/// between Steam versions ("Valve"/"valve"), so the path is compared without
/// it.
fn play_stats_from_localconfig(text: &str) -> std::collections::HashMap<String, (u64, u64)> {
    enum Tok { Str(String), Open, Close }
    let mut toks = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => { if let Some(n) = chars.next() { s.push(n); } }
                        '"' => break,
                        c => s.push(c),
                    }
                }
                toks.push(Tok::Str(s));
            }
            '{' => toks.push(Tok::Open),
            '}' => toks.push(Tok::Close),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' { break; }
                }
            }
            _ => {}
        }
    }

    const APPS_PATH: [&str; 5] = ["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"];
    let mut out: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Str(key) => match toks.get(i + 1) {
                Some(Tok::Open) => {
                    stack.push(key.clone());
                    i += 2;
                    continue;
                }
                Some(Tok::Str(value)) => {
                    let in_app = stack.len() == 6
                        && stack.iter().zip(APPS_PATH).all(|(a, b)| a.eq_ignore_ascii_case(b));
                    if in_app {
                        let n = value.trim().parse::<u64>().unwrap_or(0);
                        let entry = out.entry(stack[5].clone()).or_default();
                        if key.eq_ignore_ascii_case("LastPlayed") {
                            entry.0 = n;
                        } else if key.eq_ignore_ascii_case("Playtime") {
                            entry.1 = n;
                        }
                    }
                    i += 2;
                    continue;
                }
                _ => {}
            },
            Tok::Close => {
                stack.pop();
            }
            Tok::Open => {}
        }
        i += 1;
    }
    out
}

fn library_stats(steam_path: &str) -> std::collections::HashMap<String, LibraryGameStats> {
    let mut out: std::collections::HashMap<String, LibraryGameStats> = std::collections::HashMap::new();

    // Play history, from every Steam account signed in on this PC.
    if let Ok(users) = std::fs::read_dir(std::path::Path::new(steam_path).join("userdata")) {
        for user in users.flatten() {
            let Ok(text) = std::fs::read_to_string(user.path().join("config").join("localconfig.vdf")) else {
                continue;
            };
            for (app_id, (last, minutes)) in play_stats_from_localconfig(&text) {
                let s = out.entry(app_id).or_default();
                s.last_played = s.last_played.max(last);
                s.playtime_minutes = s.playtime_minutes.max(minutes);
            }
        }
    }

    // Steam's own record of each install.
    for lib in ManifestManager::get_library_folders(steam_path) {
        let Ok(entries) = std::fs::read_dir(std::path::Path::new(&lib).join("steamapps")) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(app_id) = name.strip_prefix("appmanifest_").and_then(|n| n.strip_suffix(".acf")) else {
                continue;
            };
            let Ok(acf) = std::fs::read_to_string(entry.path()) else { continue };
            let num = |k: &str| {
                ManifestManager::acf_value(&acf, k)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            let (flags, done, total) = (num("StateFlags"), num("BytesDownloaded"), num("BytesToDownload"));
            let s = out.entry(app_id.to_string()).or_default();
            s.downloading = total > 0 && done < total;
            s.progress = if total > 0 { done as f64 / total as f64 } else { 0.0 };
            s.update_pending = !s.downloading && flags & 0x2 != 0;
            s.size_on_disk = num("SizeOnDisk");
        }
    }

    for (app_id, missing) in crate::managers::manifest_vault::missing_counts(steam_path) {
        out.entry(app_id).or_default().manifests_missing = missing;
    }
    out
}

/// Everything the Library cards show about each game, read from disk in one
/// pass: when it was last played and for how long, whether Steam is
/// downloading or holding an update for it, and whether its ticket is missing
/// a manifest. The cards used to show only an AppID.
#[command]
async fn get_library_stats(steam_path: String) -> Result<std::collections::HashMap<String, LibraryGameStats>, String> {
    tokio::task::spawn_blocking(move || library_stats(&steam_path))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod library_stats_tests {
    use super::play_stats_from_localconfig;

    #[test]
    fn play_history_is_read_from_the_apps_block_only() {
        let text = r#""UserLocalConfigStore"
{
	"Software"
	{
		"valve"
		{
			"Steam"
			{
				"apps"
				{
					"1245620"
					{
						"cloud"
						{
							"last_sync_state"		"changesincloud"
						}
						"LastPlayed"		"1786778528"
						"Playtime"		"2034"
					}
					"730"
					{
						"LastPlayed"		"1700000000"
					}
				}
			}
		}
	}
	"friends"
	{
		"999"
		{
			"LastPlayed"		"5"
		}
	}
}"#;
        let stats = play_stats_from_localconfig(text);
        assert_eq!(stats.get("1245620"), Some(&(1786778528, 2034)));
        assert_eq!(stats.get("730"), Some(&(1700000000, 0)));
        assert!(!stats.contains_key("999"), "sólo cuenta lo que está bajo apps");
        assert!(!stats.contains_key("cloud"));
    }
}

static GAME_SIZES: std::sync::OnceLock<Mutex<std::collections::HashMap<String, u64>>> =
    std::sync::OnceLock::new();

/// A game's download size from SteamCMD's depot data, for the game page to
/// say how much room an install needs before it starts. Kept per session.
#[command]
async fn get_game_size(app_id: String) -> Result<u64, String> {
    let cache = GAME_SIZES.get_or_init(Default::default);
    if let Some(size) = cache.lock().ok().and_then(|c| c.get(&app_id).copied()) {
        return Ok(size);
    }
    let size = fetch_real_game_size(&make_client()?, &app_id).await;
    if size > 0 {
        if let Ok(mut c) = cache.lock() {
            c.insert(app_id, size);
        }
    }
    Ok(size)
}

#[command]
#[allow(dead_code)]
async fn check_admin_status() -> Result<bool, String> {
    Ok(ActivationManager::is_admin())
}

#[command]
async fn get_installed_app_ids(steam_path: String) -> Result<HashSet<String>, String> {
    let mut installed = HashSet::new();

    // Four sources, unioned, because each one knows about games the others
    // do not. A game missing from any single one of them is normal; a game
    // missing from all four genuinely is not set up.

    // 1. Ragnarok's apps sidecar — games this app registered itself.
    let map = load_apps_map(&steam_path);
    for id in map.keys() {
        installed.insert(id.clone());
    }

    // 2. Ragnarok's manifests sidecar, which is keyed by app id too.
    // The two sidecars are written at different points of the install, so one
    // can survive without the other — a run that got its depot manifests
    // written and then failed before the apps entry leaves the game recorded
    // here and nowhere else.
    for id in load_manifests_map(&steam_path).keys() {
        installed.insert(id.clone());
    }

    // 3. Whatever else is configured in config/lua.
    //
    // The sidecar is Ragnarok's private bookkeeping, and a game can be set up
    // in Steam without ever appearing in it: added by a previous install
    // whose sidecar was lost, added by a different tool writing into the same
    // folder, or restored by hand. Those games are unlocked and visible in
    // Steam, and the Library showed none of them, because the only two
    // sources it consulted were this sidecar and the list of games Steam has
    // physically downloaded — and a game that is in the library but not
    // installed is in neither.
    //
    // The .lua filename is the app id by convention (delete_game and the
    // install path both rely on it), so the folder listing is the inventory.
    for id in lua_configured_app_ids(&steam_path) {
        installed.insert(id);
    }

    // 4. Games Steam itself reports as installed, from the appmanifest ACFs.
    // The frontend also unions `get_really_installed_app_ids`, which is this
    // same scan — included here as well so this command answers the question
    // completely on its own rather than depending on the caller to make a
    // second call and merge the results correctly.
    for id in ManifestManager::list_installed_ids(&steam_path) {
        installed.insert(id);
    }

    Ok(installed)
}

/// App ids that have a `<appid>.lua` in Steam's config/lua folder.
///
/// Deliberately keyed off the filename rather than parsing `addappid(...)`
/// out of the file: a single .lua also lists its DLC and depot ids through
/// that same call, so reading the contents would report dozens of ids that
/// are not games and flood the Library with them.
fn lua_configured_app_ids(steam_path: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let config = std::path::PathBuf::from(steam_path).join("config");
    for dir in [config.join("lua"), config.join("stplug-in")] {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".lua") else { continue };
            if !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()) {
                out.insert(stem.to_string());
            }
        }
    }
    out
}

/// Unlike get_installed_app_ids, this does NOT union in Ragnarok's own apps
/// sidecar — only real Steam ACF files (appmanifest_<id>.acf with a valid
/// StateFlags) count. The sidecar gets an entry the instant "Instalar" is
/// clicked, before Steam has downloaded anything at all, so it's exactly the
/// wrong signal for "did Steam actually finish downloading this" — which is
/// what matters here: bootstrapping a real Steam client connection (achievement
/// read/write) for an app Steam has no real content for was confirmed to
/// make Steam's own Library UI flip that app from "Install" to "Play" with
/// nothing on disk, fixable only by restarting Steam.
#[command]
async fn get_really_installed_app_ids(steam_path: String) -> Result<HashSet<String>, String> {
    Ok(ManifestManager::list_installed_ids(&steam_path))
}

// Just the AppIDs Ragnarok itself put there (installed via the catalog,
// Bypass, or "Agregar Lua Manualmente") — i.e. load_apps_map()'s keys with
// no ACF cross-check at all. Used by the frontend to tell those apart from
// a real Steam install the catalog has never heard of (a legitimately-owned
// game), so destructive actions like "Desinstalar" (which deletes the real
// appmanifest_<id>.acf) only ever get offered for games Ragnarok actually
// manages.
#[command]
async fn get_ragnarok_managed_app_ids(steam_path: String) -> Result<Vec<String>, String> {
    Ok(load_apps_map(&steam_path).keys().cloned().collect())
}

#[command]
#[allow(dead_code)]
async fn setup_dlcs(steam_path: String, dlcs: Vec<String>) -> Result<String, String> {
    // Legacy: treat all dlcs as belonging to a generic entry (kept for backwards compat)
    // This just writes a lua with a single unnamed entry; real usage should call update_dlcs.
    let _ = (steam_path, dlcs);
    Ok("DLCs successfully configured".to_string())
}

/// Best-effort diagnosis for why no manifest/depot data could be found for
/// an app, queried from the same SteamCMD info endpoint used elsewhere in
/// this file. Distinguishes "this game genuinely has no public depot data
/// yet" (pre-release / token-gated on Valve's side) from an unknown/likely
/// transient failure, so the error shown to the user explains *why* instead
/// of just prompting a retry that can never succeed (e.g. a game still in
/// pre-release has no depot data for ANY source to find, no matter how many
/// times it's retried).
async fn diagnose_install_failure(client: &reqwest::Client, app_id: &str) -> Option<String> {
    let url = format!("https://api.steamcmd.net/v1/info/{}", app_id);
    let res = client.get(&url).send().await.ok()?;
    let data: serde_json::Value = res.json().await.ok()?;
    let entry = &data["data"][app_id];
    if entry.is_null() {
        return None;
    }

    let release_state = entry["common"]["releasestate"].as_str().unwrap_or("");
    let missing_token = entry["_missing_token"].as_bool().unwrap_or(false);

    if release_state == "prerelease" {
        return Some(
            "este juego todavía está en pre-lanzamiento en Steam — Valve aún no ha publicado sus datos de depot, así que ninguna fuente de manifiestos (SteamCMD, Ryuu, Hubcap) puede obtenerlos todavía. Espera a su lanzamiento oficial e inténtalo de nuevo.".to_string()
        );
    }
    if missing_token {
        return Some(
            "los datos de depot de este juego están protegidos por un token de acceso que las fuentes públicas de manifiestos no tienen (común en juegos muy nuevos o con acceso restringido). Puede que tengas que esperar a que esas fuentes lo indexen.".to_string()
        );
    }
    None
}

#[command]
async fn install_game(
    app_id: String,
    app_name: String,
    steam_path: String,
    manifest_source: Option<String>,
    hubcap_api_key: Option<String>,
) -> Result<String, String> {
    let manifest_source = manifest_source.unwrap_or_else(|| "auto".to_string());
    let hubcap_api_key = hubcap_api_key.unwrap_or_default();
    let source = TicketSource::parse(&manifest_source);
    // Per-depot manifests are never requested from Hubcap, whatever the choice.
    // Each one counts as a download against the key's daily limit, and a large
    // game has around twenty depots: asking per depot would spend a whole day's
    // allowance on a single install. Hubcap's ticket zip already carries the
    // manifests, so nothing is lost by leaving this to the mirrors.
    let per_depot_source = "github";

    // ── Pre-Step: Make sure the Steam plugin is actually installed ───────────
    // install_game only ever writes the Lua/manifest files — it has never
    // checked that OpenSteamTool.dll (the DLL-hijack that makes Steam
    // actually read those files) is present in the Steam folder. A brand
    // new user who installs a game straight from its detail page, without
    // ever pressing "Instalar Plugin" on Home first, got a false "success"
    // here: the Lua file was written correctly, but Steam had no hook
    // loaded to read it, so the game just silently never showed up or
    // downloaded — reported by users as "games don't download for new
    // people." Fail loudly and specifically instead of lying about success.
    let plugin_marker = std::path::PathBuf::from(&steam_path).join("OpenSteamTool.dll");
    if !plugin_marker.exists() {
        return Err(
            "El plugin de Steam todavía no está instalado en esta PC. Andá a Inicio y presioná \"Instalar Plugin\" una vez — después de eso, instalar juegos va a funcionar normalmente.".to_string()
        );
    }

    // ── Pre-Step: Cleanup orphaned ACFs ──────────────────────────────────────
    // If an ACF exists on a full drive (like C:) from a previous FAILED
    // attempt, Steam will stubbornly lock the installation to that drive and
    // say "NOT ENOUGH SPACE". We delete it so Steam shows the drive selection
    // dropdown again — but only when the game isn't already properly
    // installed. Without this check, re-adding an already-installed game
    // (e.g. after clearing and re-importing the whole library) wiped out a
    // perfectly valid manifest, making Steam think the game was never
    // installed and re-download it from scratch.
    let libs = ManifestManager::get_library_folders(&steam_path);
    if !ManifestManager::is_properly_installed(&libs, &app_id) {
        let _ = ManifestManager::delete_manifest(&libs, &app_id);
    }

    let client = make_client()?;

    // ── Step 0: Try Ragnarok's own GitHub-hosted ticket cache first ──────────
    // Faster than (and doesn't depend on) Ryuu's live generator or SteamCMD's
    // API being reachable — populated automatically after a successful
    // install via the normal Step 1/2 path below (owner-only write, see
    // cache_lua_ticket_to_github's doc comment). A cache hit sets the exact
    // same `depot_gids`/`download_result` shape Steps 1-2 would have
    // produced, so every downstream check (Step 4/5, the error branch right
    // below, the background size-patch task) works unmodified either way.
    let cache_hit = try_fetch_lua_cache(&app_id)
        .await
        .and_then(|bytes| apply_lua_cache_bundle(&bytes, &steam_path, &app_id).ok())
        .filter(|ids| !ids.is_empty());
    let used_cache = cache_hit.is_some();

    let (mut depot_gids, download_result): (std::collections::HashMap<String, String>, Result<HashSet<String>, String>) =
        if let Some(cached_depot_ids) = cache_hit {
            diag_log!("[install_game] Usando ticket cacheado en GitHub para {} ({} depot(s))", app_id, cached_depot_ids.len());
            (std::collections::HashMap::new(), Ok(cached_depot_ids))
        } else {
            // ── Step 1: Fetch manifest GIDs from SteamCMD → depotcache ───────
            // Done before the ticket ZIP download below so we know which depot
            // ids are actually valid for Windows (see Step 2's comment for why).
            let facts = fetch_and_cache_manifests(&client, &app_id, &steam_path, per_depot_source, &hubcap_api_key).await;
            // The ticket is sanitized against every depot Windows can use, not
            // only the ones with a manifest. Using the narrower set here meant a
            // DLC or shared-runtime depot lost its decryption key, and the
            // content then sat in Steam as owned-but-encrypted.
            let windows_depot_ids: HashSet<String> = facts.windows_ok.clone();
            let depot_gids = facts.gids;

            // ── Step 2: Download ZIP from GitHub/Ryuu generator ───────────────
            //    .lua  → config/lua/   (SteamTools license + depot ticket) —
            //            sanitized to drop addappid/setManifestid lines for
            //            depots outside windows_depot_ids. This ticket is
            //            generated by a third-party service and lists every
            //            depot indiscriminately, including platform-specific
            //            ones (e.g. the macOS build of a cross-platform game).
            //            Registering a macOS-only depot with Steam's Windows
            //            client is what was crashing Steam mid-download for
            //            games like Dave the Diver — filtering our own
            //            generated SteamTools.lua alone didn't fix it, because
            //            this separate file (loaded from the same folder)
            //            still had them.
            //    .manifest → steamapps/depotcache/  (depot ticket, untouched)
            //    The ticket itself now comes from Ryuu or Hubcap, chosen in
            //    Ajustes — see fetch_and_install_ticket.
            let dl_result = fetch_and_install_ticket(&app_id, &steam_path, &windows_depot_ids, source, &hubcap_api_key).await;
            if let Err(ref e) = dl_result {
                diag_log!("[install_game] Manifest ZIP download failed for {}: {}", app_id, e);
            }
            (depot_gids, dl_result)
        };

    // Ryuu's ticket is the authoritative source when it succeeds: if it
    // already declared a manifest for a depot, drop that depot from our own
    // SteamCMD-sourced set instead of ALSO writing a setManifestid for it.
    // Two Lua files in config/lua/ declaring two different manifest ids for
    // the same depot (ours from live SteamCMD data vs. the ticket's — which
    // can be stale right after a game update) is what was actually crashing
    // Steam for Dave the Diver even after the platform-filter fix.
    let ticket_depot_ids: HashSet<String> = download_result.clone().unwrap_or_default();
    for id in &ticket_depot_ids {
        depot_gids.remove(id);
    }

    // Not a cache hit (that path already has a working ticket by definition)
    // and something real got written — cache it for next time. Owner-only in
    // practice: cache_lua_ticket_to_github no-ops instantly if this PC has no
    // GitHub token configured, so this is inert for every user except the
    // one who set that up. Backgrounded so a slow GitHub commit never delays
    // reporting install success back to the user.
    if !used_cache && (!depot_gids.is_empty() || !ticket_depot_ids.is_empty()) {
        let all_depot_ids: Vec<String> = ticket_depot_ids.iter().cloned().chain(depot_gids.keys().cloned()).collect();
        let steam_path_bg = steam_path.clone();
        let app_id_bg = app_id.clone();
        tokio::spawn(async move {
            cache_lua_ticket_to_github(&steam_path_bg, &app_id_bg, &all_depot_ids).await;
        });
    }

    if !depot_gids.is_empty() {
        let mut mmap = load_manifests_map(&steam_path);
        mmap.insert(app_id.clone(), depot_gids.clone());
        save_manifests_map(&steam_path, &mmap);
    }

    // ── Step 3 removed: We do NOT create the ACF file.
    // Creating the ACF on the main steam_path (C:) was locking the installation
    // to the C drive, which only has 27.8 GB (not enough for games like Pragmata).
    // By letting Steam handle the ACF, the native install dialog will show the
    // drive selection dropdown so the user can pick D:, E:, or F:.

    let mut apps = load_apps_map(&steam_path);
    let already_tracked = apps.contains_key(&app_id);

    // For a brand-new game, only register it in the Ragnarok library if we
    // actually got usable data from *some* source (the ticket zip, our own
    // depot GIDs, or it was already set up in a previous attempt). Otherwise
    // the game would show as "Installed" here while Steam never received any
    // real depot/DLC data for it — i.e. it appears in the Ragnarok library
    // but not in Steam, which is exactly the "ghost entry" bug reported by users.
    if download_result.is_err() && depot_gids.is_empty() && ticket_depot_ids.is_empty() && !already_tracked {
        let display_name = if app_name.trim().is_empty() { app_id.clone() } else { app_name.clone() };
        let msg = match diagnose_install_failure(&client, &app_id).await {
            Some(reason) => format!(
                "No se pudo instalar \"{}\" (appid {}): {}",
                display_name, app_id, reason
            ),
            None => format!(
                "No se pudo obtener el manifiesto/depot para \"{}\" (appid {}), así que no se añadió a la librería para evitar que apareciera sin funcionar en Steam. Intenta de nuevo en unos minutos: {}",
                display_name,
                app_id,
                download_result.err().unwrap()
            ),
        };
        return Err(msg);
    }

    // ── Step 4: Register game ID in apps sidecar ──────────────────────────────
    apps.entry(app_id.clone()).or_insert_with(Vec::new);
    save_apps_map(&steam_path, &apps);

    // ── Step 5: Rebuild SteamTools.lua (addappid + setManifestid + DLCs) ─────
    rebuild_steam_tools(&steam_path)
        .map_err(|e| format!("SteamTools update failed: {}", e))?;

    // Steam's own install dialog/library often shows "0 B" for these injected
    // depots — it has no real size cached for a game it doesn't think is
    // owned. We deliberately don't create the ACF ourselves (see Step 3
    // above — that locked installs to the C: drive), so there's nothing to
    // patch yet; Steam creates its own ACF only once the user actually
    // confirms the install in its dialog. Poll for that to appear (on
    // whichever drive they picked) and patch its size fields with the real
    // total once it does, so the library stops showing "0 B" forever.
    {
        let app_id_bg = app_id.clone();
        let steam_path_bg = steam_path.clone();
        tokio::spawn(async move {
            let client = match make_client() {
                Ok(c) => c,
                Err(_) => return,
            };
            let size = fetch_real_game_size(&client, &app_id_bg).await;
            if size == 0 {
                return;
            }
            let libs = ManifestManager::get_library_folders(&steam_path_bg);
            for _ in 0..40 {
                // ~2 minutes at 3s intervals — plenty of time for the user to
                // pick a drive and click "Instalar" in Steam's own dialog.
                if let Some(lib) = find_acf_library(&libs, &app_id_bg) {
                    let _ = ManifestManager::update_size_on_disk(&lib, &app_id_bg, size);
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        });
    }

    // Before returning: whatever landed in depotcache during this install is
    // now the only copy on the machine, and it stays that way only until
    // something removes it. Capture it here rather than at the next startup.
    back_up_manifests_now(&steam_path, &format!("instalación de {}", app_id));

    // Put back what this machine already saved before judging what is
    // missing. Downloading a removed game again used to fail with its manifest
    // sitting in the vault: nothing here ever restored from it, and a new
    // ticket pinned to a newer build was never matched with the older one kept.
    let repins = crate::managers::manifest_vault::recover_for_app(&steam_path, &app_id).await;

    // The ticket once more, now that every manifest it pins has landed —
    // what a second press of Install used to do by hand.
    crate::managers::manifest_vault::reannounce_ticket(&steam_path, &app_id);

    // Then check whether it actually landed. Everything above is best-effort
    // about manifests, so an install where no mirror had the file still gets
    // here reporting success. That silence is the whole bug: Steam is the one
    // that eventually complains, hours later, with "UNKNOWN ERROR" or
    // "CONTENT SERVERS UNREACHABLE" — neither of which names the real cause or
    // anything the user can do.
    let missing = crate::managers::manifest_vault::missing_for_app(&steam_path, &app_id);
    if !missing.is_empty() {
        for m in &missing {
            diag_log!(
                "[install_game] Falta {}_{}.manifest tras instalar {}.",
                m.depot_id, m.gid, app_id
            );
        }
        let label = if app_name.trim().is_empty() {
            format!("AppID {}", app_id)
        } else {
            app_name.clone()
        };
        return Ok(crate::managers::manifest_sources::explain_missing(&client, &app_id, &label).await);
    }

    if download_result.is_err() {
        Ok("Juego añadido con datos parciales (el ticket de depot no se pudo descargar); si no aparece en Steam, reinténtalo".to_string())
    } else if !repins.is_empty() {
        Ok(format!(
            "Juego añadido. La versión más nueva de {} depot(s) no está disponible, así que se instala la versión anterior que guardó la bóveda.",
            repins.len()
        ))
    } else {
        Ok("Game added to library successfully".to_string())
    }
}

/// Copies whatever manifests an operation just put in depotcache into the
/// vault, right away.
///
/// The vault used to fill only at startup, which left the window that matters
/// wide open: a manifest that arrives mid-session and is gone before the next
/// launch was never seen at all. Installing a game and removing it again
/// without restarting — the obvious thing to do when testing whether a fix
/// worked — destroyed the only copy every single time.
///
/// Cheap enough to call on every install: it compares filenames and copies
/// only what is new.
fn back_up_manifests_now(steam_path: &str, reason: &str) {
    let (copied, _) = crate::managers::manifest_vault::back_up(steam_path);
    if copied > 0 {
        diag_log!("[vault] {} manifest(s) nuevos respaldados ({}).", copied, reason);
    }
    // Protect immediately so Steam cannot delete them on uninstall
    let protected = crate::managers::manifest_vault::protect_depotcache_manifests(steam_path);
    if protected > 0 {
        diag_log!("[vault] {} manifest(s) protegidos contra eliminación ({}).", protected, reason);
    }
}

/// Which service an install takes its ticket (.lua + manifests) from.
#[derive(Clone, Copy, PartialEq, Debug)]
enum TicketSource {
    /// What the "Ryuu" choice in Ajustes sends. Ryuu first; Hubcap only when a
    /// key is set and Ryuu failed or left a pinned manifest missing.
    Auto,
    /// Ryuu and nothing else. Not offered in the UI; kept for callers that
    /// must not spend Hubcap quota.
    Ryuu,
    /// What the "Hubcap" choice sends. Hubcap first, Ryuu when Hubcap cannot —
    /// most often because the day's downloads are used up.
    Hubcap,
}

impl TicketSource {
    /// Lenient on purpose. `"github"` was the old default value, stored in
    /// installs that predate the choice, and anything unrecognised is safer as
    /// Automático than as a source that might need a key the user never set.
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ryuu" => Self::Ryuu,
            "hubcap" => Self::Hubcap,
            _ => Self::Auto,
        }
    }
}

/// Fetches a ticket from the chosen source and installs it.
///
/// Modelled on how LuaTools offers several sources — Hubcap reached directly
/// with the user's own key, the rest without one — with one deliberate
/// difference in the automatic order. LuaTools tries key-gated sources first.
/// Here Ryuu goes first and Hubcap rescues it, because Hubcap's free tier is
/// 25 downloads a day: trying it first would spend that quota on games Ryuu
/// already serves for free, and leave nothing for the ones it does not.
///
/// "Rescue" covers both ways Ryuu comes up short: the download failing, and
/// the download succeeding with a ticket whose pinned manifests are still not
/// on disk — the case that otherwise surfaces hours later as Steam's
/// "UNKNOWN ERROR".
async fn fetch_and_install_ticket(
    app_id: &str,
    steam_path: &str,
    windows_depot_ids: &HashSet<String>,
    source: TicketSource,
    hubcap_key: &str,
) -> Result<HashSet<String>, String> {
    use crate::managers::downloader::RustDownloader;
    use crate::managers::hubcap;

    let has_key = hubcap::is_valid_key_format(hubcap_key);
    let allow = Some(windows_depot_ids);

    match source {
        TicketSource::Ryuu => {
            let bytes = RustDownloader::fetch_ryuu_ticket(app_id, None).await?;
            RustDownloader::install_ticket_bytes(&bytes, app_id, steam_path, allow, "Ryuu")
        }
        TicketSource::Hubcap => {
            let hub = if has_key {
                match hubcap::download_game(hubcap_key, app_id).await {
                    Ok(bytes) => RustDownloader::install_ticket_bytes(&bytes, app_id, steam_path, allow, "Hubcap"),
                    Err(e) => Err(e),
                }
            } else {
                Err("no hay una clave de Hubcap válida".to_string())
            };

            match hub {
                Ok(ids) => Ok(ids),
                // Falling back is what keeps a used-up daily allowance from
                // stopping every install until tomorrow.
                Err(hub_err) => {
                    diag_log!("[ticket] AppID {}: Hubcap no pudo ({}); se usa Ryuu.", app_id, hub_err);
                    match RustDownloader::fetch_ryuu_ticket(app_id, None).await {
                        Ok(bytes) => RustDownloader::install_ticket_bytes(&bytes, app_id, steam_path, allow, "Ryuu"),
                        Err(ryuu_err) => Err(format!(
                            "Ninguna fuente pudo entregar el juego. Hubcap: {} · Ryuu: {}",
                            hub_err, ryuu_err
                        )),
                    }
                }
            }
        }
        TicketSource::Auto => {
            let ryuu = match RustDownloader::fetch_ryuu_ticket(app_id, None).await {
                Ok(bytes) => RustDownloader::install_ticket_bytes(&bytes, app_id, steam_path, allow, "Ryuu"),
                Err(e) => Err(e),
            };
            if !has_key {
                return ryuu;
            }

            let missing = match &ryuu {
                Ok(_) => {
                    // The vault and the mirror first: a manifest this machine
                    // already saved must not spend Hubcap's daily quota.
                    crate::managers::manifest_vault::restore_exact_for_app(steam_path, app_id).await;
                    crate::managers::manifest_vault::missing_for_app(steam_path, app_id).len()
                }
                Err(_) => 0,
            };
            if ryuu.is_ok() && missing == 0 {
                return ryuu;
            }

            match &ryuu {
                Err(e) => diag_log!("[ticket] AppID {}: Ryuu falló ({}); se intenta con Hubcap.", app_id, e),
                Ok(_) => diag_log!(
                    "[ticket] AppID {}: el ticket de Ryuu dejó {} manifest(s) sin conseguir; se intenta con Hubcap.",
                    app_id,
                    missing
                ),
            }

            let hub = match hubcap::download_game(hubcap_key, app_id).await {
                Ok(bytes) => RustDownloader::install_ticket_bytes(&bytes, app_id, steam_path, allow, "Hubcap"),
                Err(e) => Err(e),
            };
            match (hub, ryuu) {
                (Ok(ids), _) => Ok(ids),
                // Ryuu's partial ticket is still installed and still better
                // than nothing; the post-install check reports what is missing.
                (Err(hub_err), Ok(ids)) => {
                    diag_log!("[ticket] AppID {}: Hubcap tampoco pudo ({}).", app_id, hub_err);
                    Ok(ids)
                }
                (Err(hub_err), Err(ryuu_err)) => Err(format!(
                    "Ninguna fuente pudo entregar el juego. Ryuu: {} · Hubcap: {}",
                    ryuu_err, hub_err
                )),
            }
        }
    }
}

/// A Hubcap key's downloads used and left today, without spending one.
#[command]
async fn hubcap_usage(key: String) -> Result<crate::managers::hubcap::HubcapUsage, String> {
    crate::managers::hubcap::usage(&key).await
}

/// App id → `"ryuu"` or `"hubcap"`, for the Library's source labels.
///
/// Games this launcher installed before the record existed are reported as
/// Ryuu: until Hubcap support arrived, Ryuu was the only service Ragnarok
/// installed from. Games Ragnarok never installed — bought and installed in
/// Steam directly — get no entry, and therefore no label.
#[command]
async fn get_install_sources(steam_path: String) -> HashMap<String, String> {
    let mut sources = crate::managers::install_sources::load();
    for app_id in load_apps_map(&steam_path).keys() {
        sources.entry(app_id.clone()).or_insert_with(|| "ryuu".to_string());
    }
    sources
}

#[cfg(test)]
mod ticket_source_tests {
    use super::TicketSource;

    #[test]
    fn the_stored_setting_maps_to_a_source() {
        assert_eq!(TicketSource::parse("ryuu"), TicketSource::Ryuu);
        assert_eq!(TicketSource::parse("hubcap"), TicketSource::Hubcap);
        assert_eq!(TicketSource::parse("  Hubcap "), TicketSource::Hubcap);
        assert_eq!(TicketSource::parse("auto"), TicketSource::Auto);
        // The value older installs stored before the choice existed.
        assert_eq!(TicketSource::parse("github"), TicketSource::Auto);
        assert_eq!(TicketSource::parse(""), TicketSource::Auto);
        assert_eq!(TicketSource::parse("lo-que-sea"), TicketSource::Auto);
    }
}

/// Finds which library folder actually holds this app's ACF — Steam may
/// create it on a different drive than steam_path if the user picked one via
/// the native drive-selection dialog.
fn find_acf_library(library_paths: &[String], app_id: &str) -> Option<String> {
    for lib in library_paths {
        let acf = std::path::PathBuf::from(lib)
            .join("steamapps")
            .join(format!("appmanifest_{}.acf", app_id));
        if acf.exists() {
            return Some(lib.clone());
        }
    }
    None
}

#[command]
#[allow(dead_code)]
async fn get_library_folders(steam_path: String) -> Result<Vec<String>, String> {
    Ok(ManifestManager::get_library_folders(&steam_path))
}

#[derive(serde::Serialize)]
struct DlcResponse {
    cached: bool,
    items: Vec<serde_json::Value>,
}

#[command]
async fn get_dlcs(app_id: String, steam_path: Option<String>) -> Result<DlcResponse, String> {
    use tokio::sync::Semaphore;
    use std::sync::Arc;

    // ── Check disk cache first (valid for 24h) ────────────────────────────────
    let cache_dir = std::env::var("APPDATA")
        .map(|p| std::path::PathBuf::from(p).join("Ragnarok Launcher").join("dlc_cache"))
        .unwrap_or_else(|_| std::env::temp_dir().join("ragnarok_dlc_cache"));
    let cache_file = cache_dir.join(format!("{}.json", app_id));

    if let Ok(metadata) = std::fs::metadata(&cache_file) {
        if let Ok(modified) = metadata.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                // Use cache if under 24 hours old
                if elapsed.as_secs() < 86400 {
                    if let Ok(json_str) = std::fs::read_to_string(&cache_file) {
                        if let Ok(cached) = serde_json::from_str::<Vec<serde_json::Value>>(&json_str) {
                            return Ok(DlcResponse { cached: true, items: cached });
                        }
                    }
                }
            }
        }
    }

    let client = make_client()?;

    // Step 1: Fetch full app details (filters=dlc is unreliable — get everything)
    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={}&l=english",
        app_id
    );
    // The store answers 429 with an empty body while it is rate limiting, and
    // an HTML page when it is having a bad moment. Both went straight into the
    // JSON parser, and its complaint — "expected value at line 1 column 1" —
    // was what the DLC Manager showed. One retry covers the short limits; past
    // that the store is treated as having said nothing, which the appinfo.vdf
    // fallback below already knows how to handle.
    let mut data = serde_json::Value::Null;
    let mut store_answered = false;
    for attempt in 0..2 {
        if attempt > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
        }
        let res = match client.get(&url).send().await {
            Ok(res) => res,
            Err(e) => {
                crate::diag_log!("[dlc] {}: sin respuesta de la tienda ({})", app_id, e);
                continue;
            }
        };
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(parsed) if !parsed.is_null() => {
                data = parsed;
                store_answered = true;
                break;
            }
            _ => crate::diag_log!(
                "[dlc] {}: la tienda respondió {} sin JSON ({} bytes)",
                app_id,
                status,
                text.len()
            ),
        }
    }

    // Two ways this used to come back empty. The reply is often keyed by a
    // different store id (see `appdetails_entry`), which read as "no data" and
    // was reported to the user as a region lock. And an app the store really
    // does not carry still has its DLC listed in Steam's own appinfo.vdf, so
    // that is asked before giving up.
    let dlc_ids: Vec<String> = if let Some(entry) = appdetails_entry(&data, &app_id) {
        entry["data"]["dlc"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|id| id.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        let local = steam_path
            .as_deref()
            .map(|sp| crate::managers::appinfo::dlc_ids(sp, &app_id))
            .unwrap_or_default();
        if local.is_empty() {
            return Err(if store_answered {
                "La tienda de Steam no tiene datos de este juego (sin publicar o no disponible en tu región), \
                 y Steam tampoco guarda su lista de DLCs en esta PC."
                    .to_string()
            } else {
                "Steam no respondió al pedir los DLCs (suele pasar cuando limita las peticiones). \
                 Prueba de nuevo en un minuto con el botón de recargar."
                    .to_string()
            });
        }
        crate::diag_log!("[dlc] {} vino de appinfo.vdf: {} DLCs", app_id, local.len());
        local
    };

    // Every DLC, not the first fifty.
    //
    // The cap existed to keep the name lookups below from becoming a flood —
    // one Steam request per DLC. But it dropped the rest *silently*: Euro
    // Truck Simulator 2 has 108 DLCs (checked against Steam's own API while
    // fixing this), so a user saw 50, with no explanation and no way to reach
    // the other 58. Reported exactly that way.
    //
    // The id is what goes into the .lua and actually unlocks the DLC; the
    // name is only there so the list is readable. So every id survives, and a
    // name that cannot be fetched falls back to its id — which the lookup
    // below already does on any failure. Losing a label is a much smaller
    // problem than losing the DLC.
    //
    // Batching was the obvious alternative and is not available: Steam's
    // appdetails endpoint answers 400 to a comma-separated list of ids, with
    // and without `filters=basic`. Both tested.

    if dlc_ids.is_empty() {
        return Ok(DlcResponse { cached: false, items: Vec::new() });
    }

    // Step 2: Fetch names for each DLC ID in parallel (max 5 concurrent)
    let sem = Arc::new(Semaphore::new(5));
    let mut handles = Vec::new();

    for dlc_id in &dlc_ids {
        let c = client.clone();
        let sem = sem.clone();
        let id = dlc_id.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            let detail_url = format!(
                "https://store.steampowered.com/api/appdetails?appids={}&filters=basic&l=english",
                id
            );
            // Small delay to be gentle on Steam API
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let name = match c.get(&detail_url).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(body) => {
                        let parsed: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                        appdetails_entry(&parsed, &id)
                            .and_then(|e| e["data"]["name"].as_str())
                            .unwrap_or(&id)
                            .to_string()
                    }
                    Err(_) => id.clone(),
                },
                Err(_) => id.clone(),
            };
            serde_json::json!({ "id": id, "name": name })
        }));
    }

    let mut results: Vec<serde_json::Value> = Vec::new();
    for h in handles {
        if let Ok(entry) = h.await {
            results.push(entry);
        }
    }

    // ── Save to disk cache ────────────────────────────────────────────────────
    if !results.is_empty() {
        if let Ok(json_str) = serde_json::to_string(&results) {
            let _ = std::fs::create_dir_all(&cache_dir);
            let _ = std::fs::write(&cache_file, json_str);
        }
    }

    Ok(DlcResponse { cached: false, items: results })
}

/// Checks Steam's own official "content descriptors" for each app ID and reports
/// whether the store page self-declares nudity/sexual content.
///
/// This is far more reliable than guessing from the game's name: Steam requires
/// developers to disclose specific content descriptor IDs on the store page.
/// IDs 1, 3 and 4 correspond to nudity/sexual content ("Some Nudity or Sexual
/// Content", "Adult Only Sexual Content", "Frequent Nudity or Sexual Content");
/// ID 2 is violence/gore and ID 5 is generic "Mature Content" — neither implies
/// adult/sexual content on their own, so games with only those are left
/// undetermined (the caller falls back to its own heuristics for them).
#[command]
async fn check_adult_content_batch(app_ids: Vec<String>) -> Result<std::collections::HashMap<String, bool>, String> {
    use tokio::sync::Semaphore;
    use std::sync::Arc;

    const SEXUAL_DESCRIPTOR_IDS: [u64; 3] = [1, 3, 4];

    let client = make_client()?;
    let sem = Arc::new(Semaphore::new(5));
    let mut handles = Vec::new();

    for app_id in app_ids {
        let c = client.clone();
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            let url = format!(
                "https://store.steampowered.com/api/appdetails?appids={}&l=english",
                app_id
            );
            // None = the request/parse itself failed — genuinely unknown, don't
            // touch this app's classification either way. Some(bool) = Steam
            // actually answered, so it's a real confirmed result (positive OR
            // negative) — the catalog's own nsfw tag can come from an
            // unreliable/loosely-tagged source, so a confirmed "no" here is
            // just as useful as a confirmed "yes" for correcting it.
            let result: Option<bool> = match c.get(&url).send().await {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Some(entry) = appdetails_entry(&json, &app_id) {
                            let ids = entry["data"]["content_descriptors"]["ids"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                            Some(
                                ids.iter()
                                    .filter_map(|v| v.as_u64())
                                    .any(|id| SEXUAL_DESCRIPTOR_IDS.contains(&id)),
                            )
                        } else {
                            None // Steam has nothing on this app — leave the catalog's own tag alone
                        }
                    }
                    Err(_) => None,
                },
                Err(_) => None,
            };
            (app_id, result)
        }));
    }

    let mut result = std::collections::HashMap::new();
    for h in handles {
        if let Ok((id, Some(is_adult))) = h.await {
            result.insert(id, is_adult);
        }
    }

    Ok(result)
}

#[command]
async fn delete_game(
    app_id: String,
    steam_path: String,
    delete_files: Option<bool>,
) -> Result<String, String> {
    let library_paths = ManifestManager::get_library_folders(&steam_path);

    // Resolved BEFORE the manifest is deleted, because the manifest is the
    // only thing that says where the game lives: `find_game_folder` reads
    // `installdir` out of the ACF. Look it up afterwards and there is nothing
    // left to look it up with.
    let install_folder = crate::managers::crackers::find_game_folder(&steam_path, &app_id);
    let folder_size = install_folder.as_deref().map(dir_size_bytes).unwrap_or(0);

    // 1. Remove ACF if it exists (may have been created by Steam during actual install)
    let _ = ManifestManager::delete_manifest(&library_paths, &app_id); // ignore if not found

    // 2. Remove from apps + manifests sidecars
    let mut apps = load_apps_map(&steam_path);
    apps.remove(&app_id);
    save_apps_map(&steam_path, &apps);

    // Snapshot before the removals below, which include this game's own
    // depot manifests. Uninstalling is the single most common way one of them
    // disappears, and reinstalling later cannot get it back: the live mirror
    // does not carry every manifest, and Steam's CDN answers 401 without
    // ownership. Backing up here means an uninstall stops being destructive.
    back_up_manifests_now(&steam_path, &format!("antes de desinstalar {}", app_id));

    let mut mmap = load_manifests_map(&steam_path);
    let mut depot_ids = Vec::new();
    
    if let Some(depots) = mmap.remove(&app_id) {
        // The depot manifests are deliberately LEFT in depotcache.
        //
        // Deleting them here is what made this app's worst loop unbreakable.
        // Traced in content_log.txt: an install writes the manifest, an
        // uninstall a moment later deletes it, and the next install cannot get
        // it back — the GitHub mirrors do not carry every manifest, and Steam's
        // CDN answers 401 Unauthorized without ownership. Users saw only
        // "UNKNOWN ERROR" / "CONTENT SERVERS UNREACHABLE", which names neither
        // the cause nor anything they can do, and reinstalling walked straight
        // back into it. One case cost several hours of a download that had
        // already succeeded once.
        //
        // Keeping them costs about 750 KB each and nothing else: a manifest is
        // inert unless some `.lua` pins it, and the sidecar entry removed just
        // above is what actually unregisters the game. Steam prunes its own
        // depotcache anyway, and the vault holds a copy either way — so the
        // only thing this deletion ever achieved was guaranteeing a restore
        // would be needed.
        for (depot_id, _gid) in &depots {
            depot_ids.push(depot_id.clone());
        }
        diag_log!(
            "[uninstall] {}: se conservan {} manifest(s) en depotcache para que reinstalar no dependa de la red.",
            app_id,
            depots.len()
        );
    }
    save_manifests_map(&steam_path, &mmap);

    // 3. Force rebuild the SteamTools.lua file
    rebuild_steam_tools(&steam_path)
        .map_err(|e| format!("SteamTools update failed: {}", e))?;

    // 4. Delete corresponding Lua files from steam_path/config/lua.
    // Before removing the Ryuu ticket file ({app_id}.lua), read the depot ids
    // it declares via setManifestid — those depots were deliberately left out
    // of our own manifests_map (to avoid the duplicate/conflicting-manifest
    // bug fixed in install_game), so step 2 above never saw them and their
    // .manifest files in depotcache would otherwise be left behind forever.
    let stplugin_path = std::path::PathBuf::from(&steam_path).join("config").join("lua");
    let mut ticket_depot_ids: Vec<String> = Vec::new();
    if stplugin_path.exists() {
        let ticket_path = stplugin_path.join(format!("{}.lua", app_id));
        if let Ok(content) = std::fs::read_to_string(&ticket_path) {
            for line in content.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("setManifestid(") {
                    if let Some(comma) = rest.find(',') {
                        let id = &rest[..comma];
                        if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                            ticket_depot_ids.push(id.to_string());
                        }
                    }
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(&stplugin_path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".lua") {
                    let matches_app = name == format!("{}.lua", app_id);
                    let matches_depot = depot_ids.iter().any(|d| name == format!("{}.lua", d));
                    if matches_app || matches_depot {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }

    // The ticket's own depot manifests are left alone too, for the same reason
    // as above — and this sweep was the more dangerous of the two, because it
    // matched on the depot id prefix alone. Every manifest for that depot went,
    // including builds this game never used and any a different app shares.

    // 5. Verify that the app is actually gone from the maps
    let final_apps = load_apps_map(&steam_path);
    if final_apps.contains_key(&app_id) {
        return Err(format!("Failed to verify deletion of game {}", app_id));
    }

    // 6. The files themselves.
    //
    // Everything above unregisters the game — the ACF, the sidecars, the lua
    // and the depot manifests — so Steam stops listing it as installed. None
    // of it touches steamapps/common, so until now "desinstalado" left the
    // whole install on disk: Steam said it was gone, the folder was still
    // there, and nothing told the user which of the two was true.
    //
    // Deleting it is the caller's decision, not a default. This runs last so
    // that a refusal here still leaves the game properly unregistered.
    let mut freed_note = String::new();
    if delete_files.unwrap_or(false) {
        match install_folder {
            Some(folder) => {
                // The path came from an ACF field, so it is checked against
                // the libraries Steam itself reports rather than trusted:
                // recursive deletion is not something to point at a path this
                // process did not construct. It must sit directly inside a
                // known <library>/steamapps/common.
                let inside_a_library = library_paths.iter().any(|lib| {
                    let common = std::path::PathBuf::from(lib).join("steamapps").join("common");
                    folder.parent().map(|p| p == common.as_path()).unwrap_or(false)
                });

                if !inside_a_library {
                    freed_note = format!(
                        " Los archivos NO se borraron: {} no está dentro de una biblioteca de Steam conocida.",
                        folder.display()
                    );
                } else if crate::managers::crackers::is_game_running(&folder) {
                    freed_note = " Los archivos NO se borraron: el juego está abierto. Cerralo y volvé a intentar.".to_string();
                } else {
                    match std::fs::remove_dir_all(&folder) {
                        Ok(()) => {
                            freed_note = format!(" Se liberaron {}.", human_bytes(folder_size));
                        }
                        Err(e) => {
                            freed_note = format!(
                                " Los archivos NO se borraron ({}). Siguen en {}.",
                                e,
                                folder.display()
                            );
                        }
                    }
                }
            }
            None => {
                freed_note = " No se encontró la carpeta de instalación, así que no había archivos que borrar.".to_string();
            }
        }
    } else if let Some(folder) = install_folder {
        // Said out loud rather than left for the user to discover by opening
        // the folder later and finding the game still sitting there.
        freed_note = format!(
            " Los archivos ({}) siguen en {}.",
            human_bytes(folder_size),
            folder.display()
        );
    }

    Ok(format!("Juego {} desinstalado.{}", app_id, freed_note))
}

/// Total size of a directory tree, best effort — anything unreadable is
/// skipped rather than aborting the walk.
fn dir_size_bytes(dir: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

fn human_bytes(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

#[command]
async fn get_steam_path() -> Result<String, String> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey("Software\\Valve\\Steam") {
            if let Ok(path) = key.get_value::<String, _>("SteamPath") {
                // Registry stores forward slashes on some systems
                return Ok(path.replace('/', "\\"));
            }
        }
    }
    Ok("C:\\Program Files (x86)\\Steam".to_string())
}


/// Lists every real Steam client installation found on this PC (see
/// `RustSteamManager::detect_steam_installations` for why this matters:
/// some users have two separate Steam installs on different drives).
#[command]
async fn detect_steam_installations() -> Result<Vec<String>, String> {
    Ok(crate::managers::steam::RustSteamManager::detect_steam_installations())
}

// Lets advanced users drop in their own Lua/manifest pack (e.g. one they got
// from a source Ryuu's ticket API doesn't cover) — requested by users on
// Discord. Community packs are distributed as .zip/.rar bundles containing
// one or more .lua files plus their matching .manifest depot tickets — the
// exact same shape as a Ryuu ticket ZIP — so this reuses the project's
// existing archive extractor and routes files the same way
// download_and_install does: .lua → config/lua/, .manifest → depotcache/.
// No AppID input needed — every file's own name determines what it's for,
// so this just extracts and routes whatever the archive actually contains.
// Placed alongside our own generated Lua, not merged with it — OpenSteamTool
// loads every file in config/lua/ together.
#[derive(serde::Serialize)]
struct ManualLuaGame {
    id: String,
    name: String,
    image: Option<String>,
}

#[derive(serde::Serialize)]
struct ManualLuaResult {
    message: String,
    games: Vec<ManualLuaGame>,
}

/// Registers each AppID the same way the normal download flow does
/// (Ragnarok sidecar + fake ACF marking it installed) and looks up its real
/// name/header image, so it shows up in Library/Achievements even when it
/// isn't in Ryuu's catalog at all. Shared by install_manual_lua and the
/// "detect already-activated games" scan — both end up with a list of
/// AppIDs that have a valid .lua ticket in config/lua/ but aren't tracked
/// yet, regardless of which tool actually wrote that .lua file.
async fn register_manual_games(steam_path: &str, app_ids: &[String]) -> Vec<ManualLuaGame> {
    let mut apps = load_apps_map(steam_path);
    for app_id in app_ids {
        apps.entry(app_id.clone()).or_insert_with(Vec::new);
    }
    save_apps_map(steam_path, &apps);

    let mut games = Vec::new();
    let client = make_client().ok();
    for app_id in app_ids {
        let mut name = format!("App {}", app_id);
        if let Some(client) = &client {
            let url = format!(
                "https://store.steampowered.com/api/appdetails?appids={}&filters=basic&l=english",
                app_id
            );
            if let Ok(resp) = client.get(&url).send().await {
                if let Ok(text) = resp.text().await {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(n) = appdetails_entry(&data, app_id)
                            .and_then(|e| e["data"]["name"].as_str())
                        {
                            name = n.to_string();
                        }
                    }
                }
            }
        }
        let image = Some(format!(
            "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg",
            app_id
        ));

        let _ = crate::managers::manifest::ManifestManager::create_fake_manifest(steam_path, app_id, &name);

        // create_fake_manifest always writes SizeOnDisk/BytesToDownload as
        // literal "0" (it has no way to know the real size at creation time)
        // — that's exactly what makes Steam show "0 B" for these games
        // forever, since nothing else ever corrects it after the fact.
        if let Some(client) = &client {
            let size = fetch_real_game_size(client, app_id).await;
            if size > 0 {
                let _ = crate::managers::manifest::ManifestManager::update_size_on_disk(steam_path, app_id, size);
            }
        }

        games.push(ManualLuaGame {
            id: app_id.clone(),
            name,
            image,
        });
    }
    games
}

// The three commands below wrap existing, previously-implemented logic in
// RustUnlockerManager that was never actually exposed as a Tauri command —
// the Bypass tab's "Goldberg Emu" sub-tab and Tools tab's "Generar
// Configuración" button called invoke('apply_goldberg'/'uninstall_goldberg'/
// 'generate_goldberg_config', ...) for a command name that didn't exist on
// the backend, so those buttons silently did nothing.
#[command]
#[allow(dead_code)]
async fn generate_goldberg_config(steam_api_key: String, app_id: String, output_dir: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::generate_goldberg_config(&steam_api_key, &app_id, &output_dir).await
}

#[command]
async fn apply_goldberg(game_folder: String, app_id: String, steam_api_key: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::apply_goldberg(game_folder, app_id, steam_api_key).await
}

#[command]
fn uninstall_goldberg(game_folder: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::restore_crack_backup(&game_folder)
}

// The commands below wrap more previously-implemented-but-never-registered
// RustUnlockerManager logic — same bug as Goldberg above, just affecting the
// rest of the Bypass tab (CreamAPI/SmokeAPI/Uplay unlockers, Steamless,
// HyperVisor bypass) instead. Every one of these buttons called a Tauri
// command name that didn't exist on the backend.
#[command]
fn detect_unlocker_type(game_path: String) -> String {
    crate::managers::unlockers::RustUnlockerManager::detect_unlocker_type(&game_path)
}

#[command]
async fn check_game_drm(app_id: String) -> String {
    crate::managers::unlockers::RustUnlockerManager::check_game_drm(&app_id).await
}

#[command]
async fn apply_creamapi(game_path: String, appid: String, dlcs: Vec<(String, String)>) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::apply_creamapi(&game_path, &appid, dlcs).await
}

#[command]
fn uninstall_creamapi(game_path: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::uninstall_creamapi(&game_path)
}

#[command]
async fn apply_smokeapi(game_path: String, appid: String, dlc_ids: Vec<String>) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::apply_smokeapi(&game_path, &appid, dlc_ids).await
}

#[command]
fn uninstall_smokeapi(game_path: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::uninstall_smokeapi(&game_path)
}

#[command]
async fn apply_uplay_unlocker(game_path: String, is_r2: bool) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::apply_uplay_unlocker(&game_path, is_r2).await
}

#[command]
fn uninstall_uplay_unlocker(game_path: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::uninstall_uplay_unlocker(&game_path)
}

#[command]
async fn apply_koaloader(game_path: String, appid: String, dlc_ids: Vec<String>) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::apply_koaloader(&game_path, &appid, dlc_ids).await
}

#[command]
fn uninstall_koaloader(game_path: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::uninstall_koaloader(&game_path)
}

#[command]
async fn run_steamless(exe_path: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::run_steamless(&exe_path).await
}

#[command]
#[allow(dead_code)]
async fn apply_hv_bypass(app_handle: tauri::AppHandle, game_path: String, href: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::apply_hv_bypass(Some(app_handle), &game_path, &href).await
}

#[command]
#[allow(dead_code)]
fn apply_hv_bypass_local(game_path: String, archive_path: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::apply_hv_bypass_local(&game_path, &archive_path)
}

#[command]
#[allow(dead_code)]
fn extract_local_archive(archive_path: String, dest_dir: String) -> Result<String, String> {
    crate::managers::unlockers::RustUnlockerManager::extract_local_archive(&archive_path, &dest_dir)
}

/// Sets (or replaces) a single `key = value` line inside a `[section]` block
/// of a simple TOML file, preserving everything else untouched. Not a
/// general TOML parser — deliberately scoped to opensteamtool.toml, whose
/// exact starting shape we control (we bundle it ourselves), so a targeted
/// line-based patch is safe here without pulling in a full TOML crate.
fn set_toml_key(content: &str, section: &str, key: &str, value: &str) -> String {
    let section_header = format!("[{}]", section);
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    let section_idx = lines.iter().position(|l| l.trim() == section_header);
    let key_line = format!("{} = {}", key, value);

    if let Some(start) = section_idx {
        // Section end = next line that starts a new [section], or EOF.
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map(|i| start + 1 + i)
            .unwrap_or(lines.len());

        let existing_key = lines[start + 1..end]
            .iter()
            .position(|l| l.trim_start().starts_with(&format!("{} =", key)) || l.trim_start().starts_with(&format!("{}=", key)));

        if let Some(rel_idx) = existing_key {
            lines[start + 1 + rel_idx] = key_line;
        } else {
            lines.insert(end, key_line);
        }
    } else {
        // Section doesn't exist yet — append a new one at the end.
        if !lines.is_empty() && !lines.last().map(|l| l.is_empty()).unwrap_or(true) {
            lines.push(String::new());
        }
        lines.push(section_header);
        lines.push(key_line);
    }

    lines.join("\n") + "\n"
}

#[cfg(test)]
mod manifest_mirror_tests {
    use super::looks_like_manifest;

    /// The real bytes at the head of a manifest served by the mirror
    /// (depot 3764201) — the check has to accept these.
    #[test]
    fn accepts_a_real_steam_manifest_header() {
        let head = [0xD0u8, 0x17, 0xF6, 0x71, 0x35, 0x16, 0x33, 0x00];
        assert!(looks_like_manifest(&head));
    }

    #[test]
    fn accepts_a_zipped_manifest() {
        assert!(looks_like_manifest(b"PKrest"));
    }

    /// The case this guard exists for: a dead or rate-limited mirror answers
    /// with an HTML error page, sometimes under a 200. Writing that into
    /// depotcache is worse than writing nothing.
    #[test]
    fn rejects_an_html_error_page() {
        assert!(!looks_like_manifest(b"<!DOCTYPE html><html>404</html>"));
        assert!(!looks_like_manifest(b"{\"message\":\"Not Found\"}"));
    }

    #[test]
    fn rejects_short_or_empty_bodies() {
        assert!(!looks_like_manifest(b""));
        assert!(!looks_like_manifest(&[0xD0, 0x17, 0xF6]));
    }
}

#[cfg(test)]
mod toml_patch_tests {
    use super::set_toml_key;

    const SAMPLE: &str = "[general]\n# OpenSteamTool Configuration\n\n[unlock]\nenabled = true\n\n[manifest]\n# Upstream API for depot manifest request codes.\n# Options: \"manifestdex\", \"opensteamtool\", \"steamrun\", \"wudrm\"\nurl = \"opensteamtool\"\n\n[stats]\n# Query https://stats.opensteamtool.com/{appid} for achievements when no Lua setStat is configured.\n# Priority: setStat > stats API > hardcoded preset SteamID\nenable_api = true\n\n[lua]\n# Additional Lua config directories (optional).\n# Files in these paths are loaded AFTER the default <Steam>/config/lua/ folder.\npaths = []\n\n[inject]\n# Optional DLL injection into game processes.\n# The injected library must match the target process architecture.\nenabled = false\n# library_x64 = \"OpenSteamTool.GameHook.x64.dll\"\n# library_x86 = \"OpenSteamTool.GameHook.x86.dll\"\n";

    #[test]
    fn adds_new_section_for_missing_remote() {
        let out = set_toml_key(SAMPLE, "remote", "url_template", "\"https://mirror.example/{channel}/{component}/{sha256}.toml\"");
        assert!(out.contains("[remote]\nurl_template = \"https://mirror.example/{channel}/{component}/{sha256}.toml\""));
        // Everything else must survive untouched.
        assert!(out.contains("[unlock]\nenabled = true"));
        assert!(out.contains("[inject]"));
    }

    #[test]
    fn replaces_existing_key_in_place() {
        let out = set_toml_key(SAMPLE, "inject", "enabled", "true");
        assert!(out.contains("[inject]\n# Optional DLL injection into game processes.\n# The injected library must match the target process architecture.\nenabled = true"));
        // Old "enabled = false" must be gone, not duplicated alongside the new line.
        assert_eq!(out.matches("enabled = ").count() - out.matches("enabled = true").count() - out.matches("enabled = false").count(), 0);
        assert!(!out.contains("enabled = false"));
    }

    #[test]
    fn inserts_new_key_into_existing_section_without_duplicating_others() {
        let out = set_toml_key(SAMPLE, "inject", "library_x64", "\"Hook.x64.dll\"");
        assert!(out.contains("library_x64 = \"Hook.x64.dll\""));
        assert!(out.contains("enabled = false")); // untouched
        assert_eq!(out.matches("[inject]").count(), 1);
    }

    #[test]
    fn round_trip_stable_when_key_already_set() {
        let once = set_toml_key(SAMPLE, "log", "level", "\"debug\"");
        let twice = set_toml_key(&once, "log", "level", "\"debug\"");
        assert_eq!(once, twice);
        assert_eq!(twice.matches("[log]").count(), 1);
    }
}

/// The services OpenSteamTool can ask for a depot's **manifest request code**
/// — the number Steam demands before it will accept a depot download.
///
/// Not the `.manifest` file itself: that one Ragnarok downloads into
/// `depotcache/` from its own mirrors. Both have to be there, which is why
/// switching this does nothing for a game whose manifest file is missing. What
/// it does fix is a code service that is down, slow, or blocked in the user's
/// country — `wudrm` is the one OpenSteamTool recommends inside China.
/// `manifestdex` (manifest.manifestdex.com) is a community-run alternative
/// that fixes the "sin conexión" Workshop error; it doesn't depend on GitHub
/// for version checks the way the stock OST binary does.
const MANIFEST_PROVIDERS: &[(&str, &str)] = &[
    ("manifestdex", "https://manifest.manifestdex.com/{gid}"),
];

/// `<Steam>/config/lua/manifest.lua`, when it exists, overrides the `[manifest]
/// url` setting entirely — OpenSteamTool calls its `fetch_manifest_code(gid)`
/// first.
const MANIFEST_LUA_MARKER: &str = "-- escrito por Ragnarok Launcher";

/// Kept to plain `http_get` calls, the only thing OpenSteamTool exposes to
/// these scripts, and returns the code as a string: these numbers go past 2^53,
/// so a Lua number would round them. ManifestDeX requires User-Agent: ManifestDeX/1.0
/// to bypass Cloudflare protection.
fn manifest_lua_body() -> String {
    format!(
        r#"{marker} — borra este archivo para volver a un solo servidor.
--
-- Pide el código de manifiesto a ManifestDeX (manifest.manifestdex.com).
-- OpenSteamTool llama a esta función antes de mirar la opción [manifest] url
-- de opensteamtool.toml.
-- Incluye User-Agent: ManifestDeX/1.0 requerido para evitar el bloqueo de Cloudflare.

function fetch_manifest_code(gid)
  local headers = {{["User-Agent"] = "ManifestDeX/1.0"}}
  local body, status = http_get("https://manifest.manifestdex.com/" .. gid, headers)
  if (status == 200 or not status) and body then
    local code = body:match("(%d+)")
    if code and #code > 5 then return code end
  end

  body, status = http_get("https://manifest.manifestdex.com/" .. gid)
  if (status == 200 or not status) and body then
    local code = body:match("(%d+)")
    if code and #code > 5 then return code end
  end

  return nil
end
"#,
        marker = MANIFEST_LUA_MARKER
    )
}

/// `<Steam>/config/lua/manifest.lua`.
fn manifest_lua_path(steam_path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(steam_path).join("config").join("lua").join("manifest.lua")
}

/// The value of `url` under `[manifest]`, or None when the section or key is
/// absent (OpenSteamTool then uses its own default).
fn manifest_provider_from_toml(content: &str) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[manifest]";
            continue;
        }
        if !in_section || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("url") {
            let value = rest.trim_start().strip_prefix('=')?.trim().trim_matches('"');
            return Some(value.to_string());
        }
    }
    None
}

/// What the manifest-code setting looks like on disk right now: `"auto"` when
/// our own chain script is in place, `"custom"` when someone else's
/// manifest.lua is (theirs wins, and overwriting it would be rude), otherwise
/// the service named in the TOML.
#[command]
fn get_manifest_provider(steam_path: String) -> Result<String, String> {
    let lua = manifest_lua_path(&steam_path);
    if lua.is_file() {
        let body = std::fs::read_to_string(&lua).unwrap_or_default();
        return Ok(if body.contains(MANIFEST_LUA_MARKER) { "auto" } else { "custom" }.to_string());
    }
    let toml_path = std::path::PathBuf::from(&steam_path).join("opensteamtool.toml");
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("No se pudo leer opensteamtool.toml: {}", e))?;
    Ok(manifest_provider_from_toml(&content).unwrap_or_else(|| "manifestdex".to_string()))
}

/// Puts the chain script in place, quietly, whenever it is safe to.
///
/// Returns what the state ended up being, for the card in Settings to show.
/// Never an error the UI needs to act on: the plugin may simply not be
/// installed yet.
#[command]
fn ensure_manifest_chain(steam_path: String) -> Result<String, String> {
    let lua = manifest_lua_path(&steam_path);
    if let Ok(body) = std::fs::read_to_string(&lua) {
        if !body.contains(MANIFEST_LUA_MARKER) {
            return Ok("custom".to_string()); // someone's own script — theirs wins
        }
        if body == manifest_lua_body() {
            return Ok("auto".to_string()); // already current
        }
    }

    let toml_path = std::path::PathBuf::from(&steam_path).join("opensteamtool.toml");
    let Ok(content) = std::fs::read_to_string(&toml_path) else {
        return Ok("sin-plugin".to_string());
    };
    let patched = set_toml_key(&content, "manifest", "url", "\"manifestdex\"");
    if patched != content {
        std::fs::write(&toml_path, patched).map_err(|e| e.to_string())?;
    }
    if let Some(parent) = lua.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&lua, manifest_lua_body()).map_err(|e| e.to_string())?;
    Ok("auto".to_string())
}

/// Points OpenSteamTool at one manifest-code service, or at the chain script
/// when `provider` is `"auto"`.
///
/// A manifest.lua this app did not write is never touched: it takes priority
/// over the TOML inside OpenSteamTool, so silently deleting it would undo
/// someone's own script, and silently leaving it would make this setting look
/// broken. The message says which happened.
#[command]
fn set_manifest_provider(steam_path: String, provider: String) -> Result<String, String> {
    let provider = provider.trim().to_lowercase();
    let known = provider == "auto" || MANIFEST_PROVIDERS.iter().any(|(name, _)| *name == provider);
    if !known {
        return Err(format!("Servidor de manifiestos desconocido: {}", provider));
    }

    let toml_path = std::path::PathBuf::from(&steam_path).join("opensteamtool.toml");
    let content = std::fs::read_to_string(&toml_path).map_err(|e| {
        format!(
            "No se encontró opensteamtool.toml en {} — instala el plugin primero. ({})",
            steam_path, e
        )
    })?;

    // "auto" still needs a value here: it is what OpenSteamTool falls back to
    // if the script is ever removed.
    let toml_value = if provider == "auto" { "manifestdex" } else { provider.as_str() };
    let patched = set_toml_key(&content, "manifest", "url", &format!("\"{}\"", toml_value));
    std::fs::write(&toml_path, patched).map_err(|e| e.to_string())?;

    let lua = manifest_lua_path(&steam_path);
    let existing = std::fs::read_to_string(&lua).ok();
    let is_foreign = existing.as_deref().is_some_and(|b| !b.contains(MANIFEST_LUA_MARKER));

    if provider == "auto" {
        if is_foreign {
            return Ok(format!(
                "Ya tienes tu propio manifest.lua en config/lua/, y ese manda sobre esta opción. \
                 Se guardó {} como respaldo, pero tu script sigue decidiendo.",
                toml_value
            ));
        }
        if let Some(parent) = lua.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&lua, manifest_lua_body()).map_err(|e| e.to_string())?;
        return Ok("✔ Servidor de manifiestos configurado en ManifestDeX. Reinicia Steam para aplicarlo.".to_string());
    }

    if is_foreign {
        return Ok(format!(
            "Se guardó {}, pero tu propio manifest.lua en config/lua/ manda sobre esta opción. \
             Bórralo si quieres que se use el servidor elegido.",
            toml_value
        ));
    }
    if existing.is_some() {
        std::fs::remove_file(&lua).map_err(|e| e.to_string())?;
    }
    Ok(format!("✔ Servidor de manifiestos: {}. Reinicia Steam para aplicarlo.", toml_value))
}

#[cfg(test)]
mod manifest_provider_tests {
    use super::*;

    const SAMPLE: &str = "[unlock]\nenabled = true\n\n[manifest]\n# Options: \"manifestdex\"\nurl = \"manifestdex\"\n\n[stats]\nenable_api = true\n";

    #[test]
    fn reads_the_configured_service_and_ignores_the_comment_above_it() {
        assert_eq!(manifest_provider_from_toml(SAMPLE).as_deref(), Some("manifestdex"));
    }

    #[test]
    fn a_url_key_in_another_section_is_not_the_manifest_one() {
        let other = "[remote]\nurl = \"https://mirror.example/{gid}\"\n\n[manifest]\nurl = \"wudrm\"\n";
        assert_eq!(manifest_provider_from_toml(other).as_deref(), Some("wudrm"));
        assert_eq!(manifest_provider_from_toml("[remote]\nurl = \"x\"\n"), None);
    }

    #[test]
    fn switching_service_rewrites_only_that_line() {
        let out = set_toml_key(SAMPLE, "manifest", "url", "\"wudrm\"");
        assert_eq!(manifest_provider_from_toml(&out).as_deref(), Some("wudrm"));
        assert!(out.contains("[unlock]\nenabled = true"), "el resto queda igual");
        assert!(out.contains("enable_api = true"));
        assert_eq!(out.matches("url = ").count(), 1, "sin duplicar la clave");
    }

    /// The chain has to name every service, and hand the code back as text:
    /// these ids run past 2^53 and a Lua number would round them.
    #[test]
    fn the_chain_script_is_recognisable_and_covers_all_services() {
        let body = manifest_lua_body();
        assert!(body.contains(MANIFEST_LUA_MARKER));
        assert!(body.contains("function fetch_manifest_code(gid)"));
        for (_, url) in MANIFEST_PROVIDERS {
            let host = url.split('/').nth(2).unwrap();
            assert!(body.contains(host), "falta {host}");
        }
    }
}

/// Writes mirror/injection/logging settings into every `opensteamtool.toml`
/// this app manages (the one it bundles at Steam's root). See
/// github.com/OpenSteam001/OpenSteamTool#usage's "Using a different mirror",
/// "Injection", and "Debug logging" sections — these aren't features Ragnarok
/// implements itself, they're config knobs for the OpenSteamTool.dll plugin
/// already installed via "Instalar Plugin".
#[command]
#[allow(dead_code)]
fn update_opensteamtool_settings(
    steam_path: String,
    mirror_url: Option<String>,
    inject_enabled: bool,
    inject_library_x64: Option<String>,
    inject_library_x86: Option<String>,
    log_level: Option<String>,
) -> Result<String, String> {
    let toml_path = std::path::PathBuf::from(&steam_path).join("opensteamtool.toml");
    let mut content = std::fs::read_to_string(&toml_path).map_err(|e| {
        format!(
            "No se encontró opensteamtool.toml en {} — instala el plugin primero. ({})",
            steam_path, e
        )
    })?;

    if let Some(url) = mirror_url.filter(|u| !u.trim().is_empty()) {
        content = set_toml_key(&content, "remote", "url_template", &format!("\"{}\"", url.trim()));
    }

    content = set_toml_key(&content, "inject", "enabled", if inject_enabled { "true" } else { "false" });
    if inject_enabled {
        if let Some(lib) = inject_library_x64.filter(|s| !s.trim().is_empty()) {
            content = set_toml_key(&content, "inject", "library_x64", &format!("\"{}\"", lib.trim()));
        }
        if let Some(lib) = inject_library_x86.filter(|s| !s.trim().is_empty()) {
            content = set_toml_key(&content, "inject", "library_x86", &format!("\"{}\"", lib.trim()));
        }
    }

    if let Some(level) = log_level.filter(|l| !l.trim().is_empty()) {
        content = set_toml_key(&content, "log", "level", &format!("\"{}\"", level.trim()));
    }

    std::fs::write(&toml_path, content).map_err(|e| e.to_string())?;
    Ok("✔ Configuración de OpenSteamTool guardada.".to_string())
}

/// Opens `<Steam>/opensteamtool/` (per-module debug logs — main.log, ipc.log,
/// manifest.log, etc.) in the system file explorer. Only debug builds of
/// OpenSteamTool.dll actually populate this folder — the bundled DLL may or
/// may not be one, so this can legitimately open an empty folder.
#[command]
fn open_opensteamtool_logs(steam_path: String) -> Result<String, String> {
    let logs_dir = std::path::PathBuf::from(&steam_path).join("opensteamtool");
    std::fs::create_dir_all(&logs_dir).map_err(|e| e.to_string())?;
    opener::open(&logs_dir).map_err(|e| e.to_string())?;
    Ok(format!("Carpeta abierta: {}", logs_dir.display()))
}

/// Opens Ragnarok's own diagnostics log in the user's editor.
///
/// The counterpart to routing every `eprintln!` through `diag_log!`: a log
/// nobody can reach is only marginally better than one nobody can read.
#[command]
fn open_diagnostics_log() -> Result<String, String> {
    let path = managers::diag::log_path().ok_or("No se pudo resolver la carpeta de datos.")?;
    let dir = path.parent().ok_or("Ruta de registro inválida.")?.to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if !path.exists() {
        std::fs::write(&path, "").map_err(|e| e.to_string())?;
    }

    // The folder, not just ragnarok.log. Three separate files matter when
    // something goes wrong and they are written by different parts of the
    // app: ragnarok.log (this one), achievement_watcher.log (the background
    // unlock watcher, which keeps its own) and panic_log.txt (the Rust panic
    // message, the single most useful line there is when the app died). A
    // button that opens only the first hides the other two.
    opener::open(&dir).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().to_string())
}

#[command]
async fn install_manual_lua(steam_path: String, archive_path: String) -> Result<ManualLuaResult, String> {
    let lower = archive_path.to_lowercase();
    if !lower.ends_with(".zip") && !lower.ends_with(".rar") {
        return Err("Selecciona un archivo .zip o .rar.".to_string());
    }

    let steam_path_clone = steam_path.clone();
    let (lua_count, manifest_count, app_ids) = tokio::task::spawn_blocking(move || -> Result<(u32, u32, Vec<String>), String> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let temp_dir = std::env::temp_dir().join(format!("ragnarok_manual_lua_{}", unique));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

        crate::managers::unlockers::RustUnlockerManager::extract_local_archive(
            &archive_path,
            &temp_dir.to_string_lossy(),
        )?;

        let lua_dir = std::path::PathBuf::from(&steam_path_clone).join("config").join("lua");
        let depotcache_dir = std::path::PathBuf::from(&steam_path_clone).join("depotcache");
        std::fs::create_dir_all(&lua_dir).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&depotcache_dir).map_err(|e| e.to_string())?;

        let mut lua_count = 0u32;
        let mut manifest_count = 0u32;
        // Figure out which game(s) this pack is for, so we can register it in
        // the library/achievements even when Ryuu's catalog doesn't cover it.
        // Ryuu/SteamTools ticket Luas are named after the base game's AppID
        // (e.g. "1868140.lua"), and their FIRST `addappid(N)` call is that
        // same base AppID — so we detect via the filename first, and fall
        // back to parsing `addappid(...)` from the content for packs whose
        // Lua isn't numerically named.
        let mut app_ids: Vec<String> = Vec::new();
        let mut push_id = |id: String, app_ids: &mut Vec<String>| {
            if !app_ids.contains(&id) {
                app_ids.push(id);
            }
        };
        for entry in walkdir::WalkDir::new(&temp_dir).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let name_lower = name.to_lowercase();
            if name_lower.ends_with(".lua") {
                let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
                if std::fs::copy(entry.path(), lua_dir.join(&name)).is_ok() {
                    lua_count += 1;
                }
                let stem = name.trim_end_matches(".lua").trim_end_matches(".Lua").trim_end_matches(".LUA");
                if !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()) {
                    push_id(stem.to_string(), &mut app_ids);
                } else {
                    // First addappid(...) argument = base game AppID.
                    //
                    // The old form searched `content.to_lowercase()` and then
                    // sliced `content` at the index it found. Lowercasing does
                    // not preserve byte length across all of UTF-8 — 'İ' grows
                    // from two bytes to three, 'ẞ' shrinks from three to two —
                    // so a `.lua` with any such character in a header comment
                    // shifted every offset after it. Best case that parsed the
                    // wrong AppID and registered the game under someone else's
                    // id; worst case the slice landed mid-character and panicked.
                    //
                    // Scanning line by line for a case-insensitive prefix needs
                    // no offsets at all.
                    let found = content.lines().find_map(|line| {
                        let line = line.trim_start();
                        let rest = strip_prefix_ignore_ascii_case(line, "addappid(")?;
                        let digits: String =
                            rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                        (!digits.is_empty()).then_some(digits)
                    });
                    if let Some(digits) = found {
                        push_id(digits, &mut app_ids);
                    }
                }
            } else if name_lower.ends_with(".manifest") {
                if std::fs::copy(entry.path(), depotcache_dir.join(&name)).is_ok() {
                    manifest_count += 1;
                }
            }
        }

        let _ = std::fs::remove_dir_all(&temp_dir);

        if lua_count == 0 {
            return Err("No se encontró ningún archivo .lua dentro del archivo comprimido.".to_string());
        }

        Ok((lua_count, manifest_count, app_ids))
    })
    .await
    .map_err(|e| e.to_string())??;

    // Register each detected game the same way the normal download flow does
    // (Ragnarok sidecar + fake ACF marking it installed) and look up its real
    // name/header image so the frontend can add it to the Library/Achievements
    // game list even when it isn't in Ryuu's catalog at all.
    let games = register_manual_games(&steam_path, &app_ids).await;

    Ok(ManualLuaResult {
        message: format!(
            "Instalado: {} archivo(s) .lua, {} manifest(s). OpenSteamTool recarga el cambio solo — no hace falta reiniciar Steam.",
            lua_count, manifest_count
        ),
        games,
    })
}

// ── Config export/import (migrate to another PC) ───────────────────────────
// Bundles the frontend's localStorage (settings, caches, API keys — passed in
// as an opaque JSON blob since only the frontend knows its own keys) together
// with the manually-added Lua license files (from "Agregar Lua Manualmente")
// and the list of catalog-installed games into a single .zip. Ryuu/SteamCMD-
// sourced Lua/manifest files themselves are deliberately NOT bundled — they
// get regenerated the moment each game is reinstalled from the catalog on
// the new PC, which the frontend now does automatically for every id in
// catalog_games right after import, so the user never has to hunt each one
// down and click "Instalar" by hand.
#[derive(Serialize, Deserialize, Clone)]
struct CatalogGameRef {
    app_id: String,
    name: String,
}

#[derive(Serialize)]
struct ImportConfigResult {
    settings_json: String,
    catalog_games: Vec<CatalogGameRef>,
}

#[command]
async fn export_ragnarok_config(
    steam_path: String,
    manual_app_ids: Vec<String>,
    catalog_games: Vec<CatalogGameRef>,
    settings_json: String,
    save_path: String,
) -> Result<(), String> {
    use std::io::Write;
    use zip::write::FileOptions;

    let file = std::fs::File::create(&save_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("settings.json", options).map_err(|e| e.to_string())?;
    zip.write_all(settings_json.as_bytes()).map_err(|e| e.to_string())?;

    // Every .lua in the folder, not just the ones the caller listed.
    //
    // The old version bundled only `manual_app_ids` and left everything else
    // to be reinstalled from the catalog on the new PC, using the app id list
    // in `ragnarok_apps.json`. That registry drifts from what is actually on
    // disk: a .lua written by the static-versions flow, by an older build, or
    // by hand never gets recorded in it. Measured on a real machine while
    // chasing this: 12 .lua files present, 11 entries in the registry.
    //
    // A user hit the same thing far worse — 90 games before the backup, 10
    // after restoring it. The eighty that vanished were the ones the registry
    // had never heard of.
    //
    // Reading the folder makes the backup describe reality. Carrying the
    // files rather than a list of things to re-download also means a restore
    // no longer depends on the catalog, the network, or a manifest mirror
    // still being alive — and a .lua is a couple of kilobytes.
    let lua_dir = std::path::PathBuf::from(&steam_path).join("config").join("lua");
    let mut bundled: Vec<String> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&lua_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.to_lowercase().ends_with(".lua") {
                continue;
            }
            if let Ok(content) = std::fs::read(&path) {
                zip.start_file(format!("lua/{}", name), options).map_err(|e| e.to_string())?;
                zip.write_all(&content).map_err(|e| e.to_string())?;
                bundled.push(name.to_string());
            }
        }
    }

    // Belt and braces: anything the caller named that the folder scan missed.
    for app_id in &manual_app_ids {
        let name = format!("{}.lua", app_id);
        if bundled.iter().any(|b| b.eq_ignore_ascii_case(&name)) {
            continue;
        }
        if let Ok(content) = std::fs::read(lua_dir.join(&name)) {
            zip.start_file(format!("lua/{}", name), options).map_err(|e| e.to_string())?;
            zip.write_all(&content).map_err(|e| e.to_string())?;
        }
    }

    // The registry itself travels too, so the new PC knows which games it
    // manages instead of rebuilding that knowledge from nothing.
    let map_path = apps_map_path(&steam_path);
    if let Ok(content) = std::fs::read(&map_path) {
        zip.start_file("ragnarok_apps.json", options).map_err(|e| e.to_string())?;
        zip.write_all(&content).map_err(|e| e.to_string())?;
    }

    let catalog_json = serde_json::to_string(&catalog_games).map_err(|e| e.to_string())?;
    zip.start_file("catalog_games.json", options).map_err(|e| e.to_string())?;
    zip.write_all(catalog_json.as_bytes()).map_err(|e| e.to_string())?;

    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// Extracts a previously-exported config bundle: restores any bundled
/// manually-added Lua files into config/lua/, and returns the embedded
/// settings.json contents plus the catalog game list as-is — the frontend
/// restores settings into localStorage itself (it owns interpreting its own
/// keys) and loops `install_game` over catalog_games right after.
#[command]
async fn import_ragnarok_config(steam_path: String, zip_path: String) -> Result<ImportConfigResult, String> {
    use std::io::Read;

    // Same check install_game does before writing any Lua file: OpenSteamTool.dll
    // is the DLL-hijack that makes Steam actually read config/lua/*.lua — without
    // it, the restored files would sit there doing nothing and the manually-added
    // games from the backup would silently never show up, with no indication why.
    let plugin_marker = std::path::PathBuf::from(&steam_path).join("OpenSteamTool.dll");
    if !plugin_marker.exists() {
        return Err(
            "El plugin de Steam todavía no está instalado en esta PC. Andá a Inicio y presioná \"Instalar Plugin\" una vez — después podés volver a importar tu configuración.".to_string()
        );
    }

    let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    let lua_dir = std::path::PathBuf::from(&steam_path).join("config").join("lua");
    std::fs::create_dir_all(&lua_dir).map_err(|e| e.to_string())?;

    let mut settings_json = String::new();
    let mut catalog_games: Vec<CatalogGameRef> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if name == "settings.json" {
            entry.read_to_string(&mut settings_json).map_err(|e| e.to_string())?;
        } else if name == "catalog_games.json" {
            let mut raw = String::new();
            entry.read_to_string(&mut raw).map_err(|e| e.to_string())?;
            // Absent in backups made before this field existed — an empty
            // list just means the frontend has nothing to auto-install,
            // same as before this feature, not an import failure.
            catalog_games = serde_json::from_str(&raw).unwrap_or_default();
        } else if name == "ragnarok_apps.json" {
            // Restored beside the .lua files it describes. Written only when
            // the backup carried one, so importing an older backup does not
            // wipe whatever this PC already knows.
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            if !buf.is_empty() {
                let _ = std::fs::write(lua_dir.join("ragnarok_apps.json"), buf);
            }
        } else if let Some(fname) = name.strip_prefix("lua/") {
            // Take only the file name, never the path the archive claims.
            //
            // `entry.name()` is attacker-controlled text: an entry called
            // `lua/../../Steam.exe` resolved straight out of config/lua and
            // overwrote whatever it pointed at. This flow exists so people can
            // move their setup between PCs by passing a .zip around on
            // Discord, which is exactly the path a malicious one arrives by.
            //
            // `download_plugin_update` in this same file already guards this
            // with `mangled_name()`, and explains why in its own comment — the
            // protection just was not carried over here. These backups are flat
            // lists of `<appid>.lua`, so flattening to the file name loses
            // nothing real.
            let safe = std::path::Path::new(fname)
                .file_name()
                .map(std::path::PathBuf::from);
            if let Some(safe) = safe.filter(|f| !f.as_os_str().is_empty()) {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                std::fs::write(lua_dir.join(safe), buf).map_err(|e| e.to_string())?;
            }
        }
    }

    if settings_json.is_empty() {
        return Err("El archivo no contiene una configuración válida de Ragnarok Launcher.".to_string());
    }
    Ok(ImportConfigResult { settings_json, catalog_games })
}

/// Where a plugin downloaded from the repo is unpacked, and the marker
/// recording which version is installed.
///
/// The plugin ships inside the app (`resources/steam_plugin/`), which means a
/// new Steam build — and the new pattern/IPC signatures it needs — could only
/// reach users through a whole new release of Ragnarok. Publishing
/// `steam_plugin.zip` in the games repo turns that into a download.
fn plugin_update_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("steam_plugin_update"))
}

/// The publish date of the plugin this machine has already seen.
///
/// A date rather than a content hash because that is what the user is being
/// told — "there is a newer plugin, from this date" — and because it is what
/// they can check against the repo themselves.
fn plugin_version_marker() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("steam_plugin_seen.txt"))
}

/// The last commit that touched the published plugin — this is what carries
/// its date. The contents API gives a size and a hash but no date at all.
const PLUGIN_ZIP_API: &str =
    "https://api.github.com/repos/RagnarokManifests/games/commits?path=steam_plugin.zip&per_page=1";
const PLUGIN_ZIP_RAW: &str =
    "https://raw.githubusercontent.com/RagnarokManifests/games/main/steam_plugin.zip";

#[derive(serde::Serialize)]
struct PluginUpdateInfo {
    available: bool,
    /// ISO date the plugin was last published, e.g. `2026-07-15T04:19:43Z`.
    published_at: String,
    /// The date already seen here. Empty on a machine that has never been
    /// offered one.
    seen_at: String,
}

/// True for archive paths that belong to the user rather than to the plugin.
///
/// Compared with forward slashes and lowercased so a zip built on Windows and
/// one built on Linux are treated the same.
fn is_user_data_path(rel: &Path) -> bool {
    let normalised = rel.to_string_lossy().replace('\\', "/").to_lowercase();
    normalised.starts_with("config/lua/") || normalised.starts_with("config/plugin/lua/")
}

/// Asks GitHub whether the published plugin differs from the installed one.
///
/// One request, and it reads only metadata: the contents API returns the blob
/// hash and size without transferring the 1.1 MB zip.
#[command]
async fn check_plugin_update() -> Result<PluginUpdateInfo, String> {
    let client = make_client()?;
    let res = client
        .get(PLUGIN_ZIP_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("No se pudo consultar GitHub: {}", e))?;

    // Checked, like every other GitHub call here: a rate-limited 403 carries
    // valid JSON, and reading a sha out of it would report "no hay
    // actualización" when the truth is "no se pudo preguntar".
    if !res.status().is_success() {
        return Err(format!(
            "GitHub respondió {} al consultar el plugin (puede ser límite de peticiones).",
            res.status()
        ));
    }

    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let published_at = json[0]["commit"]["committer"]["date"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if published_at.is_empty() {
        return Err("GitHub no devolvió la fecha del plugin.".to_string());
    }

    let seen_at = plugin_version_marker()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // First check on this machine: adopt what is published right now as the
    // baseline and say nothing.
    //
    // The alternative — treating "never seen" as "there is an update" — would
    // announce a zip that has been sitting in the repo for a month as news,
    // to every existing user at once, on the first launch after this feature
    // ships. The notice is meant to mean "Sheaker just published something",
    // and it only means that if the starting point is today.
    if seen_at.is_empty() {
        let _ = dismiss_plugin_update(published_at.clone()).await;
        return Ok(PluginUpdateInfo {
            available: false,
            published_at,
            seen_at: String::new(),
        });
    }

    Ok(PluginUpdateInfo {
        // Any change of date counts, including one that moves backwards: a
        // republished older plugin is still a plugin the user has not been
        // shown.
        available: seen_at != published_at,
        published_at,
        seen_at,
    })
}

/// Records a published date as already seen, without installing anything.
///
/// Backs the modal's "later" option. Without it the same notice would return
/// on every launch, which is nagging rather than informing — the point is to
/// speak up when the file changes, once.
#[command]
async fn dismiss_plugin_update(published_at: String) -> Result<(), String> {
    let marker = plugin_version_marker().ok_or("No se pudo resolver el directorio de datos")?;
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&marker, published_at.trim()).map_err(|e| e.to_string())
}

/// Downloads and unpacks the published plugin, ready for `install_plugin` to
/// write into Steam.
///
/// Deliberately does not touch Steam itself: installing means closing Steam,
/// replacing DLLs that may be locked, and retrying — all of which
/// `install_plugin` already does properly. This only stages the files.
#[command]
async fn download_plugin_update(app_handle: tauri::AppHandle) -> Result<String, String> {
    let emit = |msg: &str| { app_handle.emit_all("plugin_log", msg.to_string()).ok(); };

    let dir = plugin_update_dir().ok_or("No se pudo resolver el directorio de datos")?;
    emit("Descargando el plugin publicado...");

    let client = make_client()?;
    if !download_guard::is_allowed_url(PLUGIN_ZIP_RAW, GITHUB_HOSTS) {
        return Err("La URL del plugin no apunta a GitHub.".to_string());
    }
    let res = client
        .get(PLUGIN_ZIP_RAW)
        .send()
        .await
        .map_err(|e| format!("No se pudo descargar el plugin: {}", e))?;
    if !res.status().is_success() {
        return Err(format!("La descarga del plugin devolvió {}.", res.status()));
    }
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;

    // A 404 page or a truncated transfer would otherwise be unpacked as if it
    // were the plugin, and the failure would surface later as a broken Steam.
    download_guard::verify_download("el plugin", &bytes, download_guard::Payload::Zip, None)?;

    // Replaced wholesale rather than merged: a stale file from a previous
    // version left behind would be installed alongside the new ones.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    emit("Extrayendo...");
    let cursor = std::io::Cursor::new(bytes.as_ref());
    let mut zip = zip::ZipArchive::new(cursor)
        .map_err(|e| format!("No se pudo abrir el ZIP del plugin: {}", e))?;

    let mut written = 0usize;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        if file.name().ends_with('/') {
            continue;
        }
        // mangled_name() strips leading slashes and ".." components, so a
        // crafted archive cannot write outside this folder.
        let rel = file.mangled_name();
        if rel.as_os_str().is_empty() {
            continue;
        }

        // Never install over the user's own data. `config/lua/` inside Steam
        // is where the installed games' .lua files live, SteamTools.lua among
        // them — and the published zip does contain a copy of it. Writing
        // that would wipe the user's entire game list, which is the worst
        // thing an "update the plugin" button could do.
        if is_user_data_path(&rel) {
            emit(&format!("Omitido (datos del usuario): {}", rel.display()));
            continue;
        }
        let out = dir.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut sink = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut file, &mut sink).map_err(|e| e.to_string())?;
        written += 1;
    }

    if written == 0 {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("El ZIP del plugin venía vacío.".to_string());
    }

    emit(&format!("{} archivo(s) descargados.", written));
    Ok(format!(
        "Plugin descargado ({} archivos). Instalalo con \"Instalar/Reparar Plugin\".",
        written
    ))
}

#[command]
async fn install_plugin(app_handle: tauri::AppHandle, steam_path: Option<String>) -> Result<String, String> {
    let emit = |msg: &str| { app_handle.emit_all("plugin_log", msg.to_string()).ok(); };

    // ── 1. Locate Steam ────────────────────────────────────────────────────────
    // Prefer the path the user explicitly picked (e.g. when the app detected
    // more than one Steam install and asked which one to use) over the
    // registry auto-detection, which only ever remembers the most recently
    // launched install.
    emit("Buscando Steam...");
    let steam_path = match steam_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => p.replace('/', "\\"),
        None => {
            #[cfg(windows)]
            {
                use winreg::enums::HKEY_CURRENT_USER;
                use winreg::RegKey;
                RegKey::predef(HKEY_CURRENT_USER)
                    .open_subkey("Software\\Valve\\Steam")
                    .and_then(|k| k.get_value::<String, _>("SteamPath"))
                    .map(|p| p.replace('/', "\\"))
                    .unwrap_or_else(|_| r"C:\Program Files (x86)\Steam".to_string())
            }
            #[cfg(not(windows))]
            { "/usr/games/steam".to_string() }
        }
    };
    emit(&format!("Steam encontrado en: {}", steam_path));

    // ── 2. Stop Steam and every helper process/service holding file locks ──────
    emit("Cerrando Steam y procesos asociados...");
    #[cfg(windows)]
    {
        crate::managers::steam::RustSteamManager::kill_all_steam_processes();
    }
    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

    // ── 3. Read the plugin files bundled with the app ───────────────────────────
    // Previously downloaded a zip from GitHub on every install — now the
    // files ship inside the app itself (resources/steam_plugin/), so this
    // works offline and can't fail because of a flaky connection or a moved
    // GitHub file.
    emit("Leyendo archivos del plugin...");
    let steam_dir = std::path::PathBuf::from(&steam_path);
    const PLUGIN_FILES: &[&str] = &[
        "OpenSteamTool.dll",
        "dwmapi.dll",
        "dwmapi.exp",
        "dwmapi.lib",
        "lua_static.lib",
        "opensteamtool.toml",
        "xinput1_4.dll",
        "xinput1_4.exp",
        "xinput1_4.lib",
        // Pattern/IPC signature files OpenSteamTool needs to hook the
        // current Steam client build — keyed by that build's own SHA-256
        // hash, checked by check_opensteamtool_ipc_gap() below. Without
        // these, every Steam update makes users see "Unsupported Steam
        // Version" until upstream (OpenSteam001) publishes new signatures
        // AND the user manually drops them in — bundling the latest known
        // set here means "Instalar/Reparar Plugin" does it automatically.
        "opensteamtool/pattern/steamclient/3eda3b0a618c07037c4ef7496c6d69769e90519baf4c1c08daf79786448273e9.toml",
        "opensteamtool/pattern/steamclient/86112382982fa855086f566b2fb8343290e798849029337dcfeecba0d5051b5e.toml",
        "opensteamtool/pattern/steamui/af6ca9193dd6d502fad83d4a51ea29fe156699c2dfcf5739f9f70ca659d2b83d.toml",
        "opensteamtool/pattern/steamui/f35533b57f5b4c8a7c1325be26cc63ec2ba43a998c3c2bf81a1701498ba99335.toml",
        "opensteamtool/ipc/steamclient/3eda3b0a618c07037c4ef7496c6d69769e90519baf4c1c08daf79786448273e9.toml",
        "opensteamtool/ipc/steamclient/86112382982fa855086f566b2fb8343290e798849029337dcfeecba0d5051b5e.toml",
    ];

    // A plugin downloaded from the repo wins over the bundled one, and its
    // own contents decide what gets installed rather than PLUGIN_FILES.
    //
    // That distinction is the whole point of the feature: what changes most
    // often is the set of pattern/IPC signature files, one per Steam client
    // build. A hardcoded list can only ever install the signatures that
    // existed when the app was released, which is exactly the situation
    // downloading a new plugin is meant to escape.
    // Held back for 2.1.3 along with the update modal: the plugin published
    // in the repo is missing the `opensteamtool/` signature files and carries
    // a copy of `config/lua/SteamTools.lua`, so installing it would leave
    // users worse off than the bundled one. `None` here means the bundled
    // files are always used.
    let downloaded: Option<PathBuf> = None;
    let mut entries: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();

    match &downloaded {
        Some(dir) => {
            emit("Usando el plugin descargado del repositorio.");
            for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
                if !entry.file_type().is_file() {
                    continue;
                }
                let Ok(rel) = entry.path().strip_prefix(dir) else { continue };
                let buf = std::fs::read(entry.path())
                    .map_err(|e| format!("Error leyendo {}: {}", rel.display(), e))?;
                entries.push((rel.to_path_buf(), buf));
            }
            if entries.is_empty() {
                return Err(
                    "La carpeta del plugin descargado está vacía. Descargalo de nuevo.".to_string(),
                );
            }
        }
        None => {
            entries.reserve(PLUGIN_FILES.len());
            for filename in PLUGIN_FILES {
                let resource_path = find_resource(&format!("steam_plugin/{}", filename))
                    .ok_or_else(|| format!("No se encontró {} entre los recursos de la app.", filename))?;
                let buf = std::fs::read(&resource_path).map_err(|e| format!("Error leyendo {}: {}", filename, e))?;
                entries.push((std::path::PathBuf::from(filename), buf));
            }
        }
    }
    emit(&format!("{} archivo(s) listos para instalar.", entries.len()));

    // ── 4. Write everything to the Steam directory ──────────────────────────────
    for (name, buf) in &entries {
        let out_path = steam_dir.join(name);

        // Never overwrite the user's own settings file.
        //
        // `opensteamtool.toml` is in PLUGIN_FILES, so "Reparar Plugin" replaced
        // it with the bundled default — silently wiping the mirror, injection
        // and log-level choices the user had made through
        // update_opensteamtool_settings, while reporting "Plugin instalado
        // correctamente". A repair is for restoring what broke, not for
        // resetting what someone configured. Only written when absent, which is
        // exactly the case a fresh install needs.
        let is_user_config = name
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("opensteamtool.toml"))
            .unwrap_or(false);
        if is_user_config && out_path.exists() {
            emit("  · opensteamtool.toml ya existe — se conserva tu configuración.");
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Move the existing file aside instead of deleting it.
        //
        // This used to clear the read-only bit and then `remove_file`, with the
        // write happening afterwards — so if all twenty attempts below failed,
        // the function returned an error having already destroyed a DLL that
        // was working a second ago. The comment on the retry loop names the
        // exact scenario: a user with real-time antivirus hitting
        // OpenSteamTool.dll, the very first file written. Losing the hijack DLL
        // makes every one of their games vanish from Steam, and the message
        // they get tells them to disable their antivirus and try again — with
        // nothing left to fall back to.
        //
        // Renaming keeps a copy on the same volume. It is restored below if
        // every write attempt fails, and removed once one succeeds.
        let backup_path = out_path.with_extension("ragnarok-old");
        let mut restored_from = None;
        if out_path.exists() {
            if let Ok(mut perms) = std::fs::metadata(&out_path).map(|m| m.permissions()) {
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(&out_path, perms);
            }
            let _ = std::fs::remove_file(&backup_path);
            match std::fs::rename(&out_path, &backup_path) {
                Ok(()) => restored_from = Some(backup_path.clone()),
                // If it cannot even be moved it is locked hard; fall back to
                // the old behaviour so the retry loop still gets its chance.
                Err(_) => {
                    let _ = std::fs::remove_file(&out_path);
                }
            }
        }

        // Retry a fair number of times: a helper process (e.g. GameOverlayUI.exe,
        // or steamservice.exe if "net stop" silently failed for lack of admin
        // rights) can hold the file handle open well after taskkill /F returns —
        // 10 attempts over ~14s still wasn't always enough in practice
        // (reported again: a real user still hit "archivo en uso" on
        // OpenSteamTool.dll, the very first file written). Give it more real
        // margin, and re-issue the kill every few attempts (not just once)
        // in case something keeps respawning (e.g. Steam's own crash-recovery
        // relaunching steamservice faster than a single re-kill catches).
        let mut last_err = String::new();
        let mut written = false;
        for attempt in 0..20 {
            if attempt > 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            }
            if attempt > 0 && attempt % 4 == 0 {
                emit(&format!("  {} en uso, reintentando y cerrando procesos de Steam de nuevo...", name.display()));
                #[cfg(windows)]
                crate::managers::steam::RustSteamManager::kill_all_steam_processes();
            }
            match std::fs::write(&out_path, buf) {
                Ok(()) => { written = true; break; }
                Err(e) => last_err = e.to_string(),
            }
        }
        if !written {
            // Put back what was there before giving up, so a failed repair
            // leaves the user exactly as they started rather than worse.
            let mut restored = false;
            if let Some(backup) = &restored_from {
                restored = std::fs::rename(backup, &out_path).is_ok();
            }
            return Err(format!(
                "Error escribiendo {} (archivo en uso) después de varios intentos: {}. {}Cerrá Steam por completo (incluida la bandeja del sistema); un antivirus con protección en tiempo real también puede estar bloqueando el archivo.",
                name.display(),
                last_err,
                if restored { "El archivo anterior quedó intacto. " } else { "" }
            ));
        }
        if let Some(backup) = &restored_from {
            let _ = std::fs::remove_file(backup);
        }
        emit(&format!("  ✔ {}", name.display()));
    }

    // ── 4.5. Check for fresher OpenSteamTool signatures live ────────────────────
    // The bundled pattern/ipc files above match whatever Steam build was
    // current when this app version was released — Steam updates far more
    // often than Ragnarok does, so by the time someone hits this the bundled
    // hash may already be stale again. This fills the gap without a new
    // Ragnarok release for every single Steam update.
    emit("Buscando firmas actualizadas de OpenSteamTool...");
    let fetched_signatures = update_opensteamtool_signatures_from_upstream(&steam_dir).await;
    if !fetched_signatures.is_empty() {
        emit(&format!("  ✔ {} firma(s) nueva(s) de OpenSteamTool descargada(s)", fetched_signatures.len()));
    }

    // ── 5. Restart Steam ───────────────────────────────────────────────────────
    // Step 2 killed Steam unconditionally, so this is the step that decides
    // whether the user ends up with a working Steam or a closed one. It used
    // to spawn and discard the result, and the function reported "Steam
    // reiniciado" regardless — including when Steam.exe wasn't where we
    // looked, when the spawn failed outright, and when Steam started and
    // immediately exited. From the user's side that reads as "Ragnarok closed
    // my Steam and told me it reopened it", which is exactly the wrong thing
    // to be wrong about.
    emit("Reiniciando Steam...");
    let steam_exe = steam_dir.join("Steam.exe");
    let mut steam_restarted = false;
    let mut restart_problem: Option<String> = None;

    if !steam_exe.exists() {
        restart_problem = Some(format!("no se encontró {}", steam_exe.display()));
    } else {
        #[cfg(windows)]
        {
            match std::process::Command::new(&steam_exe).spawn() {
                Ok(_) => {
                    // Steam's launcher process forks and exits, so a
                    // successful spawn proves nothing on its own. Wait for
                    // steam.exe to actually be present before claiming it.
                    for _ in 0..15 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(700)).await;
                        if crate::managers::steam::RustSteamManager::steam_is_running() {
                            steam_restarted = true;
                            break;
                        }
                    }
                    if !steam_restarted {
                        restart_problem = Some("Steam no llegó a abrirse".to_string());
                    }
                }
                Err(e) => restart_problem = Some(format!("no se pudo abrir Steam ({})", e)),
            }
        }
        #[cfg(not(windows))]
        {
            restart_problem = Some("reinicio automático de Steam sólo soportado en Windows".to_string());
        }
    }

    if steam_restarted {
        emit("  ✔ Steam abierto de nuevo");
    } else if let Some(reason) = &restart_problem {
        emit(&format!("  ⚠ Steam quedó cerrado: {}", reason));
    }

    // Recorded only now, after the files are actually in Steam. Writing it at
    // download time would mark the update as applied even if the install
    // failed, and the user would never be offered it again.
    if downloaded.is_some() {
        if let Ok(info) = check_plugin_update().await {
            let _ = dismiss_plugin_update(info.published_at).await;
        }
    }

    emit("✔ Plugin instalado correctamente");
    if steam_restarted {
        Ok("Plugin instalado y Steam reiniciado".to_string())
    } else {
        // The plugin itself is in place — that part did succeed, and saying
        // otherwise would send the user back to re-run a repair they don't
        // need. What changed is that the second half of the sentence is now
        // true.
        Ok(format!(
            "Plugin instalado. Steam quedó cerrado ({}) — abrilo a mano.",
            restart_problem.as_deref().unwrap_or("motivo desconocido")
        ))
    }
}

/// Lowercase hex SHA-256 of a file's contents, matching the hash format
/// OpenSteamTool itself shows in its "IPC spec missing" dialog and uses to
/// name the compatibility file it looks for.
fn sha256_hex_file(path: &std::path::Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).ok()?;
    let hash = Sha256::digest(&bytes);
    Some(hash.iter().map(|b| format!("{:02x}", b)).collect())
}

/// Checks whether OpenSteamTool already has working pattern/IPC signatures
/// for the Steam build currently on disk (by hash), and for any that are
/// missing, tries to download them live from OpenSteam001/steam-monitor —
/// the community project that actually reverse-engineers and publishes a new
/// per-build signature (branches "pattern" and "ipc", as
/// `<branch>/steamclient|steamui/<sha256-of-dll>.toml`) every time Steam
/// updates. This is what keeps "Instalar/Reparar Plugin" working across
/// future Steam updates without a new Ragnarok release each time — the
/// bundled files bootstrap a fresh install, this keeps them current
/// afterward. Best-effort throughout: a 404 just means upstream hasn't
/// published support for this exact build yet (matches what
/// check_opensteamtool_ipc_gap already tells the user), and a network
/// failure here shouldn't fail plugin install/repair over a nice-to-have.
// (branch, subdir, dll filename to hash) — shared between the upstream
// fetcher below and update_opensteamtool_signatures' own "are we fully
// covered now" check.
const OPENSTEAMTOOL_SIGNATURE_TARGETS: &[(&str, &str, &str)] = &[
    ("pattern", "steamclient", "steamclient64.dll"),
    ("pattern", "steamui", "steamui.dll"),
    ("ipc", "steamclient", "steamclient64.dll"),
];

async fn update_opensteamtool_signatures_from_upstream(steam_dir: &Path) -> Vec<String> {
    let Ok(client) = crate::make_client() else { return Vec::new() };
    let mut fetched = Vec::new();

    for (branch, subdir, dll_name) in OPENSTEAMTOOL_SIGNATURE_TARGETS {
        let dll_path = steam_dir.join(dll_name);
        let Some(hash) = sha256_hex_file(&dll_path) else { continue };
        let out_path = steam_dir
            .join("opensteamtool")
            .join(branch)
            .join(subdir)
            .join(format!("{}.toml", hash));
        if out_path.exists() {
            continue; // Already covered — bundled with the app or fetched previously.
        }
        let url = format!(
            "https://raw.githubusercontent.com/OpenSteam001/steam-monitor/{}/{}/{}.toml",
            branch, subdir, hash
        );
        let res = match client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !res.status().is_success() {
            continue;
        }
        let Ok(bytes) = res.bytes().await else { continue };
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&out_path, &bytes).is_ok() {
            fetched.push(format!("{}/{}/{}.toml", branch, subdir, hash));
        }
    }
    fetched
}

/// After a Repair Plugin run restarts Steam, checks whether OpenSteamTool
/// actually has a working IPC compatibility file for the Steam build that
/// just started — the exact same file its own "IPC spec missing" dialog
/// says it's missing (`<Steam>\opensteamtool\ipc\steamclient\<hash>.toml`,
/// hash = SHA-256 of steamclient64.dll). Reported by a user who kept
/// hitting that dialog right after Steam auto-updated and assumed Repair
/// Plugin should fix it — it can't: reinstalling the exact same
/// OpenSteamTool.dll doesn't change whether upstream has published support
/// for today's new Steam build yet. Detecting this lets us say so honestly
/// instead of claiming "reparación completa" for something that isn't
/// actually fixed.
fn check_opensteamtool_ipc_gap(steam_path: &str) -> Option<String> {
    let steam_dir = std::path::PathBuf::from(steam_path);
    let steamclient = steam_dir.join("steamclient64.dll");
    let hash = sha256_hex_file(&steamclient)?;
    let spec_path = steam_dir.join("opensteamtool").join("ipc").join("steamclient").join(format!("{}.toml", hash));
    if spec_path.exists() {
        None
    } else {
        Some(
            "Steam se actualizó a una versión nueva que OpenSteamTool todavía no tiene soporte para (falta el archivo de compatibilidad IPC para este build — verás \"IPC spec missing\" al abrir Steam). \
            Esto NO se arregla reinstalando el plugin: los archivos ya están bien, lo que falta es que el proyecto OpenSteamTool publique soporte para esta versión de Steam, algo que suele tardar unas horas después de cada actualización de Steam. \
            Mientras tanto, los hooks basados en patrones siguen funcionando (según el propio aviso de OpenSteamTool) — solo se pierde la interceptación IPC de esta sesión. \
            Podés revisar el estado en https://github.com/OpenSteam001/steam-monitor".to_string()
        )
    }
}

/// Lightweight, dedicated button for just the signature check — unlike
/// "Reparar Plugin" this doesn't kill Steam, rewrite every plugin file, or
/// clear caches; it only checks the 3 pattern/IPC files against Steam's
/// current build and fetches whatever's missing from upstream. Steam still
/// needs a restart afterward to actually pick up a newly-installed file
/// (it only reads these at startup), which the returned message says.
#[command]
#[allow(dead_code)]
async fn update_opensteamtool_signatures(steam_path: Option<String>) -> Result<String, String> {
    let steam_path = match steam_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => p.replace('/', "\\"),
        None => {
            #[cfg(windows)]
            {
                use winreg::enums::HKEY_CURRENT_USER;
                use winreg::RegKey;
                RegKey::predef(HKEY_CURRENT_USER)
                    .open_subkey("Software\\Valve\\Steam")
                    .and_then(|k| k.get_value::<String, _>("SteamPath"))
                    .map(|p| p.replace('/', "\\"))
                    .unwrap_or_else(|_| r"C:\Program Files (x86)\Steam".to_string())
            }
            #[cfg(not(windows))]
            { r"C:\Program Files (x86)\Steam".to_string() }
        }
    };
    let steam_dir = std::path::PathBuf::from(&steam_path);
    let fetched = update_opensteamtool_signatures_from_upstream(&steam_dir).await;

    let all_covered = OPENSTEAMTOOL_SIGNATURE_TARGETS.iter().all(|(branch, subdir, dll_name)| {
        let dll_path = steam_dir.join(dll_name);
        match sha256_hex_file(&dll_path) {
            Some(hash) => steam_dir.join("opensteamtool").join(branch).join(subdir).join(format!("{}.toml", hash)).exists(),
            None => true, // DLL missing entirely — not something this check can fix, don't block on it.
        }
    });

    if !fetched.is_empty() {
        Ok(format!(
            "✔ {} firma(s) nueva(s) de OpenSteamTool instalada(s). Reiniciá Steam para que se apliquen.",
            fetched.len()
        ))
    } else if all_covered {
        Ok("Ya tenías las firmas más recientes de OpenSteamTool instaladas.".to_string())
    } else {
        Err(
            "OpenSteamTool todavía no publicó firmas para esta build de Steam. Suele tardar unas horas después de cada actualización — probá de nuevo más tarde.".to_string()
        )
    }
}

#[command]
async fn repair_steam_plugin(app_handle: tauri::AppHandle, steam_path: Option<String>) -> Result<String, String> {
    let emit = |msg: &str| { app_handle.emit_all("plugin_log", msg.to_string()).ok(); };
    emit("Cerrando Steam y procesos asociados para liberar archivos...");

    #[cfg(windows)]
    {
        crate::managers::steam::RustSteamManager::kill_all_steam_processes();
    }

    // Wait for file locks to clear (helper processes can take a moment to
    // release DLL handles even after taskkill /F returns)
    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

    // Clean up .old leftover files in Steam dir. Same override rule as
    // install_plugin: prefer the user-picked path (relevant when the PC has
    // more than one Steam install) over the registry auto-detection.
    let steam_path = match steam_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => p.replace('/', "\\"),
        None => {
            #[cfg(windows)]
            {
                use winreg::enums::HKEY_CURRENT_USER;
                use winreg::RegKey;
                RegKey::predef(HKEY_CURRENT_USER)
                    .open_subkey("Software\\Valve\\Steam")
                    .and_then(|k| k.get_value::<String, _>("SteamPath"))
                    .map(|p| p.replace('/', "\\"))
                    .unwrap_or_else(|_| r"C:\Program Files (x86)\Steam".to_string())
            }
            #[cfg(not(windows))]
            { r"C:\Program Files (x86)\Steam".to_string() }
        }
    };

    emit("Eliminando archivos de bloqueo y cachés corruptas...");
    let steam_dir = std::path::PathBuf::from(&steam_path);
    if let Ok(entries) = std::fs::read_dir(&steam_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("old") {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    // Stale lock/crash files that can prevent Steam from starting cleanly,
    // e.g. after the PC was powered off without Steam shutting down properly.
    //
    // `steam.cfg` used to be in this list and is not one of them: it is a file
    // the user writes by hand, usually `BootStrapperInhibitAll=enable`, to stop
    // Steam updating itself — often precisely so the plugin keeps working.
    // Deleting it let Steam update on next launch and produced the very
    // "IPC spec missing" breakage check_opensteamtool_ipc_gap exists to
    // diagnose, so the repair button caused the fault it was pressed to fix.
    // `crash_handler.exe` is likewise a binary Steam ships, not a leftover.
    for f in [".crash", "steam.pid", "steamui.dll.bak"] {
        let fp = steam_dir.join(f);
        if fp.exists() {
            let _ = std::fs::remove_file(&fp);
        }
    }
    // Corrupted caches that commonly cause Steam to hang on launch.
    //
    // `appcache/stats` is spared: it holds UserGameStatsSchema_<appid>.bin,
    // which is where this launcher reads achievement names and descriptions
    // from. Wiping it left every game's Achievements tab saying "open this
    // game's achievement page in Steam at least once" — with no visible
    // connection to the repair button just pressed, and no way back for a user
    // without a Steam Web API key.
    let stats_dir = steam_dir.join("appcache").join("stats");
    let stats_backup = std::env::temp_dir().join(format!("ragnarok_stats_{}", std::process::id()));
    let stats_saved = stats_dir.is_dir() && std::fs::rename(&stats_dir, &stats_backup).is_ok();

    for d in ["appcache", "package", "config/htmlcache"] {
        let dp = steam_dir.join(d);
        if dp.exists() {
            let _ = std::fs::remove_dir_all(&dp);
        }
    }

    if stats_saved {
        let _ = std::fs::create_dir_all(steam_dir.join("appcache"));
        if std::fs::rename(&stats_backup, &stats_dir).is_err() {
            emit("Aviso: no se pudieron devolver los esquemas de logros a appcache/stats.");
        }
    }

    // Re-run install_plugin to restore all files from the ZIP, passing along
    // the already-resolved path so it doesn't re-detect (and potentially
    // pick a different Steam install) via the registry a second time.
    let install_result = install_plugin(app_handle.clone(), Some(steam_path.clone())).await?;

    // Give Steam a few seconds to actually come back up before checking
    // whether OpenSteamTool has a working IPC spec for this build — right
    // after install_plugin spawns Steam.exe, Steam hasn't even started
    // reading its own client DLLs yet, so checking immediately would always
    // look like a gap even when there isn't one.
    tokio::time::sleep(tokio::time::Duration::from_secs(6)).await;
    if let Some(gap_message) = check_opensteamtool_ipc_gap(&steam_path) {
        emit(&gap_message);
        return Ok(gap_message);
    }

    Ok(install_result)
}

#[command]
async fn update_dlcs(
    steam_path: String,
    app_id: String,
    dlc_ids: Vec<String>,
) -> Result<String, String> {
    let mut map = load_apps_map(&steam_path);
    map.insert(app_id.clone(), dlc_ids);
    save_apps_map(&steam_path, &map);
    rebuild_steam_tools(&steam_path)
        .map_err(|e| format!("SteamTools update failed: {}", e))?;
    Ok(format!("DLCs updated for app {}", app_id))
}

/// The DLCs saved for a game, so the DLC manager opens showing what is active
/// instead of ticking every DLC Steam lists.
#[command]
async fn get_saved_dlcs(steam_path: String, app_id: String) -> Result<Vec<String>, String> {
    Ok(load_apps_map(&steam_path).get(&app_id).cloned().unwrap_or_default())
}

#[command]
async fn restart_steam() -> Result<String, String> {
    // restart_steam() calls std::thread::sleep internally; run it on a
    // blocking-pool thread so it doesn't stall a tokio worker thread (and
    // whatever else — downloads, image preloads — happens to share it) for
    // the full ~1s wait.
    tokio::task::spawn_blocking(RustSteamManager::restart_steam)
        .await
        .map_err(|e| e.to_string())?
        .map(|_| "Steam is restarting...".to_string())
}

// Called right after install_game for a single app. No Steam kill/restart
// here: at install time the game usually hasn't been downloaded/played yet,
// so there's typically nothing to clean up — this just becomes a no-op
// (caught and ignored by the caller) in that case.
/// Puts back the remote caches an earlier restore renamed and abandoned.
/// Returns the app ids repaired.
#[command]
#[allow(dead_code)]
fn repair_cloud_caches(steam_path: String) -> Vec<String> {
    cloud_saves::restore_cloud_caches(&steam_path)
}

#[command]
async fn disable_steam_cloud_for_app(steam_path: String, app_id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || cloud_saves::apply_cloud_fix_batch(&steam_path, &[app_id]))
        .await
        .map_err(|e| e.to_string())?
}

// ── Achievements (local, GSE/Goldberg-backed) ───────────────────────────────
//
// These games have no real Steam ownership, so there is no legitimate online
// achievement progress to read from Valve's servers. What actually drives
// what the Steam overlay/library shows for a cracked game is the local GSE
// (Goldberg SteamEmu) emulator's save file — Steam's own UI for these games
// is just rendering whatever that local JSON says. So "unlocking" an
// achievement here means writing to that same file: the *next* time the game
// is launched (or the overlay is opened), Steam shows it as unlocked because
// the emulated Steamworks API reports it that way. Nothing is faked upstream
// on Valve's servers — this only affects the local, already-unofficial state.
//
// Achievement metadata (name/description/icon) comes from Steam's public
// ISteamUserStats/GetSchemaForGame Web API, using the same user-supplied key
// already used by generate_goldberg_config (rl_steam_api_key in the Tools
// tab) — that call needs a key too, so we reuse it instead of asking twice.

#[derive(Serialize, Deserialize, Clone)]
struct AchievementInfo {
    name: String,
    display_name: String,
    description: String,
    hidden: bool,
    icon: String,
    icongray: String,
    earned: bool,
    earned_time: u64,
    global_percent: Option<f64>,
}

/// Locates (without creating) the GSE save file for an app's achievements.
/// Goldberg forks have used two different folder names over time, so both
/// are checked. Returns the first one that already exists.
/// Achievements a Uplay-emulated game has unlocked.
///
/// Ubisoft titles do not go through the Steam emulator at all: they run on an
/// emulated Uplay, which keeps its own file under
/// `%APPDATA%\Goldberg UplayEmu Saves\<uplay_appid>\achievements.json`. Two
/// things make it awkward to read alongside the Steam one, and both are
/// handled here rather than by asking the user for anything:
///
/// * The folder is named after the **Uplay** app id (66088 for Black Flag
///   Resynced), which has no relation to the Steam one this app works in. So
///   instead of mapping ids, every folder is read and entries are kept only
///   when their achievement name appears in this game's own schema — names
///   like `ACObsidian_Ach_24` are specific enough that a false match is not a
///   real risk.
/// * Its shape differs: the file is an object keyed by position, each value
///   carrying its own `name`, and `earned` is `1` rather than `true`.
fn load_uplay_emu_achievements(known: &[String]) -> std::collections::HashMap<String, (bool, u64)> {
    let mut out = std::collections::HashMap::new();
    let Some(data_dir) = dirs::data_dir() else { return out };
    let root = data_dir.join("Goldberg UplayEmu Saves");
    let Ok(entries) = std::fs::read_dir(&root) else { return out };

    for entry in entries.flatten() {
        let file = entry.path().join("achievements.json");
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let Some(obj) = json.as_object() else { continue };

        for (_, v) in obj {
            let Some(name) = v["name"].as_str() else { continue };
            if !known.iter().any(|k| k == name) {
                continue;
            }
            // `earned` is written as 1/0 here, not true/false.
            let earned = v["earned"].as_bool().unwrap_or_else(|| {
                v["earned"].as_u64().map(|n| n != 0).unwrap_or(false)
            });
            if earned {
                let when = v["earned_time"].as_u64().unwrap_or(0);
                out.insert(name.to_string(), (true, when));
            }
        }
    }
    out
}

fn find_local_achievements_file(app_id: &str) -> Option<std::path::PathBuf> {
    let data_dir = dirs::data_dir()?;
    for folder in ["GSE Saves", "Goldberg SteamEmu Saves"] {
        let candidate = data_dir.join(folder).join(app_id).join("achievements.json");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Same lookup, but returns a path to write to even if nothing exists yet
/// (defaults to the "GSE Saves" folder name, the current gbe_fork default).
fn local_achievements_file_for_write(app_id: &str) -> Result<std::path::PathBuf, String> {
    if let Some(existing) = find_local_achievements_file(app_id) {
        return Ok(existing);
    }
    let data_dir = dirs::data_dir().ok_or("No se pudo resolver %APPDATA%")?;
    Ok(data_dir.join("GSE Saves").join(app_id).join("achievements.json"))
}

fn load_local_achievement_state(steam_path: &str, app_id: &str) -> std::collections::HashMap<String, (bool, u64)> {
    let mut result = std::collections::HashMap::new();
    if let Some(path) = find_local_achievements_file(app_id) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(obj) = json.as_object() {
                    for (name, v) in obj {
                        let earned = v["earned"].as_bool().unwrap_or(false);
                        let earned_time = v["earned_time"].as_u64().unwrap_or(0);
                        result.insert(name.clone(), (earned, earned_time));
                    }
                }
            }
        }
    }

    // Fallback source for games that never go through Ragnarok's live-Steam
    // ownership sync at all (e.g. pure DRM-free cracks added via Bypass) —
    // scans local achievement files from ~8 different crack tools (CODEX,
    // RUNE, OnlineFix, RLD, CreamAPI, SKIDROW, EMPRESS, Razor1911). Only
    // fills in names not already covered above (Goldberg/GSE stays
    // authoritative where both exist, matching the priority already used by
    // the caller: live Steam state > this local state).
    for (name, state) in crate::managers::crackers::load_all_cracker_achievements(steam_path, app_id) {
        result.entry(name).or_insert(state);
    }

    // Ubisoft games keep theirs somewhere else entirely; the schema is what
    // says which names belong to this game.
    let known: Vec<String> = load_local_achievement_schema(steam_path, app_id)
        .or_else(|| load_cached_schema(app_id))
        .map(|s| s.into_iter().map(|a| a.name).collect())
        .unwrap_or_default();
    if !known.is_empty() {
        for (name, state) in load_uplay_emu_achievements(&known) {
            // An unlock recorded anywhere counts; only fill gaps so a source
            // already consulted stays authoritative.
            result.entry(name).or_insert(state);
        }
    }

    result
}

#[cfg(test)]
mod uplay_achievements_tests {
    use super::load_uplay_emu_achievements;

    /// The real shape the Uplay emulator writes, taken from a live file:
    /// keyed by position, `name` inside, `earned` as a number.
    #[test]
    fn reads_the_numeric_earned_flag() {
        let dir = dirs::data_dir().unwrap().join("Goldberg UplayEmu Saves").join("rl_test_66088");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("achievements.json"),
            r#"{
              "23": { "name": "RL_Test_Ach_24", "earned": 1, "earned_time": 1787815603 },
              "24": { "name": "RL_Test_Ach_25", "earned": 0, "earned_time": 0 },
              "25": { "name": "RL_Other_Game_Ach", "earned": 1, "earned_time": 99 }
            }"#,
        )
        .unwrap();

        let known = vec!["RL_Test_Ach_24".to_string(), "RL_Test_Ach_25".to_string()];
        let got = load_uplay_emu_achievements(&known);

        assert_eq!(got.get("RL_Test_Ach_24"), Some(&(true, 1787815603)));
        assert!(!got.contains_key("RL_Test_Ach_25"), "un logro sin ganar no se reporta");
        assert!(
            !got.contains_key("RL_Other_Game_Ach"),
            "un nombre ajeno al esquema no debe colarse"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Achievement *definitions* only (name/description/icon/hidden) — these
/// almost never change for a released game, unlike earned/unlocked state
/// (which always comes fresh from Steam's live client, never from this
/// cache). Cached so opening a game's achievements repeatedly doesn't hit
/// the Steam Web API every single time, and so it still works without an
/// API key once a game has been loaded once.
#[derive(Serialize, Deserialize, Clone)]
struct AchievementSchemaEntry {
    name: String,
    display_name: String,
    description: String,
    hidden: bool,
    icon: String,
    icongray: String,
    // What % of all Steam players have earned this — same data Steam's own
    // achievement page shows. Public endpoint, no API key needed.
    #[serde(default)]
    global_percent: Option<f64>,
}

fn schema_cache_path(app_id: &str) -> Option<std::path::PathBuf> {
    Some(
        dirs::data_dir()?
            .join("com.ragnarok.launcher")
            .join("achievement_schema_cache")
            .join(format!("{}.json", app_id)),
    )
}

fn load_cached_schema(app_id: &str) -> Option<Vec<AchievementSchemaEntry>> {
    let path = schema_cache_path(app_id)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cached_schema(app_id: &str, entries: &[AchievementSchemaEntry]) {
    if let Some(path) = schema_cache_path(app_id) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(entries) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// Public endpoint, no key required — best-effort, returns empty on any
/// failure (network hiccup, rate limit, etc.) rather than erroring, since
/// this is supplementary rarity data and must never block the achievements
/// list itself from showing.
async fn fetch_global_percents(
    client: &reqwest::Client,
    app_id: &str,
) -> std::collections::HashMap<String, f64> {
    let percent_url = format!(
        "https://api.steampowered.com/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v2/?gameid={}",
        app_id
    );
    if let Ok(resp) = client.get(&percent_url).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            if let Some(arr) = json["achievementpercentages"]["achievements"].as_array() {
                return arr
                    .iter()
                    .filter_map(|a| {
                        let name = a["name"].as_str()?.to_string();
                        let pct = a["percent"].as_f64()?;
                        Some((name, pct))
                    })
                    .collect();
            }
        }
    }
    std::collections::HashMap::new()
}

/// Fallback achievement schema source for users who never set up a Steam
/// Web API Key: Steam's own client caches every game's achievement/stat
/// schema locally at `<steam>\appcache\stats\UserGameStatsSchema_<appid>.bin`
/// the first time it fetches stats for that game — reading it directly
/// needs no key and no network call at all. Same technique
/// github.com/gibbed/SteamAchievementManager uses (see managers::vdf's
/// module comment for the verified format). Returns None if the file
/// doesn't exist yet (game's stats/achievements page was never opened in
/// Steam itself) or contains no achievement stats.
fn load_local_achievement_schema(steam_path: &str, app_id: &str) -> Option<Vec<AchievementSchemaEntry>> {
    let path = Path::new(steam_path)
        .join("appcache")
        .join("stats")
        .join(format!("UserGameStatsSchema_{}.bin", app_id));
    if !path.exists() {
        return None;
    }

    let root = crate::managers::vdf::load(&path).ok()?;
    let app_node = root.get(app_id)?;
    let stats = app_node.get("stats")?;

    let mut out = Vec::new();
    for (_, stat) in stats.children() {
        let type_str = stat.get("type").and_then(|t| t.as_string());
        let is_achievements = match type_str.as_deref() {
            Some(s) => {
                let s = s.to_ascii_lowercase();
                s == "achievements" || s == "groupachievements"
            }
            None => {
                let raw = stat
                    .get("type_int")
                    .and_then(|t| t.as_string())
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);
                raw == 4 || raw == 5
            }
        };
        if !is_achievements {
            continue;
        }

        for (bits_key, bits) in stat.children() {
            if !bits_key.eq_ignore_ascii_case("bits") {
                continue;
            }
            for (_, bit) in bits.children() {
                let Some(name) = bit.get("name").and_then(|n| n.as_string()).filter(|s| !s.is_empty()) else { continue };
                let display = bit.get("display");
                let display_name = display
                    .and_then(|d| d.get("name"))
                    .and_then(|n| n.localized("spanish"))
                    .unwrap_or_else(|| name.clone());
                let description = display
                    .and_then(|d| d.get("desc"))
                    .and_then(|n| n.localized("spanish"))
                    .unwrap_or_default();
                let hidden = display.and_then(|d| d.get("hidden")).map(|n| n.as_bool()).unwrap_or(false);
                // The local schema only stores an icon filename token, not a
                // full URL (SAM itself resolves these through the live
                // client's GetAchievementIcon+image APIs) — Valve's own
                // public GetSchemaForGame Web API serves the *same* tokens
                // as full URLs under this exact CDN path, so reuse that.
                let icon_url = |field: &str| -> String {
                    display
                        .and_then(|d| d.get(field))
                        .and_then(|n| n.as_string())
                        .filter(|s| !s.is_empty())
                        .map(|token| format!("https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{}/{}", app_id, token))
                        .unwrap_or_default()
                };

                out.push(AchievementSchemaEntry {
                    name,
                    display_name,
                    description,
                    hidden,
                    icon: icon_url("icon"),
                    icongray: icon_url("icon_gray"),
                    global_percent: None,
                });
            }
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[command]
async fn get_game_achievements(
    steam_path: String,
    app_id: String,
    api_key: String,
    force_refresh: bool,
) -> Result<Vec<AchievementInfo>, String> {
    let cached = if force_refresh { None } else { load_cached_schema(&app_id) };

    let schema = if let Some(mut cached) = cached {
        // A cache written before rarity (%) tracking existed, or one where
        // that one-off fetch failed transiently, would otherwise show every
        // card as the same flat "unknown" tier forever — self-heal it here
        // instead of requiring the user to notice and hit "Actualizar".
        let needs_percent_backfill = !cached.is_empty() && cached.iter().all(|a| a.global_percent.is_none());
        if needs_percent_backfill {
            if let Ok(client) = make_client() {
                let percents = fetch_global_percents(&client, &app_id).await;
                if !percents.is_empty() {
                    for entry in cached.iter_mut() {
                        entry.global_percent = percents.get(&entry.name).copied();
                    }
                    save_cached_schema(&app_id, &cached);
                }
            }
        }
        cached
    } else if api_key.trim().is_empty() {
        // No API key configured — fall back to Steam's own locally-cached
        // achievement schema instead of failing outright. Only reachable
        // when no key is set at all; a key that's present but invalid/fails
        // still goes through the Web API path below unchanged.
        let steam_path = steam_path.clone();
        let app_id_clone = app_id.clone();
        let local = tokio::task::spawn_blocking(move || load_local_achievement_schema(&steam_path, &app_id_clone))
            .await
            .unwrap_or(None);

        match local {
            Some(entries) => {
                save_cached_schema(&app_id, &entries);
                entries
            }
            None => {
                return Err("No hay Steam Web API Key configurada (Herramientas) y Steam todavía no tiene el schema de logros de este juego guardado localmente — abrí la página de logros de este juego en el propio cliente de Steam al menos una vez, o configurá una API Key.".to_string());
            }
        }
    } else {
        let client = make_client()?;
        let schema_url = format!(
            "https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?key={}&appid={}&l=spanish",
            api_key, app_id
        );

        let json: serde_json::Value = client
            .get(&schema_url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        let achievements_arr = json["game"]["availableGameStats"]["achievements"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        if achievements_arr.is_empty() {
            return Err("Este juego no tiene logros de Steam, o la API Key no es válida.".to_string());
        }

        let mut fetched: Vec<AchievementSchemaEntry> = achievements_arr
            .iter()
            .filter_map(|ach| {
                let name = ach["name"].as_str()?.to_string();
                Some(AchievementSchemaEntry {
                    display_name: ach["displayName"].as_str().unwrap_or(&name).to_string(),
                    description: ach["description"].as_str().unwrap_or("").to_string(),
                    hidden: ach["hidden"].as_i64().unwrap_or(0) == 1,
                    icon: ach["icon"].as_str().unwrap_or("").to_string(),
                    icongray: ach["icongray"].as_str().unwrap_or("").to_string(),
                    global_percent: None,
                    name,
                })
            })
            .collect();

        let percents = fetch_global_percents(&client, &app_id).await;
        for entry in fetched.iter_mut() {
            entry.global_percent = percents.get(&entry.name).copied();
        }

        save_cached_schema(&app_id, &fetched);
        fetched
    };

    let local_state = load_local_achievement_state(&steam_path, &app_id);

    // Best-effort: ask the REAL running Steam client for actual current
    // state (covers achievements earned by playing normally, or unlocked
    // through Steam itself — not just what we previously wrote). Silently
    // falls back to the local (Goldberg) state above if Steam isn't open or
    // the game's steam_api(64).dll can't be found — this must never block
    // the achievements list from showing.
    let names: Vec<String> = schema.iter().map(|a| a.name.clone()).collect();
    let steam_state = {
        let steam_path = steam_path.clone();
        let app_id = app_id.clone();
        tokio::task::spawn_blocking(move || read_steam_achievement_state(&steam_path, &app_id, &names))
            .await
            .unwrap_or_default()
    };

    let mut result = Vec::new();
    for ach in schema {
        let name = ach.name;
        // steam_state.or_else(local_state) used to mean: ANY entry from the
        // live Steam read (even a correctly-parsed "not earned") completely
        // shadowed our own local record, regardless of what it said. That
        // silently made unlocks look permanently lost whenever Steam's own
        // client state lagged behind (or never reflected) a StoreStats() this
        // app itself just performed — the program's own saved record should
        // be sticky proof that WE unlocked it, not something Steam's live
        // read can unilaterally override back to locked.
        let steam_entry = steam_state.get(&name).copied();
        let local_entry = local_state.get(&name).copied();
        let (earned, earned_time) = match (steam_entry, local_entry) {
            (Some((true, t)), _) => (true, t),
            (_, Some((true, t))) => (true, t),
            (Some((false, _)), _) => (false, 0),
            (None, Some((false, _))) => (false, 0),
            (None, None) => (false, 0),
        };
        result.push(AchievementInfo {
            display_name: ach.display_name,
            description: ach.description,
            hidden: ach.hidden,
            icon: ach.icon,
            icongray: ach.icongray,
            global_percent: ach.global_percent,
            name,
            earned,
            earned_time,
        });
    }

    // Snapshot this fresh, live-confirmed state locally so it survives the
    // game being uninstalled later — see persist_achievement_snapshot.
    persist_achievement_snapshot(&app_id, &result);

    Ok(result)
}

#[derive(Serialize)]
struct AchievementHistorySummary {
    app_id: String,
    total: usize,
    earned: usize,
}

/// Every game with a cached achievement schema — i.e. every game whose
/// Achievements page has ever actually been opened in Ragnarok — paired
/// with how many of its achievements are earned according to the durable
/// local record (see persist_achievement_snapshot / write_local_achievement).
/// Used by the read-only "achievement history" browser, which — unlike the
/// interactive Achievements tab — deliberately does NOT require the game to
/// currently be installed, so completion (e.g. 100%) stays visible after
/// uninstalling. Never touches the live Steam client at all, so it carries
/// none of the "connecting for this AppID makes Steam's library show Play"
/// risk that gated the interactive tab to real-installs-only.
/// Off the main thread, deliberately.
///
/// A `#[command] fn` that is not `async` runs on Tauri's main thread, and
/// this one does, per cached game: a JSON schema read, a scan of eight
/// different crack tools' achievement files (dozens of `exists()` probes
/// each), a `read_dir` of Steam's userdata, and — through
/// `exe_relative_paths` — resolving the game's install folder and reading it
/// to find the main .exe. Times however many games the user has ever opened
/// this tab for, twice per visit. That is the freeze: not one slow thing, a
/// few hundred small ones with the UI thread held the whole time.
///
/// spawn_blocking does not make the work smaller, it makes it not the UI's
/// problem. The memo inside `exe_relative_paths` is what makes the second
/// visit cheap.
#[command]
async fn list_achievement_history(steam_path: String) -> Result<Vec<AchievementHistorySummary>, String> {
    tokio::task::spawn_blocking(move || list_achievement_history_blocking(&steam_path))
        .await
        .map_err(|e| e.to_string())?
}

fn list_achievement_history_blocking(steam_path: &str) -> Result<Vec<AchievementHistorySummary>, String> {
    let dir = dirs::data_dir()
        .ok_or("No se pudo resolver %APPDATA%")?
        .join("com.ragnarok.launcher")
        .join("achievement_schema_cache");

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let app_id = match file_name.strip_suffix(".json") {
            Some(id) => id.to_string(),
            None => continue,
        };
        let schema = match load_cached_schema(&app_id) {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let local_state = load_local_achievement_state(steam_path, &app_id);
        let earned = schema.iter().filter(|a| local_state.get(&a.name).map(|(e, _)| *e).unwrap_or(false)).count();
        out.push(AchievementHistorySummary { app_id, total: schema.len(), earned });
    }
    Ok(out)
}

/// True if this directory is the one the emulator reads its settings from.
fn holds_steam_api_dll(dir: &Path) -> bool {
    dir.join("steam_api64.dll").is_file() || dir.join("steam_api.dll").is_file()
}

/// The handful of places the dll actually lives, checked directly.
///
/// Worth doing before any walk because the deepest of them is also the most
/// common one to miss: Unreal buries it six levels down, at
/// `Engine\Binaries\ThirdParty\Steamworks\Steamv151\Win64`. That version
/// folder is numbered per engine release (Steamv151, Steamv153, ...) so it is
/// expanded at runtime rather than written out.
fn known_steam_api_locations(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![
        root.to_path_buf(),
        root.join("bin64"),
        root.join("bin"),
        root.join("Binaries").join("Win64"),
        root.join("Binaries").join("Win32"),
        root.join("Win64"),
    ];

    // Unreal's per-project layout: <Game>\Binaries\Win64.
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten().take(64) {
            let dir = entry.path();
            if dir.is_dir() {
                out.push(dir.join("Binaries").join("Win64"));
            }
        }
    }

    // Unreal's bundled Steamworks, whose version folder varies.
    let steamworks = root
        .join("Engine")
        .join("Binaries")
        .join("ThirdParty")
        .join("Steamworks");
    if let Ok(entries) = std::fs::read_dir(&steamworks) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if dir.is_dir() {
                out.push(dir.join("Win64"));
                out.push(dir.join("Win32"));
            }
        }
    }

    out
}

/// Finds the folder the emulator reads its settings from: the one holding the
/// game's steam_api dll.
///
/// A directory that already has `steam_settings` wins outright — that is the
/// copy the game is really loading, and on a game shipping several copies of
/// the dll it is the only one worth writing to.
fn find_emulator_settings_dir(game_path: &str) -> Option<PathBuf> {
    let root = Path::new(game_path);
    if !root.is_dir() {
        return None;
    }

    let mut best: Option<PathBuf> = None;

    for dir in known_steam_api_locations(root) {
        if holds_steam_api_dll(&dir) {
            if dir.join("steam_settings").is_dir() {
                return Some(dir);
            }
            if best.is_none() {
                best = Some(dir);
            }
        }
    }

    // Nothing in the usual places, so search. The budget is what keeps this
    // from crawling a 100 GB install: games bury the dll deep, but never
    // behind thousands of directories.
    fn walk(
        dir: &Path,
        depth: u32,
        budget: &mut u32,
        best: &mut Option<PathBuf>,
        exact: &mut Option<PathBuf>,
    ) {
        if depth > 8 || exact.is_some() || *budget == 0 {
            return;
        }
        *budget -= 1;

        if holds_steam_api_dll(dir) {
            if dir.join("steam_settings").is_dir() {
                *exact = Some(dir.to_path_buf());
                return;
            }
            if best.is_none() {
                *best = Some(dir.to_path_buf());
            }
        }

        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth + 1, budget, best, exact);
                if exact.is_some() {
                    return;
                }
            }
        }
    }

    let mut exact = None;
    let mut budget = 4000u32;
    walk(root, 0, &mut budget, &mut best, &mut exact);
    exact.or(best)
}

#[cfg(test)]
mod settings_dir_tests {
    use super::find_emulator_settings_dir;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rl_settings_dir_{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The layout that produced "No encontré steam_api64.dll": six levels
    /// down, under a version-numbered folder.
    #[test]
    fn finds_the_unreal_steamworks_layout() {
        let root = scratch("unreal");
        let deep = root
            .join("Engine/Binaries/ThirdParty/Steamworks/Steamv151/Win64");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("steam_api64.dll"), b"x").unwrap();

        assert_eq!(
            find_emulator_settings_dir(&root.to_string_lossy()),
            Some(deep)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_the_plain_bin64_layout() {
        let root = scratch("bin64");
        let bin = root.join("bin64");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("steam_api64.dll"), b"x").unwrap();

        assert_eq!(find_emulator_settings_dir(&root.to_string_lossy()), Some(bin));
        let _ = fs::remove_dir_all(&root);
    }

    /// Several copies of the dll ship in one game; the one already carrying
    /// steam_settings is the copy the game actually loads.
    #[test]
    fn prefers_the_copy_that_already_has_settings() {
        let root = scratch("two_copies");
        let plain = root.join("bin64");
        let real = root.join("Binaries/Win64");
        fs::create_dir_all(&plain).unwrap();
        fs::create_dir_all(real.join("steam_settings")).unwrap();
        fs::write(plain.join("steam_api64.dll"), b"x").unwrap();
        fs::write(real.join("steam_api64.dll"), b"x").unwrap();

        assert_eq!(find_emulator_settings_dir(&root.to_string_lossy()), Some(real));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gives_up_on_a_game_without_the_dll() {
        let root = scratch("none");
        fs::create_dir_all(root.join("data/textures")).unwrap();
        assert_eq!(find_emulator_settings_dir(&root.to_string_lossy()), None);
        let _ = fs::remove_dir_all(&root);
    }
}

/// Valve's own steam_api64.dll is around 300 KB. Every Steam emulator is far
/// larger — the gbe_fork build shipped with these games is 11 MB — because it
/// carries a whole reimplementation rather than a thin client shim. A megabyte
/// sits an order of magnitude above one and well below the other.
const REAL_STEAM_API_MAX_BYTES: u64 = 1_048_576;

/// Whether the dll in this folder is an emulator rather than Valve's.
///
/// Worth asking because the two need opposite handling and look identical from
/// the outside: an emulated game needs an achievements.json written next to it
/// or it records nothing, while a game using the real dll already reports
/// straight to Steam and gains nothing from one. Installing the emulator's
/// config beside a real dll does nothing at all — but it used to report
/// success, which is worse than doing nothing.
///
/// Games change sides, too: Crimson Desert shipped emulated and had its dll
/// replaced by the real one a few days later, so this is decided by looking at
/// what is on disk now, never cached.
fn folder_uses_steam_emulator(dir: &Path) -> bool {
    for name in ["steam_api64.dll", "steam_api.dll"] {
        if let Ok(meta) = std::fs::metadata(dir.join(name)) {
            if meta.len() > REAL_STEAM_API_MAX_BYTES {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod emulator_detection_tests {
    use super::{folder_uses_steam_emulator, REAL_STEAM_API_MAX_BYTES};
    use std::fs;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rl_emu_{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The real dll, at the size Valve ships.
    #[test]
    fn a_valve_sized_dll_is_not_an_emulator() {
        let dir = scratch("real");
        fs::write(dir.join("steam_api64.dll"), vec![0u8; 301_928]).unwrap();
        assert!(!folder_uses_steam_emulator(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The gbe_fork build these games actually shipped with.
    #[test]
    fn an_eleven_megabyte_dll_is_an_emulator() {
        let dir = scratch("emu");
        fs::write(
            dir.join("steam_api64.dll"),
            vec![0u8; (REAL_STEAM_API_MAX_BYTES + 1) as usize],
        )
        .unwrap();
        assert!(folder_uses_steam_emulator(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_dll_at_all_is_not_an_emulator() {
        let dir = scratch("none");
        assert!(!folder_uses_steam_emulator(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Gives an emulated game the achievement list it is missing.
///
/// gbe_fork only tracks achievements it has a schema for. Without
/// `steam_settings/achievements.json` the game's unlock calls match nothing at
/// all — the emulator does not even create its save folder, so the player sees
/// no popup and no progress and assumes the game simply has no achievements.
///
/// The schema comes from Steam's own `UserGameStatsSchema_<appid>.bin`, which
/// is the authoritative pairing of API name to display name. Building it from
/// the public web pages instead looks tempting but is wrong: those pages give
/// no API names, so the only join available is the global completion
/// percentage — and achievements that share a percentage then get paired
/// arbitrarily, silently swapping names between them.
#[command]
async fn install_emulator_achievements(
    steam_path: String,
    app_id: String,
) -> Result<String, String> {
    let schema = load_local_achievement_schema(&steam_path, &app_id).ok_or_else(|| {
        "Steam todavía no tiene el esquema de logros de este juego. Abrí su ficha en Steam una vez \
         y volvé a intentarlo."
            .to_string()
    })?;
    if schema.is_empty() {
        return Err("Este juego no tiene logros.".to_string());
    }

    // The Achievements tab carries only the appid, so the install folder is
    // resolved the same way the rest of the app resolves it.
    let game_path = crate::managers::crackers::find_game_folder(&steam_path, &app_id)
        .ok_or_else(|| "No encontré la carpeta de este juego instalada.".to_string())?;

    let base = find_emulator_settings_dir(&game_path.to_string_lossy()).ok_or_else(|| {
        "No encontré steam_api64.dll en la carpeta del juego, así que no sé dónde poner los logros."
            .to_string()
    })?;
    // A game on the real dll reports to Steam by itself — but only once it
    // knows which app it is.
    //
    // SteamAPI_Init learns the appid from the environment Steam sets when it
    // launches the process, or from a steam_appid.txt beside the exe. Launched
    // straight from its own executable with neither present, the API cannot
    // attach to anything and the game unlocks nothing at all — silently, since
    // from the player's side it simply runs. Assassin's Creed Black Flag
    // Resynced sat at 0 of 49 achievements this way, with Steam's own
    // RunningAppID still reading 0 while the game was open.
    //
    // So for these games the useful thing to install is not the emulator's
    // achievement list, it is that one file.
    if !folder_uses_steam_emulator(&base) {
        let marker = base.join("steam_appid.txt");
        let already = std::fs::read_to_string(&marker)
            .map(|c| c.trim() == app_id)
            .unwrap_or(false);
        if already {
            return Ok(format!(
                "Este juego usa la API real de Steam y ya está identificado, así que sus {} \
                 logros se guardan directos en tu cuenta.",
                schema.len()
            ));
        }
        std::fs::write(&marker, &app_id)
            .map_err(|e| format!("No se pudo escribir steam_appid.txt: {}", e))?;
        return Ok(format!(
            "Este juego usa la API real de Steam. Le puse su identificador en {} para que \
             reconozca sus {} logros aunque lo abras sin pasar por Steam. Reiniciá el juego.",
            marker.display(),
            schema.len()
        ));
    }

    let settings = base.join("steam_settings");
    std::fs::create_dir_all(&settings).map_err(|e| e.to_string())?;

    // The emulator's own format: a flat array, one object per achievement.
    let entries: Vec<serde_json::Value> = schema
        .iter()
        .map(|a| {
            serde_json::json!({
                "name": a.name,
                "displayName": a.display_name,
                "description": a.description,
                "hidden": if a.hidden { "1" } else { "0" },
                "icon": a.icon,
                "icongray": a.icongray,
            })
        })
        .collect();

    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    std::fs::write(settings.join("achievements.json"), json).map_err(|e| e.to_string())?;

    // ── icons ────────────────────────────────────────────────────────────────
    // Named by hash in the schema and fetched from the community CDN. A
    // missing icon only costs the popup its picture, so a failed download is
    // reported but never fails the whole install.
    let images = settings.join("achievement_images");
    std::fs::create_dir_all(&images).map_err(|e| e.to_string())?;

    let mut wanted: Vec<String> = Vec::new();
    for a in &schema {
        for name in [&a.icon, &a.icongray] {
            if !name.is_empty() && !wanted.contains(name) {
                wanted.push(name.clone());
            }
        }
    }

    let client = make_client()?;
    let (mut got, mut had, mut failed) = (0usize, 0usize, 0usize);
    for name in &wanted {
        let dest = images.join(name);
        if dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            had += 1;
            continue;
        }
        let url = format!(
            "https://shared.fastly.steamstatic.com/community_assets/images/apps/{}/{}",
            app_id, name
        );
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) if b.len() > 100 => {
                    if std::fs::write(&dest, &b).is_ok() {
                        got += 1;
                    } else {
                        failed += 1;
                    }
                }
                _ => failed += 1,
            },
            _ => failed += 1,
        }
    }

    let where_ = settings.display();
    let mut msg = format!(
        "Listo: {} logros instalados en {}. Iconos: {} descargados",
        schema.len(),
        where_,
        got
    );
    if had > 0 {
        msg.push_str(&format!(", {} ya estaban", had));
    }
    if failed > 0 {
        msg.push_str(&format!(", {} fallaron", failed));
    }
    msg.push_str(". Reiniciá el juego para que empiece a contarlos.");
    Ok(msg)
}

/// Read-only companion to get_game_achievements — same schema + local-record
/// merge, but skips the live Steam client read entirely (no ach_helper
/// call), so it's safe to use for a game that isn't currently installed.
/// Also off the main thread: the history modal calls this on hover, once per
/// row, and each call does the same eight-cracker scan as above.
#[command]
async fn get_cached_game_achievements(steam_path: String, app_id: String) -> Result<Vec<AchievementInfo>, String> {
    tokio::task::spawn_blocking(move || get_cached_game_achievements_blocking(&steam_path, &app_id))
        .await
        .map_err(|e| e.to_string())?
}

fn get_cached_game_achievements_blocking(steam_path: &str, app_id: &str) -> Result<Vec<AchievementInfo>, String> {
    let schema = load_cached_schema(app_id).ok_or_else(|| "Este juego no tiene logros cacheados todavía — ábrelo primero en la pestaña de Logros mientras esté instalado.".to_string())?;
    let local_state = load_local_achievement_state(steam_path, app_id);

    // The cache is built from Steam's Web API, which returns an empty
    // description for hidden achievements — Elden Ring came back with 6 of 42
    // described, so most rows rendered with a bare dash. The schema Steam
    // downloads for itself has all 42, so it fills the gaps here. Only gaps:
    // where the Web API did answer, its text is the one already translated to
    // the user's language.
    let local_schema = load_local_achievement_schema(steam_path, app_id);
    let describe = |name: &str| -> Option<(String, String)> {
        local_schema.as_ref()?.iter().find(|a| a.name == name).map(|a| {
            (a.display_name.clone(), a.description.clone())
        })
    };

    Ok(schema.into_iter().map(|ach| {
        let (earned, earned_time) = local_state.get(&ach.name).copied().unwrap_or((false, 0));
        let (mut display_name, mut description) = (ach.display_name, ach.description);
        if display_name.trim().is_empty() || description.trim().is_empty() {
            if let Some((local_name, local_desc)) = describe(&ach.name) {
                if display_name.trim().is_empty() {
                    display_name = local_name;
                }
                if description.trim().is_empty() {
                    description = local_desc;
                }
            }
        }
        AchievementInfo {
            display_name,
            description,
            hidden: ach.hidden,
            icon: ach.icon,
            icongray: ach.icongray,
            global_percent: ach.global_percent,
            name: ach.name,
            earned,
            earned_time,
        }
    }).collect())
}

/// Sets or clears one achievement for a game Ragnarok can't reach through the
/// live Steam client.
///
/// Writes to two places on purpose, because they answer different questions:
///
///  * The game's OWN cracker store, when it has one. This is the only write
///    the game itself reads — an achievement in Goldberg's JSON is invisible
///    to a CODEX game, which looks for an INI somewhere else entirely.
///  * Ragnarok's local record, which is what the Achievements tab shows and
///    what survives the game being uninstalled.
///
/// A missing cracker store is not an error: plenty of games have none, and
/// the local record still gives the user the tab they expect.
#[command]
async fn set_local_achievement(
    app_id: String,
    achievement_name: String,
    earned: bool,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || set_local_achievement_blocking(app_id, achievement_name, earned))
        .await
        .map_err(|e| e.to_string())?
}

fn set_local_achievement_blocking(
    app_id: String,
    achievement_name: String,
    earned: bool,
) -> Result<String, String> {
    let earned_time = if earned {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    } else {
        0
    };

    // Held back for 2.1.3: `write_cracker_achievement` is finished and
    // tested, but writing into a game's own CODEX/OnlineFix/RLD file is a
    // change to somebody's save data, and it has never run against a real
    // game. Only the local record is written until that has been tried.
    //
    // Disabled here at the single call site rather than by deleting the
    // writer: the code and its tests stay in place, and turning it back on is
    // this block again.
    write_local_achievement(&app_id, &achievement_name, earned, earned_time)?;

    Ok("Guardado en el registro local.".to_string())
}

fn write_local_achievement(app_id: &str, achievement_name: &str, earned: bool, earned_time: u64) -> Result<(), String> {
    let path = local_achievements_file_for_write(app_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut root: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    root[achievement_name] = serde_json::json!({ "earned": earned, "earned_time": earned_time });

    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Snapshots every EARNED entry from a fresh live-merged achievements read
/// into the same local-record file set_local_achievement writes to — one
/// read-modify-write for the whole batch, not one per achievement (matters
/// for games with huge achievement lists). This is what lets a game's
/// completion state survive uninstalling it: the live Steam read this data
/// came from only works while the game has a real ACF (see
/// get_really_installed_app_ids), so without a durable local snapshot taken
/// while it WAS installed, there'd be nothing left to show once it isn't.
/// Only ever writes earned=true rows — never overwrites an achievement with
/// false, so this can't clobber a local record from some other source
/// (cracker tools, manual edits) for names this fetch didn't recognize.
fn persist_achievement_snapshot(app_id: &str, entries: &[AchievementInfo]) {
    let earned: Vec<&AchievementInfo> = entries.iter().filter(|a| a.earned).collect();
    if earned.is_empty() {
        return;
    }
    let path = match local_achievements_file_for_write(app_id) {
        Ok(p) => p,
        Err(_) => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut root: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    for ach in earned {
        root[&ach.name] = serde_json::json!({ "earned": true, "earned_time": ach.earned_time });
    }
    if let Ok(text) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::write(&path, text);
    }
}

/// Locates a bundled helper resource, checking the dev-mode project root
/// first (matches the convention already used for hardware_db.json /
/// "archivos de OpenSteamTool") and falling back to next-to-the-installed-exe
/// for a packaged build.
fn find_resource(filename: &str) -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        let dev_path = cwd.join("src-tauri").join("resources").join(filename);
        if dev_path.exists() {
            return Some(dev_path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("resources").join(filename);
            if bundled.exists() {
                return Some(bundled);
            }
        }
    }
    None
}

// Serializes every ach_helper invocation app-wide. Two SteamAPI_Init() calls
// for the same (or even different) AppID landing on Steam's client at the
// same time — e.g. from double-clicking a toggle during the old hang, or
// switching games while a previous "get" was still resolving — was traced
// as the actual cause of Steam auto-launching the real game as a side
// effect (a single, isolated call never did this in direct testing). Only
// one ach_helper process is allowed to talk to Steam at a time.
static ACH_HELPER_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Runs a Command with a hard wall-clock timeout, killing it if exceeded.
///
/// `Command::output()` blocks indefinitely if the child never exits — and it
/// turns out ach_helper CAN hang forever: SteamAPI_Init()/RequestCurrentStats
/// end up waiting on Steam's own IPC pipe, and if the Steam client itself is
/// busy or stuck on some unrelated background dialog, that wait never
/// returns. Since this runs inside spawn_blocking, it doesn't block Tokio,
/// but it silently ties up the caller (a toggle button spinning forever) and
/// was observed to make the whole Ragnarok window go "Not Responding" —
/// spawning several of these in a row (e.g. switching between games quickly
/// in the Achievements tab) piles up stuck child processes. A hard timeout
/// makes every call fail cleanly instead of hanging.
fn run_helper_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Result<(bool, String, String), String> {
    use std::io::Read;

    // Poisoning (a previous call panicking mid-lock) shouldn't permanently
    // brick achievement syncing — recover the guard either way.
    let _guard = ACH_HELPER_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    // Drain both pipes on their own threads, starting now.
    //
    // They used to be read only after the child had exited, which deadlocks
    // the moment the child writes more than a pipe buffer holds (tens of KB on
    // Windows): the child blocks inside its own `eprintln!`, so it never exits,
    // so `try_wait` never returns Some, so nothing is ever read — and the
    // timeout below kills a helper that was working perfectly.
    //
    // ach_helper reaches that easily. With ACH_HELPER_DEBUG=1 it prints a line
    // per Steam callback in a loop that only sleeps when no callback arrived,
    // and Steam's callback pipe is global, so a busy Steam produces hundreds of
    // lines in seconds. In "get" mode it prints a line per achievement, so a
    // game with a long list fills stdout on its own.
    //
    // This is very likely the real cause of the intermittent achievement
    // failures the comments above blame on Steam being flaky — and it fits the
    // note that running the helper by hand from a console always works: there
    // the output goes to a terminal, and a terminal never fills up.
    let mut stdout_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = pipe.read_to_string(&mut buf);
            buf
        })
    });
    let mut stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = pipe.read_to_string(&mut buf);
            buf
        })
    });

    // Killing the child closes its pipes, so both readers finish on their own.
    let collect = |h: Option<std::thread::JoinHandle<String>>| -> String {
        h.map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    };

    let start = std::time::Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = collect(stdout_reader.take());
                let stderr = collect(stderr_reader.take());
                return Ok((status.success(), stdout, stderr));
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Whatever it managed to say before being killed is often
                    // the only clue about why, so keep it out of the void.
                    let partial = collect(stderr_reader.take());
                    let _ = collect(stdout_reader.take());
                    if !partial.trim().is_empty() {
                        diag_log!("[ach_helper] Salida antes del timeout: {}", partial.trim());
                    }
                    return Err("Steam no respondió a tiempo (tardó demasiado). Intenta de nuevo.".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Steam client (same helper binary as sync_steam_achievement, "get" mode).
/// Returns an empty map — never an error — on any failure (Steam closed,
/// helper missing, etc.), since this only supplements the achievements list
/// and must not block it from rendering.
///
/// Talks to steamclient(64).dll — Steam's OWN client library, always in the
/// Steam install folder itself — rather than any specific game's bundled
/// steam_api(64).dll. See sync_steam_achievement for why.
fn read_steam_achievement_state(
    steam_path: &str,
    app_id: &str,
    names: &[String],
) -> std::collections::HashMap<String, (bool, u64)> {
    let mut result = std::collections::HashMap::new();
    if names.is_empty() {
        return result;
    }

    let helper_path = match find_resource("ach_helper_x64.exe") {
        Some(p) => p,
        None => return result,
    };

    // Same transient-connection flakiness as sync_steam_achievement — retry
    // rather than silently showing stale/wrong "not earned" state on a
    // one-off failure.
    let mut stdout = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(1500 * attempt as u64));
        }
        let mut cmd = std::process::Command::new(&helper_path);
        cmd.arg(steam_path)
            .arg(app_id)
            .arg("get")
            .arg(names.join(","));
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        match run_helper_with_timeout(cmd, std::time::Duration::from_secs(15)) {
            Ok((_, out, _)) if !out.trim().is_empty() => {
                stdout = out;
                break;
            }
            _ => continue,
        }
    }
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, '=').collect();
        if parts.len() == 3 {
            let earned = parts[1] == "1";
            let earned_time: u64 = parts[2].parse().unwrap_or(0);
            result.insert(parts[0].to_string(), (earned, earned_time));
        }
    }
    result
}

// Unlocks/locks an achievement in the REAL, running Steam client (SAM-style,
// following github.com/gibbed/SteamAchievementManager's own approach) for
// games installed through Ragnarok's ownership-flag method (OpenSteamTool /
// SteamTools) — these are not Goldberg-emulated, so the achievement panel
// Steam's own UI shows for them is backed by Steam's real local stats cache,
// which only the real client can update.
//
// An earlier version of this called SteamAPI_Init() through the target
// game's own steam_api(64).dll (the high-level flat API) — that was found to
// intermittently make Steam actually LAUNCH the real game as a side effect
// of re-validating ownership, and remained unreliable after several rounds
// of tuning. This version instead loads steamclient(64).dll — Steam's own
// client library, always present in the Steam install folder itself,
// completely independent of any specific game's files — and talks to the
// already-running Steam client through its low-level C++ interfaces
// directly, exactly like SAM does. In testing this was 100% reliable across
// 10 consecutive set/clear cycles with zero game launches, vs. the frequent
// intermittent failures of the flat-API approach.
#[command]
async fn sync_steam_achievement(
    app_handle: tauri::AppHandle,
    steam_path: String,
    app_id: String,
    achievement_name: String,
    earned: bool,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        // Live progress feed for the Achievements tab's on-screen console —
        // requested so the user can see what's actually happening (and
        // share it) without needing to relay findings back and forth over
        // chat every time something fails.
        let log = |msg: String| {
            let _ = app_handle.emit_all("achievement_sync_log", msg);
        };

        let action_label = if earned { "activar" } else { "quitar" };
        log(format!("Intentando {} '{}'...", action_label, achievement_name));

        let helper_path = match find_resource("ach_helper_x64.exe") {
            Some(p) => p,
            None => {
                let msg = "Falta el componente ach_helper_x64.exe del launcher.".to_string();
                log(format!("✗ {}", msg));
                return Err(msg);
            }
        };

        // Every isolated, one-off manual test of this exact helper succeeds
        // reliably (confirmed repeatedly) — but real usage through the app,
        // where the Achievements tab has likely already opened/closed
        // several Steam pipe connections in quick succession (browsing
        // between games, each triggering its own connect/request/release
        // cycle for the live-state read), intermittently fails. Steam's own
        // logs also show every connection attempt for these ownership-flag
        // games gets an "ownership ticket ... Access Denied" — not
        // necessarily the direct cause (writes mostly succeed despite it),
        // but evidence Steam is doing extra background validation work per
        // connection that can transiently contend with our own. Retrying
        // several times with real spacing reliably lands on a working
        // moment in practice — this must be resilient enough that the user
        // never has to notice or manually retry themselves.
        let mut last_err = String::new();
        for attempt in 0..5 {
            if attempt > 0 {
                let wait_secs = 1.5 * attempt as f64;
                log(format!("Reintentando (intento {}/5, esperé {:.1}s)...", attempt + 1, wait_secs));
                std::thread::sleep(std::time::Duration::from_millis(1500 * attempt as u64));
            }

            // ach_helper's CLI contract is <steam_path> <app_id> <action> <payload>
            // (action before the achievement name) — this call had them
            // swapped, so every real invocation actually ran
            // ClearAchievement("set")/SetAchievement("clear") instead of
            // Set/ClearAchievement(achievement_name). That's the true root
            // cause of every failure seen through the live app: manual CLI
            // testing always typed the correct order by hand and so never
            // hit this bug, masking it as "Steam-side flakiness".
            let mut cmd = std::process::Command::new(&helper_path);
            cmd.arg(&steam_path)
                .arg(&app_id)
                .arg(if earned { "set" } else { "clear" })
                .arg(&achievement_name)
                .env("ACH_HELPER_DEBUG", "1");

            #[cfg(windows)]
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

            let (success, stdout, stderr) = match run_helper_with_timeout(cmd, std::time::Duration::from_secs(15)) {
                Ok(v) => v,
                Err(e) => {
                    log(format!("✗ {}", e));
                    return Err(e);
                }
            };

            // Full step-by-step trace (interface resolution, callback ids
            // seen, etc.) for THIS specific attempt — needed to actually see
            // where a live failure diverges from isolated manual testing,
            // which has succeeded 100% of the time so far.
            for line in stderr.lines() {
                if let Some(dbg) = line.strip_prefix("DEBUG: ") {
                    log(format!("  · {}", dbg));
                }
            }

            if success && stdout.contains("OK") {
                log(format!("✓ Sincronizado con Steam (intento {}/5).", attempt + 1));
                return Ok("Sincronizado con Steam.".to_string());
            }

            // steamclient(64).dll and the OS loader can write harmless debug
            // lines to stderr — only lines WE printed with the "ERROR: "
            // prefix are real failure reasons.
            last_err = stderr
                .lines()
                .find(|l| l.starts_with("ERROR: "))
                .map(|l| l.trim_start_matches("ERROR: ").to_string())
                .unwrap_or_else(|| "Steam debe estar abierto para sincronizar logros.".to_string());
            log(format!("Intento {}/5 falló: {}", attempt + 1, last_err));
        }
        log(format!("✗ Falló después de 5 intentos: {}", last_err));
        Err(last_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Same as `sync_steam_achievement` but for many achievements at once — used
/// by the Achievements tab's "Desbloquear todos" button. ach_helper now
/// accepts a comma-separated payload and calls StoreStats() only once at the
/// end, so this is ONE Steam connect/RequestUserStats/callback-wait cycle
/// for the whole batch instead of one per achievement (which, for a game
/// with 50+ achievements, would each pay that same multi-second cost and
/// compound the connection contention already described above).
/// What a batch sync actually achieved. A count, not a sentence: the
/// achievement bridge needs to know whether anything was rejected so it can
/// retry, and parsing that back out of a human-readable message is not a
/// thing to build a retry decision on.
#[derive(serde::Serialize, Clone)]
struct SyncOutcome {
    done: usize,
    failed: usize,
    message: String,
}

#[command]
#[allow(dead_code)]
async fn sync_steam_achievements_batch(
    app_handle: tauri::AppHandle,
    steam_path: String,
    app_id: String,
    achievement_names: Vec<String>,
    earned: bool,
) -> Result<SyncOutcome, String> {
    if achievement_names.is_empty() {
        return Ok(SyncOutcome { done: 0, failed: 0, message: "Nada que sincronizar.".to_string() });
    }
    let payload = achievement_names.join(",");

    tokio::task::spawn_blocking(move || {
        let log = |msg: String| {
            let _ = app_handle.emit_all("achievement_sync_log", msg);
        };

        let action_label = if earned { "activar" } else { "quitar" };
        log(format!("Intentando {} {} logro(s)...", action_label, achievement_names.len()));

        let helper_path = match find_resource("ach_helper_x64.exe") {
            Some(p) => p,
            None => {
                let msg = "Falta el componente ach_helper_x64.exe del launcher.".to_string();
                log(format!("✗ {}", msg));
                return Err(msg);
            }
        };

        let mut last_err = String::new();
        for attempt in 0..5 {
            if attempt > 0 {
                let wait_secs = 1.5 * attempt as f64;
                log(format!("Reintentando (intento {}/5, esperé {:.1}s)...", attempt + 1, wait_secs));
                std::thread::sleep(std::time::Duration::from_millis(1500 * attempt as u64));
            }

            let mut cmd = std::process::Command::new(&helper_path);
            cmd.arg(&steam_path)
                .arg(&app_id)
                .arg(if earned { "set" } else { "clear" })
                .arg(&payload)
                .env("ACH_HELPER_DEBUG", "1");

            #[cfg(windows)]
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

            let (success, stdout, stderr) = match run_helper_with_timeout(cmd, std::time::Duration::from_secs(30)) {
                Ok(v) => v,
                Err(e) => {
                    log(format!("✗ {}", e));
                    return Err(e);
                }
            };

            for line in stderr.lines() {
                if let Some(dbg) = line.strip_prefix("DEBUG: ") {
                    log(format!("  · {}", dbg));
                }
            }

            if success && stdout.contains("OK") {
                // Report what actually landed, not what was asked for.
                //
                // The helper exits 0 and prints OK as soon as ONE achievement
                // of the batch succeeded, so this used to announce
                // `achievement_names.len()` — "500 logro(s) sincronizado(s)"
                // for a run that wrote one. The user saw a green tick, went to
                // look at their Steam profile, and found a single achievement
                // with nothing anywhere explaining the other 499. The helper
                // now prints `COUNT ok=N fail=M`; this reads it.
                //
                // The count is missing only when an older helper is in place,
                // which the build step should now prevent — falling back to
                // the requested figure keeps that case working rather than
                // reporting zero.
                let counted = stdout
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("COUNT ok="))
                    .and_then(|rest| {
                        let (ok, fail) = rest.split_once(" fail=")?;
                        Some((ok.trim().parse::<usize>().ok()?, fail.trim().parse::<usize>().ok()?))
                    });

                let (done, failed) = counted.unwrap_or((achievement_names.len(), 0));

                if failed > 0 {
                    // Surface why the rest were refused; the helper prints one
                    // ERROR line per failure and they were being discarded
                    // entirely on a partial success.
                    for reason in stderr.lines().filter(|l| l.starts_with("ERROR: ")).take(5) {
                        log(format!("  · {}", reason.trim_start_matches("ERROR: ")));
                    }
                    log(format!(
                        "⚠ {} de {} logro(s) sincronizado(s); {} rechazado(s) por Steam.",
                        done,
                        done + failed,
                        failed
                    ));
                    return Ok(SyncOutcome {
                        done,
                        failed,
                        message: format!(
                            "{} de {} logro(s) sincronizados. Steam rechazó {} — revisá el registro para el motivo.",
                            done,
                            done + failed,
                            failed
                        ),
                    });
                }

                log(format!("✓ {} logro(s) sincronizado(s) con Steam (intento {}/5).", done, attempt + 1));
                return Ok(SyncOutcome {
                    done,
                    failed: 0,
                    message: format!("{} logro(s) sincronizados con Steam.", done),
                });
            }

            last_err = stderr
                .lines()
                .find(|l| l.starts_with("ERROR: "))
                .map(|l| l.trim_start_matches("ERROR: ").to_string())
                .unwrap_or_else(|| "Steam debe estar abierto para sincronizar logros.".to_string());
            log(format!("Intento {}/5 falló: {}", attempt + 1, last_err));
        }
        log(format!("✗ Falló después de 5 intentos: {}", last_err));
        Err(last_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Extracts a real Encrypted App Ticket (ETicket) for `app_id` from the
/// currently logged-in Steam account, via the same native helper used for
/// achievements ("ticket" action). This is OpenSteamTool's own
/// setETicket(appid, "hex") mechanism (github.com/OpenSteam001/OpenSteamTool
/// #usage) — used for Denuvo-protected games, which validate ownership
/// through this exact ticket rather than the addappid ownership-flag spoof
/// the rest of Ragnarok relies on.
///
/// Only succeeds if the logged-in account genuinely owns the game — Steam
/// enforces this server-side, not something this app can work around. Also
/// requires the game's own developer to have configured an "Encrypted App
/// Ticket Key" for their app in Steamworks (most games with server-side
/// anti-cheat/Denuvo do; plain single-player titles usually don't and this
/// will simply time out for them).
///
/// The ticket is written to its own per-app Lua file (not the main
/// SteamTools.lua) because it expires in ~30-60 minutes — baking it into a
/// config that's only regenerated occasionally would go stale.
#[command]
#[allow(dead_code)]
async fn extract_app_ticket(steam_path: String, app_id: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let helper_path = find_resource("ach_helper_x64.exe")
            .ok_or_else(|| "No se encontró ach_helper_x64.exe entre los recursos de la app.".to_string())?;

        let mut cmd = std::process::Command::new(&helper_path);
        cmd.arg(&steam_path).arg(&app_id).arg("ticket").arg("-");
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        // Steam's own docs: RequestEncryptedAppTicket is rate-limited to one
        // pending call per 60s, so give it the full window before giving up.
        let (success, stdout, stderr) = run_helper_with_timeout(cmd, std::time::Duration::from_secs(60))?;
        if !success {
            let msg = stderr
                .lines()
                .find(|l| l.starts_with("ERROR:"))
                .map(|l| l.trim_start_matches("ERROR:").trim().to_string())
                .unwrap_or_else(|| "Fallo desconocido al extraer el ticket.".to_string());
            return Err(msg);
        }

        let hex = stdout
            .lines()
            .find_map(|l| l.strip_prefix("TICKET="))
            .ok_or_else(|| "El helper no devolvió ningún ticket.".to_string())?
            .to_string();

        let lua_dir = std::path::PathBuf::from(&steam_path).join("config").join("lua");
        std::fs::create_dir_all(&lua_dir).map_err(|e| e.to_string())?;
        let ticket_lua_path = lua_dir.join(format!("RagnarokTicket_{}.lua", app_id));
        let content = format!(
            "-- Auto-generated by Ragnarok Launcher\n-- Encrypted App Ticket for AppID {app_id} — expires ~30-60 min after extraction.\n-- If the game reports a Denuvo/auth error during play, re-run \"Extraer Ticket\" for this AppID.\nsetETicket({app_id}, \"{hex}\")\n",
            app_id = app_id,
            hex = hex,
        );
        std::fs::write(&ticket_lua_path, content).map_err(|e| e.to_string())?;

        Ok(format!(
            "✔ Ticket extraído ({} bytes) y guardado en config/lua/RagnarokTicket_{}.lua. Válido ~30-60 min — vuelve a extraerlo antes de jugar si ha pasado más tiempo.",
            hex.len() / 2,
            app_id
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[command]
async fn run_fixer() -> Result<String, String> {
    // Script mejorado: replica lo que hacía steam.run + fix permanente de arranque
    let ps_script = r#"
# ============================================================
#  RAGNAROK ULTRA FIXER  —  by Ragnarok Launcher
#  Equivalente mejorado de: irm steam.run | iex
#  Ejecutar como Administrador (ya elevado por el launcher)
# ============================================================

# ── Localizar Steam ──────────────────────────────────────────
$steamPath = $null
$registryPaths = @(
    'HKLM:\SOFTWARE\WOW6432Node\Valve\Steam',
    'HKLM:\SOFTWARE\Valve\Steam',
    'HKCU:\SOFTWARE\Valve\Steam'
)
foreach ($regPath in $registryPaths) {
    if (Test-Path $regPath) {
        $val = Get-ItemProperty -Path $regPath -ErrorAction SilentlyContinue
        $p = if ($val.InstallPath) { $val.InstallPath } else { $val.SteamPath }
        if ($p -and (Test-Path $p)) { $steamPath = $p; break }
    }
}

if (-not $steamPath) {
    Write-Host 'ERROR: Steam no encontrado en el registro.' -ForegroundColor Red
    Read-Host 'Presiona Enter para salir...'
    exit 1
}

$steamExe = Join-Path $steamPath 'Steam.exe'

Write-Host ''
Write-Host '  ╔══════════════════════════════════════╗' -ForegroundColor Cyan
Write-Host '  ║     RAGNAROK ULTRA FIXER  v2.0       ║' -ForegroundColor Cyan
Write-Host '  ╚══════════════════════════════════════╝' -ForegroundColor Cyan
Write-Host ''
Write-Host ('  Steam: ' + $steamPath) -ForegroundColor DarkGray
Write-Host ''

# ── PASO 1: Cerrar Steam por completo ────────────────────────
Write-Host '[1/7] Cerrando Steam y procesos asociados...' -ForegroundColor Yellow
$steamProcs = @('steam', 'steamwebhelper', 'steamerrorreporter', 'steamservice', 'GameOverlayUI')
foreach ($proc in $steamProcs) {
    Get-Process -Name $proc -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 2
Write-Host '  -> OK' -ForegroundColor Green

# ── PASO 2: Detener y reparar el Servicio de Steam ───────────
Write-Host '[2/7] Reparando servicio de Steam (SteamService)...' -ForegroundColor Yellow
$svc = Get-Service -Name 'Steam Client Service' -ErrorAction SilentlyContinue
if ($svc) {
    Stop-Service -Name 'Steam Client Service' -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    Write-Host '  -> Servicio detenido' -ForegroundColor Green
} else {
    # Registrar/reparar el servicio si no existe
    $steamSvc = Join-Path $steamPath 'bin\steamservice.exe'
    if (Test-Path $steamSvc) {
        & $steamSvc /Install 2>$null
        Write-Host '  -> Servicio reinstalado' -ForegroundColor Green
    } else {
        Write-Host '  -> Servicio no encontrado (no es crítico)' -ForegroundColor DarkGray
    }
}

# ── PASO 3: Eliminar archivos de bloqueo y configuración corrupta ──
Write-Host '[3/7] Eliminando archivos de bloqueo y configs corruptas...' -ForegroundColor Yellow
# steam.cfg y crash_handler.exe NO van en esta lista. steam.cfg lo escribe el
# usuario a mano (BootStrapperInhibitAll=enable) para que Steam no se
# autoactualice, muchas veces para que el plugin siga funcionando: borrarlo
# provoca justo el "IPC spec missing" que este reparador intenta resolver.
# crash_handler.exe es un binario que Steam trae, no un residuo.
$filesToDelete = @('.crash', 'steam.pid', 'steamui.dll.bak')
foreach ($f in $filesToDelete) {
    $fp = Join-Path $steamPath $f
    if (Test-Path $fp) {
        Remove-Item $fp -Force -ErrorAction SilentlyContinue
        Write-Host ('  -> Eliminado: ' + $f) -ForegroundColor Green
    }
}

# ── PASO 4: Limpiar cachés corruptas ─────────────────────────
Write-Host '[4/7] Limpiando cachés (appcache, package, config/htmlcache)...' -ForegroundColor Yellow
$cacheDirs = @(
    'appcache',
    'package',
    (Join-Path 'config' 'htmlcache')
)
# appcache/stats guarda UserGameStatsSchema_<appid>.bin, de donde el launcher
# lee los nombres y descripciones de los logros. Sin API key de Steam es la
# unica fuente, asi que borrarlo deja la pestana de Logros de TODOS los juegos
# pidiendo "abri la pagina de logros en Steam al menos una vez", sin relacion
# aparente con el boton que se acaba de apretar. Se aparta y se devuelve.
$statsDir = Join-Path $steamPath 'appcache\stats'
$statsTmp = Join-Path $env:TEMP ('ragnarok_stats_' + $PID)
$statsSaved = $false
if (Test-Path $statsDir) {
    try {
        Move-Item $statsDir $statsTmp -Force -ErrorAction Stop
        $statsSaved = $true
    } catch {
        Write-Host '  -> Aviso: no se pudieron apartar los esquemas de logros.' -ForegroundColor Yellow
    }
}

foreach ($d in $cacheDirs) {
    $dp = Join-Path $steamPath $d
    if (Test-Path $dp) {
        Remove-Item $dp -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host ('  -> Limpiado: ' + $d) -ForegroundColor Green
    }
}

if ($statsSaved) {
    New-Item -ItemType Directory -Force (Join-Path $steamPath 'appcache') | Out-Null
    try {
        Move-Item $statsTmp $statsDir -Force -ErrorAction Stop
        Write-Host '  -> Esquemas de logros conservados.' -ForegroundColor Green
    } catch {
        Write-Host ('  -> Aviso: los esquemas quedaron en ' + $statsTmp) -ForegroundColor Yellow
    }
}
# Limpiar solo los archivos de caché de imágenes y datos (no el depotcache ni configs de juegos)
$steamdataCfg = Join-Path $steamPath 'config\config.vdf'
Write-Host '  -> Cachés limpiadas correctamente' -ForegroundColor Green

# ── PASO 5: AGREGAR STEAM AL INICIO DE WINDOWS ───────────────
Write-Host '[5/7] Configurando Steam para iniciar con Windows...' -ForegroundColor Yellow
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$currentVal = (Get-ItemProperty -Path $runKey -Name 'Steam' -ErrorAction SilentlyContinue).Steam
if ($currentVal -ne $steamExe) {
    Set-ItemProperty -Path $runKey -Name 'Steam' -Value $steamExe -Force -ErrorAction SilentlyContinue
    Write-Host '  -> Steam agregado al inicio automatico de Windows' -ForegroundColor Green
} else {
    Write-Host '  -> Steam ya estaba en el inicio automatico' -ForegroundColor DarkGray
}

# Tambien en HKLM si tenemos permisos (para todos los usuarios)
$runKeyLM = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run'
Set-ItemProperty -Path $runKeyLM -Name 'Steam' -Value $steamExe -Force -ErrorAction SilentlyContinue

# ── PASO 6: Verificar Plugin ──────────────────────────────────
Write-Host '[6/7] Verificando plugin de Ragnarok...' -ForegroundColor Yellow
$pluginFound = $false
foreach ($dll in @('dwmapi.dll', 'version.dll', 'xinput1_4.dll')) {
    if (Test-Path (Join-Path $steamPath $dll)) {
        Write-Host ('  -> Plugin detectado: ' + $dll) -ForegroundColor Green
        $pluginFound = $true
        break
    }
}
if (-not $pluginFound) {
    Write-Host '  -> Plugin no encontrado. Usa Instalar Plugin en el launcher.' -ForegroundColor DarkYellow
}

# ── PASO 7: Reiniciar Steam limpiamente ──────────────────────
Write-Host '[7/7] Iniciando Steam en modo limpio...' -ForegroundColor Yellow
Start-Sleep -Seconds 1
if (Test-Path $steamExe) {
    # NOTE: deliberately no -silent here. That flag makes Steam start fully
    # hidden (tray-only, no window at all) — from user reports, this is
    # exactly why "Reparar Steam" looked like it did nothing: Steam actually
    # relaunched fine every time, just invisibly, so people kept clicking
    # Repair over and over waiting for a window that -silent was suppressing.
    Start-Process -FilePath $steamExe -ArgumentList '-clearbeta'
    Write-Host '  -> Steam lanzado, verificando...' -ForegroundColor Green
}

# ── VERIFICACION: confirmar que Steam realmente sigue vivo ───
# Reports of "tengo que ejecutar el fix varias veces" turned out to be this:
# Steam.exe sometimes launches, then immediately crashes/exits on its own a
# few seconds later (usually a conflicting third-party Steam-hook tool also
# proxying dwmapi.dll/version.dll/xinput1_4.dll) — the script above declared
# success the instant Start-Process returned, without checking Steam was
# still running moments later. Wait, check, and retry once automatically
# instead of making the user notice it died and click Repair again by hand.
Start-Sleep -Seconds 5
$steamRunning = Get-Process -Name 'steam' -ErrorAction SilentlyContinue
if (-not $steamRunning) {
    Write-Host '  -> Steam no sigue en ejecucion, reintentando una vez...' -ForegroundColor Yellow
    Start-Process -FilePath $steamExe -ArgumentList '-clearbeta'
    Start-Sleep -Seconds 5
    $steamRunning = Get-Process -Name 'steam' -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host '  ============================================' -ForegroundColor Cyan
if ($steamRunning) {
    Write-Host '  Reparacion COMPLETA. Steam esta corriendo.' -ForegroundColor Green
    Write-Host '  Steam se abrira automaticamente al encender' -ForegroundColor Green
    Write-Host '  la PC desde ahora.' -ForegroundColor Green
} else {
    Write-Host '  Steam no pudo mantenerse abierto tras 2 intentos.' -ForegroundColor Red
    Write-Host '  Esto casi siempre es por OTRO programa tipo Lua/hook' -ForegroundColor Red
    Write-Host '  instalado junto a Steam que no es compatible con' -ForegroundColor Red
    Write-Host '  OpenSteamTool (ambos modifican los mismos archivos:' -ForegroundColor Red
    Write-Host '  dwmapi.dll, version.dll o xinput1_4.dll en la carpeta' -ForegroundColor Red
    Write-Host '  de Steam). Desinstala esas otras herramientas o' -ForegroundColor Red
    Write-Host '  reinstala Steam limpio antes de reintentar.' -ForegroundColor Red
}
Write-Host '  ============================================' -ForegroundColor Cyan
Write-Host ''
Write-Host 'Puedes cerrar esta ventana.' -ForegroundColor DarkGray
"#;
    let script_path = std::env::temp_dir().join("ragnarok_fixer.ps1");
    std::fs::write(&script_path, ps_script).unwrap_or_default();

    // Ejecutamos el script con privilegios de administrador y -NoExit para que el usuario vea el log
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-WindowStyle", "Hidden",
            "-Command",
            &format!("Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -NoExit -File \"{}\"'", script_path.display()),
        ])
        .creation_flags(0x08000000)
        .spawn()
        .map(|_| "Fixer lanzado correctamente".to_string())
        .map_err(|e| e.to_string())
}

// `run_steam_command` was removed here.
//
// It ran `irm steam.run | iex` through `Start-Process -Verb RunAs` with
// `-ExecutionPolicy Bypass`: downloading a script from a third-party domain
// and executing it as Administrator, with no hash, no signature and no review
// of what arrived. Whoever controls steam.run — its owner today, whoever buys
// the domain when it lapses, or anyone able to poison a user's DNS — would
// have had Administrator on every machine that pressed that button.
//
// Deleting it cost nothing: no part of the frontend ever invoked it. If a
// Steam repair helper is wanted again, it needs to ship inside the app or be
// fetched with its hash checked first, the way cloud_redirect.rs does.

fn parse_version(v: &str) -> Vec<u32> {
    v.trim_start_matches('v')
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect()
}

fn is_newer(remote: &str, current: &str) -> bool {
    let r = parse_version(remote);
    let c = parse_version(current);
    for i in 0..r.len().max(c.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if rv != cv { return rv > cv; }
    }
    false
}

/// Hosts the updater is willing to fetch an installer from.
///
/// GitHub serves a release asset from github.com and redirects to
/// objects.githubusercontent.com. Nothing else is a legitimate source for
/// this app's own installer, so anything else is treated as an attempt to
/// point the updater somewhere it shouldn't go.
fn is_trusted_release_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else { return false };
    let host = host.to_ascii_lowercase();
    host == "github.com" || host == "githubusercontent.com" || host.ends_with(".githubusercontent.com")
}

/// 32 hex chars straight from the OS CSPRNG, for a temp path no other
/// process can guess ahead of time.
fn random_token() -> String {
    use aes_gcm::aead::rand_core::RngCore;
    let mut buf = [0u8; 16];
    aes_gcm::aead::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn sha256_hex_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{:02x}", b)).collect()
}

/// The installer asset of a release, exactly as GitHub describes it.
struct ReleaseAsset {
    url: String,
    /// 0 when the API didn't report one.
    size: u64,
    /// GitHub publishes a per-asset `sha256:<hex>` digest. Present on
    /// anything uploaded recently, absent on older releases.
    sha256: Option<String>,
}

/// Reads the latest release and picks its NSIS setup installer.
///
/// Both the update check and the download itself go through here, so the URL
/// that eventually gets executed is always one this function read out of
/// GitHub's API over TLS.
async fn fetch_latest_release(
    client: &reqwest::Client,
) -> Result<(String, Option<ReleaseAsset>), String> {
    // Retry up to 3 times with exponential backoff (1s, 2s, 4s)
    let mut last_err = String::new();
    let mut data: Option<serde_json::Value> = None;
    for attempt in 0u32..3 {
        if attempt > 0 {
            tokio::time::sleep(tokio::time::Duration::from_secs(2u64.pow(attempt - 1))).await;
        }
        match client
            .get(RELEASES_API)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(res) => match res.json::<serde_json::Value>().await {
                Ok(json) => { data = Some(json); break; }
                Err(e) => { last_err = e.to_string(); }
            },
            Err(e) => { last_err = format!("Network error: {}", e); }
        }
    }

    let data = data.ok_or_else(|| format!("Failed after 3 attempts: {}", last_err))?;
    let tag = data["tag_name"].as_str().unwrap_or("v0.0.0").to_string();

    // Only accept the NSIS setup installer — ignore old C# portable builds.
    let assets = data["assets"].as_array().cloned().unwrap_or_default();
    let asset = assets
        .iter()
        .find(|a| {
            let name = a["name"].as_str().unwrap_or("");
            name.ends_with(".exe") && (name.contains("setup") || name.contains("Setup"))
        })
        .and_then(|a| {
            let url = a["browser_download_url"].as_str()?.to_string();
            if !is_trusted_release_url(&url) {
                diag_log!("[update] Asset descartado: host no confiable en {}", url);
                return None;
            }
            Some(ReleaseAsset {
                url,
                size: a["size"].as_u64().unwrap_or(0),
                sha256: a["digest"]
                    .as_str()
                    .and_then(|d| d.strip_prefix("sha256:"))
                    .map(|h| h.trim().to_ascii_lowercase()),
            })
        });

    Ok((tag, asset))
}

#[command]
async fn check_for_updates() -> Result<UpdateInfo, String> {
    let client = make_client()?;
    let (tag, asset) = fetch_latest_release(&client).await?;
    let latest = tag.trim_start_matches('v').to_string();
    let download_url = asset.map(|a| a.url);

    // If no NSIS installer was found in this release, report no update available
    // so the old C# portable exe is never mistakenly downloaded.
    let has_update = is_newer(&latest, APP_VERSION) && download_url.is_some();

    Ok(UpdateInfo {
        has_update,
        current_version: APP_VERSION.to_string(),
        latest_version: latest,
        download_url,
    })
}
#[command]
async fn download_and_apply_update(
    app_handle: tauri::AppHandle,
    download_url: String,
) -> Result<(), String> {
    let client = make_client()?;

    // The URL is re-resolved from GitHub here instead of being trusted as it
    // arrived. This command runs an .exe with the user's own privileges, and
    // its only argument used to be a string handed in from the webview — the
    // one check on it was that the text contained "setup". Asking the API
    // again over TLS means the file that ends up executing is always the
    // asset the release actually publishes, whatever the caller passed.
    let (_tag, asset) = fetch_latest_release(&client).await?;
    let asset = asset.ok_or_else(|| {
        "La release más reciente no publica un instalador válido. Actualización cancelada.".to_string()
    })?;
    if asset.url != download_url {
        diag_log!(
            "[update] La URL recibida no coincide con la de la release ({} en lugar de {}); se usa la de GitHub.",
            download_url, asset.url
        );
    }
    let download_url = asset.url.clone();

    // ── Stream download with retry (up to 3 attempts, exponential backoff) ───
    let mut bytes: Vec<u8> = Vec::new();
    let mut last_err = String::new();
    let mut success = false;

    for attempt in 0u32..3 {
        if attempt > 0 {
            // Notify UI that we're retrying
            app_handle.emit_all(
                "update_download_progress",
                serde_json::json!({ "retry": attempt, "pct": 0u32 })
            ).ok();
            tokio::time::sleep(tokio::time::Duration::from_secs(2u64.pow(attempt - 1))).await;
        }

        let resp = match client.get(&download_url).send().await {
            Ok(r) => r,
            Err(e) => { last_err = e.to_string(); continue; }
        };

        let total = resp.content_length().unwrap_or(0);
        let mut attempt_bytes: Vec<u8> = Vec::with_capacity(total as usize);
        let mut stream = resp.bytes_stream();
        let mut chunk_err = false;

        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(c) => {
                    attempt_bytes.extend_from_slice(&c);
                    if total > 0 {
                        let pct = (attempt_bytes.len() as f64 / total as f64 * 100.0) as u32;
                        app_handle.emit_all("update_download_progress", pct).ok();
                    }
                }
                Err(e) => { last_err = e.to_string(); chunk_err = true; break; }
            }
        }

        if !chunk_err {
            app_handle.emit_all("update_download_progress", 100u32).ok();
            bytes = attempt_bytes;
            success = true;
            break;
        }
    }

    if !success {
        return Err(format!("La descarga falló después de 3 intentos: {}", last_err));
    }

    // ── Verify what actually arrived ──────────────────────────────────────────
    // Three checks, cheapest first. Until now the only thing between a
    // corrupted or substituted download and Start-Process was "the file
    // exists and is as long as the buffer we just wrote", which is true of
    // any bytes that finished transferring.
    if asset.size > 0 && bytes.len() as u64 != asset.size {
        return Err(format!(
            "El instalador descargado no tiene el tamaño esperado ({} bytes en lugar de {}). Actualización cancelada.",
            bytes.len(),
            asset.size
        ));
    }
    if bytes.len() < 2 || &bytes[..2] != b"MZ" {
        return Err("El archivo descargado no es un ejecutable de Windows. Actualización cancelada.".to_string());
    }
    match asset.sha256.as_deref() {
        Some(expected) => {
            let actual = sha256_hex_bytes(&bytes);
            if actual != expected {
                return Err(format!(
                    "El instalador descargado no coincide con el publicado por GitHub (SHA-256 {} en lugar de {}). Actualización cancelada.",
                    actual, expected
                ));
            }
        }
        // Releases uploaded before GitHub began publishing per-asset digests
        // have nothing to compare against. Recorded rather than passed off as
        // a verified download.
        None => diag_log!("[update] La release no publica SHA-256; sólo se verificó tamaño y cabecera del ejecutable."),
    }

    // ── Save to a private, unpredictable temp directory ───────────────────────
    // The installer used to be written to %TEMP%\RagnarokUpdate.exe: a fixed,
    // guessable name in a directory every process running as this user can
    // write to. Anything sitting on that name could swap the file in the gap
    // between the check below and the installer actually starting. A fresh
    // directory with a random name closes that — create_dir fails outright if
    // the name already exists, so it can be neither pre-created nor pointed
    // somewhere else.
    let temp_root = std::env::temp_dir();
    // Sweep whatever a previous update left behind; that installer has long
    // since run, and these are ~100 MB each.
    if let Ok(entries) = std::fs::read_dir(&temp_root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("ragnarok_update_") && entry.path().is_dir() {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    let update_dir = temp_root.join(format!("ragnarok_update_{}", random_token()));
    std::fs::create_dir(&update_dir)
        .map_err(|e| format!("No se pudo crear la carpeta temporal de actualización: {}", e))?;
    let temp_path = update_dir.join("RagnarokUpdate.exe");
    std::fs::write(&temp_path, &bytes).map_err(|e| e.to_string())?;

    // Re-read from disk rather than trusting the buffer just written. Some
    // antivirus products quarantine or rewrite an unsigned exe the moment it
    // lands, and this is also the last point at which the bytes about to be
    // executed can still be compared against what GitHub published.
    match std::fs::read(&temp_path) {
        Ok(on_disk) if on_disk == bytes => {}
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&update_dir);
            return Err("El instalador descargado fue modificado en disco antes de ejecutarse. Actualización cancelada.".to_string());
        }
        Err(_) => {
            return Err("El instalador descargado fue eliminado o modificado (posiblemente por un antivirus) antes de poder ejecutarse.".to_string());
        }
    }

    // ── Launch installer and relaunch app (Windows only) ─────────────────────
    #[cfg(windows)]
    {
        let temp_str = temp_path.to_string_lossy().to_string();

        // Expected install path after NSIS runs (currentUser mode → %LOCALAPPDATA%)
        let install_path = std::env::var("LOCALAPPDATA")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|local| format!("{}\\ragnarok-launcher\\ragnarok-launcher.exe", local))
            .unwrap_or_default();

        // Escape single-quotes for PowerShell string literals by doubling them.
        let safe_temp    = temp_str.replace('\'', "''");
        let safe_install = install_path.replace('\'', "''");

        // Marker file the NEXT launch reads (see check_previous_update_failure)
        // to tell the user what happened — this process is about to exit, so
        // it can no longer show anything itself once the script below runs.
        let marker_path = std::env::temp_dir().join("ragnarok_update_result.txt");
        let safe_marker = marker_path.to_string_lossy().replace('\'', "''");

        // Build a single-line PowerShell command (no backslash continuations —
        // PowerShell uses backtick for that; here we just use semicolons).
        //
        // Logic:
        //   1. Wait 2 s so this process has time to exit and release file locks.
        //   2. Run the NSIS installer silently with -Wait (blocks until finished),
        //      recording its exit code / any launch failure (e.g. blocked by an
        //      AppLocker/Software Restriction Policy on the %TEMP% path) to the
        //      marker file instead of failing silently.
        //   3. Poll every 2 s for up to 60 s until the new exe appears, then launch.
        //      If it never appears, record that in the marker file too.
        // Real-world reports: users with an antivirus that keeps flagging
        // Ragnarok as a false-positive "trojan" (unsigned exe + DLL-proxy
        // injection into Steam's folder looks exactly like real malware
        // behavior to heuristics) see the update silently never apply —
        // Defender quarantines/deletes the downloaded installer sometime in
        // the 2s sleep below, so Start-Process throws a generic "file not
        // found"-style exception that used to get lumped into the same
        // vague installer_blocked bucket as e.g. an AppLocker policy. Check
        // explicitly for the file having vanished first so
        // check_previous_update_failure can name the actual cause.
        let ps_script = if !install_path.is_empty() {
            format!(
                "Start-Sleep -Seconds 2; \
                if (-not (Test-Path '{safe_temp}')) {{ Set-Content -Path '{safe_marker}' -Value 'installer_missing'; exit }} \
                try {{ \
                    $p = Start-Process '{safe_temp}' -ArgumentList '/S' -PassThru -Wait; \
                    if ($p.ExitCode -ne 0) {{ Set-Content -Path '{safe_marker}' -Value \"installer_failed:$($p.ExitCode)\"; exit }} \
                }} catch {{ \
                    Set-Content -Path '{safe_marker}' -Value \"installer_blocked:$($_.Exception.Message)\"; exit \
                }}; \
                $t = '{safe_install}'; \
                for ($i = 0; $i -lt 30; $i++) \
                {{ if (Test-Path $t) {{ Start-Process $t; Remove-Item -Path '{safe_marker}' -ErrorAction SilentlyContinue; exit }}; Start-Sleep -Seconds 2 }}; \
                Set-Content -Path '{safe_marker}' -Value 'relaunch_not_found'"
            )
        } else {
            format!(
                "Start-Sleep -Seconds 2; \
                if (-not (Test-Path '{safe_temp}')) {{ Set-Content -Path '{safe_marker}' -Value 'installer_missing'; exit }} \
                try {{ Start-Process '{safe_temp}' -ArgumentList '/S' -Wait }} \
                catch {{ Set-Content -Path '{safe_marker}' -Value \"installer_blocked:$($_.Exception.Message)\" }}"
            )
        };

        std::process::Command::new("powershell")
            .args([
                "-WindowStyle",   "Hidden",
                "-NonInteractive",
                "-Command",       &ps_script,
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW — no console flicker
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        let _ = temp_path;
        return Err("Auto-update solo está soportado en Windows.".to_string());
    }

    // Record what this update is supposed to produce, plus where the exe
    // being replaced actually lives. check_update_did_not_apply() compares
    // both on the next launch, which is the only way to catch an installer
    // that reports success while the running version never changes.
    // The version is taken from the asset filename (…_2.1.1_x64-setup.exe)
    // rather than re-querying GitHub, so it describes this exact download.
    {
        let target = download_url
            .rsplit('/')
            .next()
            .and_then(|name| name.split('_').nth(1))
            .unwrap_or("")
            .to_string();
        if !target.is_empty() {
            let current_exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let _ = std::fs::write(update_target_path(), format!("{}\n{}", target, current_exe));
        }
    }

    // Cleared here directly, same reasoning as the X button/tray "Salir"
    // fixes above: std::process::exit() terminates immediately with no
    // cleanup, bypassing RunEvent::Exit entirely — without this, literally
    // every single auto-update would leave the crash marker in place and
    // falsely report "didn't shut down cleanly" on the very next launch,
    // right when the update itself is the reason it's starting fresh.
    if let Some(marker) = crash_marker_path() {
        let _ = std::fs::remove_file(&marker);
    }

    // Close this process so NSIS can replace the locked exe file.
    std::process::exit(0);
}

// Called once at startup: reports (and clears) a leftover marker left by a
// previous download_and_apply_update run that failed *after* this process had
// already exited — e.g. the installer got blocked by an antivirus/AppLocker
// policy, or the new exe never appeared. Without this, that failure was
// completely invisible: the app would just vanish with no explanation, then
// show "update available" again the next time it's opened.
/// Records which version an update was meant to produce, written just before
/// this process exits to let the installer run.
///
/// The existing marker above only catches an installer that fails *loudly*
/// (bad exit code, blocked, deleted). The failure reported by users is the
/// silent one: the installer reports success, but the next launch is still
/// the old version, so the app offers the same update again — forever, with
/// no indication anything went wrong. Comparing the running version against
/// the version we intended to install is what turns that infinite loop into
/// a single, explicit message.
fn update_target_path() -> PathBuf {
    std::env::temp_dir().join("ragnarok_update_target.txt")
}

/// Reads the target-version marker and reports when the update plainly did
/// not take effect. Also returns the paths involved, because the most likely
/// causes (a second, older install elsewhere; an install the silent NSIS run
/// couldn't overwrite) are only distinguishable by seeing where the running
/// exe actually is.
fn check_update_did_not_apply() -> Option<String> {
    let path = update_target_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let mut lines = content.lines();
    let target = lines.next()?.trim().to_string();
    let previous_exe = lines.next().unwrap_or("").trim().to_string();
    if target.is_empty() {
        let _ = std::fs::remove_file(&path);
        return None;
    }

    // Update landed: clear the marker and say nothing.
    if !is_newer(&target, APP_VERSION) {
        let _ = std::fs::remove_file(&path);
        return None;
    }

    // Still older than what we installed. Keep the marker so a user who
    // ignores this once still sees it, and name the concrete paths.
    let current_exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "desconocida".to_string());

    let mut msg = format!(
        "La actualización a la v{target} se instaló pero NO se aplicó: seguís ejecutando la v{}.\n\nEstás abriendo:\n{current_exe}",
        APP_VERSION
    );
    if !previous_exe.is_empty() && !previous_exe.eq_ignore_ascii_case(&current_exe) {
        msg.push_str(&format!("\n\nLa actualización se instaló en otra ruta:\n{previous_exe}\n\nTenés dos instalaciones distintas. Desinstalá Ragnarok desde Configuración de Windows, borrá la carpeta que quede, y volvé a instalar una sola vez."));
    } else {
        msg.push_str("\n\nEl instalador no pudo reemplazar ese archivo. Cerrá Ragnarok por completo (incluido el ícono de la bandeja del sistema) y ejecutá el instalador manualmente desde GitHub.");
    }
    Some(msg)
}

#[command]
fn check_previous_update_failure() -> Option<String> {
    // The silent "installed but still old" case takes priority: it's the one
    // that otherwise loops forever without ever writing the marker below.
    if let Some(msg) = check_update_did_not_apply() {
        return Some(msg);
    }

    let marker_path = std::env::temp_dir().join("ragnarok_update_result.txt");
    let content = std::fs::read_to_string(&marker_path).ok()?;
    let _ = std::fs::remove_file(&marker_path);
    let content = content.trim();
    if content.is_empty() {
        return None;
    }

    let message = if content == "installer_missing" {
        "Tu antivirus eliminó el instalador de la actualización antes de que pudiera ejecutarse (Ragnarok no está firmado digitalmente, así que algunos antivirus lo marcan por error). Agrega una excepción para Ragnarok Launcher en tu antivirus, o descarga la actualización manualmente desde GitHub.".to_string()
    } else if let Some(code) = content.strip_prefix("installer_failed:") {
        format!("La actualización automática falló (el instalador devolvió el código {code}). Puede que un antivirus lo haya bloqueado. Descárgala manualmente desde GitHub.")
    } else if let Some(reason) = content.strip_prefix("installer_blocked:") {
        format!("No se pudo ejecutar el instalador de la actualización ({reason}). Es posible que un antivirus o una política de seguridad lo esté bloqueando. Descárgala manualmente desde GitHub.")
    } else if content == "relaunch_not_found" {
        "La actualización se instaló, pero Ragnarok Launcher no pudo reabrirse automáticamente. Ábrelo manualmente desde el menú inicio.".to_string()
    } else {
        format!("La actualización automática falló: {content}")
    };

    Some(message)
}

#[cfg(test)]
mod support_relay_tests {
    use super::*;

    /// Guards the release checklist: a build that cannot reach Discord support
    /// must not ship quietly. Every Discord call funnels through
    /// `make_discord_bot_client`, so this failing means the constants it reads
    /// are half-edited.
    #[test]
    fn a_shipping_build_can_reach_discord_support() {
        if !support_relay_is_configured() {
            // Not a hard failure while support is being reconfigured, but it
            // must be impossible to miss in the test output.
            eprintln!("\n  AVISO: falta el token del bot — el soporte no va a funcionar en este build.\n");
            return;
        }
        assert!(
            DISCORD_API_BASE.starts_with("https://"),
            "las llamadas de soporte tienen que ir por TLS: {DISCORD_API_BASE}"
        );

        // Two shapes are legitimate, and which one is in use is a deployment
        // decision rather than something this test should force:
        //
        //   * the relay (`.../d`), which keeps the bot token out of the binary;
        //   * Discord's own API, for a build that talks to it directly.
        //
        // Anything else is a half-edited constant — the case that shipped a
        // placeholder host and made every support action die with a DNS error
        // nobody could connect to a cause.
        const DISCORD_DIRECT: &str = "https://discord.com/api/v10";
        assert!(
            DISCORD_API_BASE == DISCORD_DIRECT || DISCORD_API_BASE.ends_with("/d"),
            "DISCORD_API_BASE tiene que ser el relay (terminado en /d) o exactamente {DISCORD_DIRECT}, \
             no un valor a medio editar: {DISCORD_API_BASE}"
        );
    }

    /// A missing token has to be refused before a request is ever built.
    /// Otherwise the call goes out unauthenticated and comes back 401, which
    /// reads as "Discord is broken" rather than "this build has no token".
    #[test]
    fn a_missing_token_is_refused_with_an_explanation() {
        assert!(!token_is_configured(""), "sin token no puede haber soporte");
        assert!(token_is_configured("MTUyNzAy.ejemplo.token"), "un token cualquiera sí cuenta");

        // The shipped constant has to satisfy the same rule the client checks,
        // so this test tracks the real build rather than only its own helper.
        assert_eq!(
            support_relay_is_configured(),
            token_is_configured(&get_discord_bot_token()),
            "el chequeo real y el de este test tienen que coincidir"
        );
    }

    /// Mirrors `support_relay_is_configured` exactly, against an arbitrary
    /// value so the rule can be exercised without editing the constant.
    fn token_is_configured(token: &str) -> bool {
        !token.is_empty()
    }
}

fn emit_log(app: &tauri::AppHandle, msg: &str) {
    let _ = app.emit_all("repair_log", msg.to_string());
}

#[command]
async fn deep_repair(
    app_handle: tauri::AppHandle,
    app_id: String,
    steam_path: String,
) -> Result<(), String> {
    use std::path::PathBuf;

    let client = make_client()?;

    // ── Step 1: Lua tracker check ──────────────────────────────────────────
    let lua_path = PathBuf::from(&steam_path)
        .join("config").join("stplug-in")
        .join(format!("{}.lua", app_id));

    let mut depot_ids_from_lua: Vec<u64> = Vec::new();
    if let Ok(content) = std::fs::read_to_string(&lua_path) {
        emit_log(&app_handle, &format!("✔ Lua tracker found: {}", lua_path.display()));
        for line in content.lines() {
            for token in line.split(|c: char| !c.is_ascii_digit()) {
                if let Ok(n) = token.parse::<u64>() {
                    if n > 999 && n < 2_000_000_000 {
                        depot_ids_from_lua.push(n);
                    }
                }
            }
        }
        emit_log(&app_handle, &format!("  → {} depot(s) extracted from Lua", depot_ids_from_lua.len()));
    } else {
        // Check our ragnarok_apps.json sidecar
        let map = load_apps_map(&steam_path);
        if map.contains_key(&app_id) {
            emit_log(&app_handle, "✔ Game found in Ragnarok tracker (sidecar)");
        } else {
            emit_log(&app_handle, "⚠ Game not in local tracker — repairing anyway");
        }
    }

    // ── Step 2: SteamCMD API ──────────────────────────────────────────────
    emit_log(&app_handle, &format!("⟳ Querying SteamCMD API for app {}...", app_id));
    let api_url = format!("https://api.steamcmd.net/v1/info/{}", app_id);
    let res = client
        .get(&api_url)
        .send()
        .await
        .map_err(|e| format!("SteamCMD error: {}", e))?;
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

    let depots = &data["data"][&app_id]["depots"];
    if !depots.is_object() {
        return Err("No depot data returned by SteamCMD".into());
    }

    // Build (depot_id, manifest_gid) pairs
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(depot_map) = depots.as_object() {
        for (depot_id, info) in depot_map {
            // skip non-numeric keys like "branches"
            if !depot_id.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let gid = info["manifests"]["public"]["gid"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| info["manifests"]["public"]["gid"].as_u64().map(|n| n.to_string()));
            if let Some(g) = gid {
                emit_log(&app_handle, &format!("  Depot {} → GID {}", depot_id, g));
                pairs.push((depot_id.clone(), g));
            }
        }
    }

    if pairs.is_empty() {
        return Err("No manifest GIDs found for this app".into());
    }
    emit_log(&app_handle, &format!("⟳ {} manifest(s) to fetch", pairs.len()));

    // ── Steps 3 & 4: Download → depotcache ───────────────────────────────
    let depotcache = PathBuf::from(&steam_path).join("depotcache");
    std::fs::create_dir_all(&depotcache).map_err(|e| e.to_string())?;

    let mut ok = 0u32;
    let mut failed = 0u32;

    for (depot_id, gid) in &pairs {
        let filename = format!("{}_{}.manifest", depot_id, gid);
        emit_log(&app_handle, &format!("⟳ Fetching {}...", filename));

        match fetch_manifest_from_mirrors(&client, &filename).await {
            Some(bytes) => {
                let dest = depotcache.join(&filename);
                match crate::managers::manifest_vault::write_protected_file(&dest, &bytes) {
                    Ok(_) => {
                        emit_log(&app_handle, &format!("  ✔ Injected {} (protected)", filename));
                        ok += 1;
                    }
                    Err(e) => {
                        emit_log(&app_handle, &format!("  ✗ Write error: {}", e));
                        failed += 1;
                    }
                }
            }
            None => {
                emit_log(&app_handle, &format!("  ✗ Not in mirror: {}", filename));
                failed += 1;
            }
        }
    }

    emit_log(&app_handle, &format!(
        "━━ Repair complete: {} injected, {} unavailable ━━",
        ok, failed
    ));
    Ok(())
}

/// Scans this game's install folder for bundled prerequisite installers
/// (VC++/DirectX/.NET redistributables, or per-game middleware like EOS/EAC
/// or the PlayStation PC SDK some crossplay titles need) and runs whatever
/// it finds, one at a time. Scene repacks (DODI, FitGirl, etc.) almost
/// always ship these already — this replaces "dig through the game's
/// Binaries folder and run each .bat by hand" with one click. Streams
/// progress on the same `repair_log` event the Reparar tab already listens
/// to, so no new frontend event wiring is needed.
#[command]
async fn install_prerequisites(app_handle: tauri::AppHandle, app_id: String, steam_path: String) -> Result<String, String> {
    let folder = managers::crackers::find_game_folder(&steam_path, &app_id)
        .ok_or_else(|| "No se encontró una instalación completa de este juego en Steam.".to_string())?;

    let installers = managers::unlockers::RustUnlockerManager::find_prerequisite_installers(
        folder.to_string_lossy().as_ref(),
    );
    if installers.is_empty() {
        return Ok("No se encontró ningún instalador de prerequisitos en la carpeta del juego.".to_string());
    }

    emit_log(&app_handle, &format!("⟳ {} instalador(es) de prerequisitos encontrado(s)", installers.len()));
    let (ok, failed) = managers::unlockers::RustUnlockerManager::run_prerequisite_installers(&app_handle, &installers);
    emit_log(&app_handle, &format!("━━ Prerequisitos: {} ok, {} con error ━━", ok, failed));

    Ok(format!("{} instalador(es) ejecutado(s) ({} ok, {} con error)", installers.len(), ok, failed))
}

// ── Manifests sidecar: app_id → { depot_id → manifest_gid } ─────────────────

/// The one folder these sidecars live in: `config/lua`.
///
/// There used to be two. Both lookups preferred `config/lua` *if the file was
/// already there* and otherwise fell through to `config/stplug-in` — so on a
/// clean install, where neither exists yet, the very first write landed in
/// stplug-in and every later read followed it there. The comment above each
/// one said the file "actually lives" in config/lua, which was true only on
/// machines that had been through an import.
///
/// That split had a real consequence: `import_ragnarok_config` writes straight
/// to `config/lua`, so after an import the preference flipped and whatever the
/// machine had registered in stplug-in became invisible — silently, since a
/// missing file and an empty registry look identical.
///
/// One location now, with the old one migrated on first use rather than
/// abandoned: nobody who already had a registry loses it.
fn sidecar_path(steam_path: &str, filename: &str) -> std::path::PathBuf {
    let config = std::path::PathBuf::from(steam_path).join("config");
    let target = config.join("lua").join(filename);
    if target.exists() {
        return target;
    }

    let legacy = config.join("stplug-in").join(filename);
    if legacy.exists() {
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Copy rather than rename: if anything goes wrong the old file is still
        // there, and a stale duplicate is harmless once nothing reads it.
        match std::fs::copy(&legacy, &target) {
            Ok(_) => {
                diag_log!("[sidecar] {} migrado de stplug-in a config/lua.", filename);
                // Renamed, not deleted, so a person can still find it.
                let _ = std::fs::rename(&legacy, legacy.with_extension("json.migrado"));
                return target;
            }
            Err(e) => {
                diag_log!("[sidecar] No se pudo migrar {} ({}); se sigue usando stplug-in.", filename, e);
                return legacy;
            }
        }
    }

    target
}

fn manifests_map_path(steam_path: &str) -> std::path::PathBuf {
    sidecar_path(steam_path, "ragnarok_manifests.json")
}

fn load_manifests_map(
    steam_path: &str,
) -> std::collections::HashMap<String, std::collections::HashMap<String, String>> {
    let path = manifests_map_path(steam_path);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_manifests_map(
    steam_path: &str,
    map: &std::collections::HashMap<String, std::collections::HashMap<String, String>>,
) {
    let path = manifests_map_path(steam_path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(map) {
        atomic_write(&path, json.as_bytes());
    }
}

/// Writes to a temp file in the same directory and renames it over the
/// destination. A rename is atomic on the same filesystem, so a crash or
/// forced Steam-kill mid-write (this app kills/restarts Steam constantly)
/// can never leave the sidecar JSON half-written/corrupted — the reader
/// either sees the old complete file or the new complete file, never a
/// truncated one.
fn atomic_write(path: &std::path::Path, bytes: &[u8]) {
    let mut tmp_os = path.as_os_str().to_os_string();
    tmp_os.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_os);
    if std::fs::write(&tmp_path, bytes).is_ok() {
        let _ = std::fs::rename(&tmp_path, path);
    }
}

/// Fetches total uncompressed game size in bytes from SteamCMD (sum of depot maxsize fields).
/// Repairs the "0 B" size Steam shows for games this launcher registered.
///
/// The size is written once, by a one-shot task right after install, and that
/// task gives up in two cases: SteamCMD returning 0, and the ACF not appearing
/// within two minutes. Either one leaves the entry at "0 B" in Steam's Storage
/// page forever, with nothing that ever revisits it — which is the state in
/// the screenshots users keep sending (Twisted Tower, Halo, several at 0 B).
///
/// So this exists to be run later, over everything already installed. For each
/// ACF whose SizeOnDisk is 0 or missing, it asks SteamCMD for the real total
/// (the same source the post-install task uses) and patches the file. Returns
/// how many it fixed.
///
/// Deliberately cheap to call repeatedly: an entry that already has a real
/// size is skipped, so re-running it does nothing but read a few files.
/// Install size per app id, read from each library's appmanifest.
///
/// The Library card has always had a size chip and a translated "Tamaño"
/// label for it, and it has never once rendered: `GameMetadata.size_bytes` is
/// declared `Option<u64>` and the only assignment anywhere in the backend
/// sets it to `None`. The frontend then does `formatSize(undefined)`, gets
/// null, and draws nothing — for every game, on every card, always.
///
/// `SizeOnDisk` is the right source: Steam maintains it for real installs,
/// and `repair_install_sizes` already fills it in for the injected ones that
/// would otherwise sit at 0.
#[command]
async fn get_installed_sizes(steam_path: String) -> Result<HashMap<String, u64>, String> {
    tokio::task::spawn_blocking(move || {
        let mut out: HashMap<String, u64> = HashMap::new();
        for lib in ManifestManager::get_library_folders(&steam_path) {
            let steamapps = std::path::Path::new(&lib).join("steamapps");
            let Ok(entries) = std::fs::read_dir(&steamapps) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some(app_id) = name
                    .strip_prefix("appmanifest_")
                    .and_then(|r| r.strip_suffix(".acf"))
                else {
                    continue;
                };
                if !app_id.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(entry.path()) else { continue };
                let size = content
                    .lines()
                    .find(|l| l.trim_start().starts_with("\"SizeOnDisk\""))
                    .and_then(|l| l.rsplit('"').nth(1).and_then(|v| v.trim().parse::<u64>().ok()))
                    .unwrap_or(0);
                // A game in two libraries keeps the larger figure rather than
                // whichever directory happened to be read last.
                if size > 0 {
                    let slot = out.entry(app_id.to_string()).or_insert(0);
                    if size > *slot {
                        *slot = size;
                    }
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[command]
async fn repair_install_sizes(steam_path: String) -> Result<usize, String> {
    let client = make_client()?;
    let libraries = ManifestManager::get_library_folders(&steam_path);

    // Every appmanifest across every library, deduplicated by app id.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut targets: Vec<(String, String)> = Vec::new(); // (library, app_id)

    for lib in &libraries {
        let steamapps = std::path::Path::new(lib).join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(app_id) = name
                .strip_prefix("appmanifest_")
                .and_then(|r| r.strip_suffix(".acf"))
            else {
                continue;
            };
            if !app_id.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            // Only the ones actually showing 0 B — reading the field is far
            // cheaper than a network round trip, and leaves real installs
            // (including genuine Steam games) untouched.
            let Ok(content) = std::fs::read_to_string(entry.path()) else { continue };
            let size_is_zero = content
                .lines()
                .find(|l| l.trim_start().starts_with("\"SizeOnDisk\""))
                .and_then(|l| l.rsplit('"').nth(1).map(|v| v.to_string()))
                .map(|v| v.trim() == "0" || v.trim().is_empty())
                .unwrap_or(true);
            if size_is_zero && seen.insert(app_id.to_string()) {
                targets.push((lib.clone(), app_id.to_string()));
            }
        }
    }

    let mut fixed = 0usize;
    for (lib, app_id) in targets {
        // Same source the post-install task uses: SteamCMD's per-depot maxsize.
        let size = fetch_real_game_size(&client, &app_id).await;
        if size == 0 {
            continue; // no source knew the size; leave it rather than write 0
        }
        if ManifestManager::update_size_on_disk(&lib, &app_id, size).is_ok() {
            fixed += 1;
        }
    }

    Ok(fixed)
}

async fn fetch_real_game_size(client: &reqwest::Client, app_id: &str) -> u64 {
    let url = format!("https://api.steamcmd.net/v1/info/{}", app_id);
    let data: serde_json::Value = match client.get(&url).send().await {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(_) => return 0,
        },
        Err(_) => return 0,
    };

    let depots = match data["data"][app_id]["depots"].as_object() {
        Some(m) => m,
        None => return 0,
    };

    let mut total: u64 = 0;
    for (depot_id, info) in depots {
        if !depot_id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Some(v) = info["maxsize"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| info["maxsize"].as_u64())
        {
            total += v;
        }
    }
    total
}

/// What SteamCMD says about an app's depots, kept as two separate answers.
///
/// They look interchangeable and are not, and collapsing them is what silently
/// stripped DLC and Workshop decryption keys out of the ticket:
///
///   * `gids` is "which depots have content worth pinning". Pinning a manifest
///     for a depot with none is what crashed Steam's content validation.
///   * `windows_ok` is "which depots this machine may use at all". A DLC entry
///     SteamCMD reports at size 0, or a shared runtime depot with no public
///     manifest, belongs here: it has no manifest to pin, but its
///     `addappid(depot, flag, key)` line is exactly what makes the content
///     decryptable.
#[derive(Default)]
struct DepotFacts {
    gids: std::collections::HashMap<String, String>,
    windows_ok: std::collections::HashSet<String>,
}

/// Queries SteamCMD for depot manifest GIDs, downloads .manifest files to
/// depotcache (so Steam can read real file sizes), and reports both depot sets.
/// Best-effort — never fails the calling command.
async fn fetch_and_cache_manifests(
    client: &reqwest::Client,
    app_id: &str,
    steam_path: &str,
    manifest_source: &str,
    hubcap_api_key: &str,
) -> DepotFacts {
    let mut facts = DepotFacts::default();

    let api_url = format!("https://api.steamcmd.net/v1/info/{}", app_id);
    let data: serde_json::Value = match client.get(&api_url).send().await {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(_) => return facts,
        },
        Err(_) => return facts,
    };

    let depots = &data["data"][app_id]["depots"];
    let depot_map = match depots.as_object() {
        Some(m) => m,
        None => return facts,
    };

    let depotcache = std::path::PathBuf::from(steam_path).join("depotcache");
    let _ = std::fs::create_dir_all(&depotcache);

    for (depot_id, info) in depot_map {
        if !depot_id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        // Skip depots restricted to a different OS (e.g. macOS-only depots).
        // Some games ship a separate depot per platform for the base game
        // *and* every DLC (Dave the Diver is a real example: depot 1868142,
        // 2677021, 2841141... are all macOS-only siblings of Windows depots).
        // Writing a setManifestid entry for a depot Steam's Windows client
        // can never actually own/download destabilizes content validation
        // mid-download — this was traced as the cause of Steam repeatedly
        // crashing after installing such a game, with no fix short of
        // removing the game from the library entirely.
        if let Some(oslist) = info["config"]["oslist"].as_str() {
            if !oslist.is_empty() && !oslist.split(',').any(|os| os.trim().eq_ignore_ascii_case("windows")) {
                continue;
            }
        }

        // Recorded before the content checks below, and that ordering is the
        // whole point: everything past this line is about whether the depot has
        // a manifest worth pinning, which says nothing about whether Windows
        // may use it. Verified against PAYDAY 2 — SteamCMD reports depots
        // 218623-218630 (its DLC) at size 0, so they never reached this set,
        // and the sanitizer then deleted their keys from the ticket.
        facts.windows_ok.insert(depot_id.clone());

        // Skip zero-size placeholder depots (e.g. a DLC entry with no actual
        // content, "size": "0"/"download": "0"). Declaring a setManifestid for
        // these and feeding Steam a near-empty manifest for them is what was
        // traced as the real cause of the repeated Dave the Diver crash:
        // Steam's own content manifest parser asserts on an empty name field
        // (Assert( !m_strName.IsEmpty() ), contentmanifest.cpp:1630) when
        // validating this kind of degenerate manifest.
        let size = info["manifests"]["public"]["size"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| info["manifests"]["public"]["size"].as_u64().map(|n| n.to_string()));
        if size.as_deref() == Some("0") {
            continue;
        }

        let gid = info["manifests"]["public"]["gid"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| info["manifests"]["public"]["gid"].as_u64().map(|n| n.to_string()));

        if let Some(g) = gid {
            facts.gids.insert(depot_id.clone(), g.clone());

            // Download manifest file to depotcache (best-effort). When
            // Hubcap's Manifest (hubcapmanifest.com) is selected and a key
            // is configured, try it first — it generates manifests on
            // demand rather than depending on a pre-populated mirror repo —
            // falling back to the GitHub mirror on any failure (missing key,
            // rate limit, 404, network) so a bad/expired key never blocks an
            // install outright.
            let filename = format!("{}_{}.manifest", depot_id, g);
            let dest = depotcache.join(&filename);
            if dest.exists() {
                crate::managers::manifest_vault::make_readonly(&dest);
            } else {
                let mut fetched = false;
                if manifest_source == "hubcap" && !hubcap_api_key.trim().is_empty() {
                    let hubcap_url = format!(
                        "https://hubcapmanifest.com/api/v1/generate/manifest?depot_id={}&manifest_id={}",
                        depot_id, g
                    );
                    if let Ok(resp) = client
                        .get(&hubcap_url)
                        .header("Authorization", format!("Bearer {}", hubcap_api_key.trim()))
                        .timeout(std::time::Duration::from_secs(60))
                        .send()
                        .await
                    {
                        if resp.status().is_success() {
                            if let Ok(bytes) = resp.bytes().await {
                                if !bytes.is_empty() {
                                    let _ = crate::managers::manifest_vault::write_protected_file(&dest, &bytes);
                                    fetched = true;
                                }
                            }
                        }
                    }
                }
                if !fetched {
                    if let Some(bytes) = fetch_manifest_from_mirrors(&client, &filename).await {
                        let _ = crate::managers::manifest_vault::write_protected_file(&dest, &bytes);
                    }
                }
            }
        }
    }
    facts
}

/// Rebuilds SteamTools.lua using current apps + manifests sidecars.
fn rebuild_steam_tools(steam_path: &str) -> Result<(), String> {
    let apps_map = load_apps_map(steam_path);
    let manifests_map = load_manifests_map(steam_path);
    let entries: Vec<(String, Vec<String>)> = apps_map.into_iter().collect();
    RustSteamManager::update_steam_tools(steam_path, entries, &manifests_map)
}

fn apps_map_path(steam_path: &str) -> std::path::PathBuf {
    sidecar_path(steam_path, "ragnarok_apps.json")
}

/// The registry of games this launcher installed.
///
/// A failure to read or parse is loud now. It used to collapse into an empty
/// map via `unwrap_or_default()`, and an empty map is indistinguishable from
/// "this user has no games" — which `update_steam_tools` then wrote to all
/// three .lua files, erasing the user's whole library from Steam without a
/// single error message. `update_steam_tools` refuses an empty list as well,
/// so the two guards cover each other, but the silence started here.
///
/// A corrupt file is also preserved rather than left to be overwritten: it is
/// the only record of what was installed, and a person may well be able to
/// repair it by hand.
fn load_apps_map(steam_path: &str) -> std::collections::HashMap<String, Vec<String>> {
    let path = apps_map_path(steam_path);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                diag_log!("[apps_map] No se pudo leer {}: {}", path.display(), e);
            }
            return Default::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(map) => map,
        Err(e) => {
            diag_log!(
                "[apps_map] {} está corrupto ({}). Se conserva una copia en .corrupto y se \
                 trabaja con un registro vacío — los .lua NO se reescriben.",
                path.display(),
                e
            );
            let _ = std::fs::copy(&path, path.with_extension("json.corrupto"));
            Default::default()
        }
    }
}

fn save_apps_map(steam_path: &str, map: &std::collections::HashMap<String, Vec<String>>) {
    let path = apps_map_path(steam_path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(map) {
        atomic_write(&path, json.as_bytes());
    }
}

/// Ensures a game's cover image is cached on disk and returns its local file
/// path (NOT the file contents). The frontend turns this into an asset://
/// URL via Tauri's convertFileSrc and loads it directly in an <img> tag.
///
/// This used to read the cached file and re-encode it as a base64 data-URL
/// on every single call — including cache hits — which meant every visible
/// game card round-tripped tens/hundreds of KB of base64 text through Tauri's
/// JSON-based IPC bridge just to display an image already sitting on disk.
/// Returning the bare path lets the WebView stream the file natively (same
/// mechanism as a normal <img src> load), which is what actually removed the
/// scroll/catalog jank — the IPC+base64 round trip was the real bottleneck,
/// not React rendering.
/// Cache folder: %APPDATA%\Roaming\com.ragnarok.launcher\image_cache\
/// Writes cover art into the cache, re-encoded to cut what it costs on disk.
///
/// Steam serves headers at near-maximum JPEG quality. Measured across a sample
/// of the real cache, re-encoding at quality 80 keeps the very same 460x215
/// pixels in 41% of the bytes — a full-catalog cache goes from 3.1 GB to about
/// 1.3 GB. The loss is invisible in the 128px-tall box a card actually draws it
/// in, and the format does not change, so WebView2 and WebKitGTK keep rendering
/// it exactly as before.
///
/// Anything that fails to decode, or that does not actually get smaller, is
/// written through untouched: a cover stored as-is beats no cover at all.
fn write_cover_jpeg(path: &Path, bytes: &[u8]) {
    match encode_cover(bytes) {
        Some(out) if out.len() < bytes.len() => {
            let _ = std::fs::write(path, &out);
        }
        _ => {
            let _ = std::fs::write(path, bytes);
        }
    }
}

/// Decodes a cover and re-encodes it as a smaller JPEG.
///
/// What makes it smaller, none of it visible at the size a card draws it:
///   * quality 75 instead of 80,
///   * progressive scans with optimised Huffman tables, which cost nothing at
///     decode time and trim several percent more.
/// Measured on 564 files of the real cache (already at quality 80): 52% of the
/// bytes they started with, ~31 dB PSNR on average and no visible difference
/// even on dithered pixel art, the worst case for JPEG.
///
/// Decoding goes through `jpeg-decoder` on purpose. `image`'s decoder turns
/// perfectly good covers green (it does so on this app's own cache files, e.g.
/// 1000050.jpg), and re-encoding that would have destroyed them for good.
/// Even so, the result is decoded again and its average colour compared with
/// the source's: anything that drifts is rejected and the original is kept.
fn encode_cover(bytes: &[u8]) -> Option<Vec<u8>> {
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
    const QUALITY: u8 = 75;

    let (rgb, w, h) = decode_rgb(bytes)?;
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut enc = Encoder::new(&mut out, QUALITY);
    enc.set_sampling_factor(SamplingFactor::R_4_2_0);
    enc.set_optimized_huffman_tables(true);
    enc.set_progressive(true);
    enc.encode(&rgb, w, h, ColorType::Rgb).ok()?;

    let (check, cw, ch) = decode_rgb(&out)?;
    if (cw, ch) != (w, h) || mean_rgb(&rgb).iter().zip(mean_rgb(&check)).any(|(a, b)| (a - b).abs() > 12.0) {
        return None;
    }
    Some(out)
}

fn decode_rgb(bytes: &[u8]) -> Option<(Vec<u8>, u16, u16)> {
    let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    let px = d.decode().ok()?;
    let info = d.info()?;
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => Some((px, info.width, info.height)),
        jpeg_decoder::PixelFormat::L8 => {
            Some((px.iter().flat_map(|v| [*v, *v, *v]).collect(), info.width, info.height))
        }
        _ => None,
    }
}

fn mean_rgb(rgb: &[u8]) -> [f64; 3] {
    let n = (rgb.len() / 3).max(1) as f64;
    let mut sum = [0f64; 3];
    for px in rgb.chunks_exact(3) {
        for c in 0..3 {
            sum[c] += px[c] as f64;
        }
    }
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

/// One-time pass over covers cached before `encode_cover` existed.
///
/// New covers are written small; this brings the ones already on disk down to
/// the same size instead of waiting for the cache to churn. It runs in the
/// background, throttled, replaces a file only when the result is clearly
/// smaller (so a cover is never re-encoded twice for nothing), and writes to a
/// temp name first so a card reading the file never sees a half-written one.
/// Every cover can be re-downloaded, so the worst case of a failure is a
/// re-download. A marker file makes it run once.
fn recompress_image_cache() {
    let Ok(dir) = image_cache_dir() else { return };
    // Kept beside the cache, not in it: prune_image_cache deletes anything
    // that small, which would have this run again on every launch.
    let Some(marker) = dir.parent().map(|d| d.join("image_cache_recompressed_v1")) else { return };
    if marker.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&dir) else { return };

    let (mut done, mut saved) = (0u64, 0u64);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jpg") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        // prune_image_cache evicts oldest-modified first, so the rewrite must
        // not make every cover look brand new.
        let modified = entry.metadata().and_then(|m| m.modified()).ok();
        if let Some(out) = encode_cover(&bytes) {
            // Under 90% of the original, or leave it alone.
            if (out.len() as u64) * 10 < (bytes.len() as u64) * 9 {
                let tmp = path.with_extension("jpg.tmp");
                if std::fs::write(&tmp, &out).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                    if let (Some(t), Ok(f)) = (modified, std::fs::OpenOptions::new().write(true).open(&path)) {
                        let _ = f.set_modified(t);
                    }
                    done += 1;
                    saved += (bytes.len() - out.len()) as u64;
                } else {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
        }
        // Keep the disk and one core free for whoever is using the app.
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    let _ = std::fs::write(&marker, b"1");
    diag_log!(
        "[imagenes] Caché de portadas recomprimida: {} archivos, {} MB liberados.",
        done,
        saved / (1024 * 1024)
    );
}

/// The one place that knows where cached cover art lives.
fn image_cache_dir() -> Result<PathBuf, String> {
    Ok(dirs::data_dir()
        .ok_or("Cannot locate AppData".to_string())?
        .join("com.ragnarok.launcher")
        .join("image_cache"))
}

/// Files and bytes currently held in the image cache, for the Settings panel.
#[command]
fn image_cache_stats() -> Result<(usize, u64), String> {
    let dir = image_cache_dir()?;
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok((0, 0)) };
    let mut files = 0usize;
    let mut bytes = 0u64;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    Ok((files, bytes))
}

/// Empties the image cache. Returns the bytes freed.
///
/// Safe at any time: every image is re-downloadable, and get_cached_image_path
/// fetches whatever is missing the next time a card renders.
#[command]
fn clear_image_cache() -> Result<u64, String> {
    let dir = image_cache_dir()?;
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(0) };
    let mut freed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            freed += size;
        }
    }
    Ok(freed)
}

/// Trims the image cache down to `max_bytes`, dropping the least recently
/// modified files first. Returns the bytes freed.
///
/// A ceiling is what the cache never had: the only check before downloading
/// was whether the file already existed, so it grew to 3.1 GB for one user and
/// would have kept going with the catalog. Oldest-first is a reasonable proxy
/// for least-wanted here — a cover the user has not looked at in months costs
/// one small download to get back, and only if they scroll to it again.
fn prune_image_cache(max_bytes: u64) -> u64 {
    let Ok(dir) = image_cache_dir() else { return 0 };
    let Ok(entries) = std::fs::read_dir(&dir) else { return 0 };

    // Steam answers HTTP 200 with a flat grey "no art yet" JPEG of about
    // 1.4-1.9 KB. Before the download path learned to reject those, they were
    // cached permanently and the card showed grey forever. Sweeping them here
    // is what lets the read path above trust any file it finds, and costs one
    // small re-download for a game that has real art by now.
    const PLACEHOLDER_MAX: u64 = 2048;
    let mut freed = 0u64;

    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            if meta.len() <= PLACEHOLDER_MAX {
                let len = meta.len();
                if std::fs::remove_file(e.path()).is_ok() {
                    freed += len;
                }
                return None;
            }
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();

    let total: u64 = files.iter().map(|(_, len, _)| len).sum();

    // `freed` already counts the placeholders swept above; `trimmed` is the
    // separate business of respecting the ceiling, and `total` covers only the
    // files that survived, so the two must not be mixed in this comparison.
    let mut trimmed = 0u64;
    if total > max_bytes {
        // Oldest first, so the newest covers — the ones just looked at — survive.
        files.sort_by_key(|(modified, _, _)| *modified);

        for (_, len, path) in files {
            if total - trimmed <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                trimmed += len;
            }
        }
    }

    let total_freed = freed + trimmed;
    if total_freed > 0 {
        diag_log!(
            "[image_cache] Liberados {:.1} MB ({:.1} MB de placeholders, {:.1} MB por el tope).",
            total_freed as f64 / 1_048_576.0,
            freed as f64 / 1_048_576.0,
            trimmed as f64 / 1_048_576.0
        );
    }
    total_freed
}

#[command]
async fn get_cached_image_path(app_id: String) -> Result<String, String> {
    // Build a stable cache directory inside AppData/Roaming
    let cache_dir = dirs::data_dir()
        .ok_or("Cannot locate AppData".to_string())?
        .join("com.ragnarok.launcher")
        .join("image_cache");
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

    let file_path = cache_dir.join(format!("{}.jpg", app_id));

    // Already cached — hand back the path.
    //
    // This used to demand `len() > 8192`, the same threshold the download below
    // uses to reject Steam's grey "no art yet" placeholder. That worked while
    // the bytes written were the bytes downloaded. It stopped working the
    // moment write_cover_jpeg started re-encoding: a cover that arrives at
    // 9 KB is stored at roughly 41% of that, lands under the threshold, and is
    // then rejected on every single read — so the launcher re-downloaded it
    // from the CDN every time its card mounted, forever. A change made to save
    // bandwidth was quietly spending it without limit.
    //
    // Nothing is written here that has not already passed the placeholder
    // check, so the file's existence is the answer. Legacy placeholders cached
    // before that check existed are swept up by prune_image_cache at startup.
    if let Ok(metadata) = std::fs::metadata(&file_path) {
        if metadata.len() > 0 {
            return Ok(file_path.to_string_lossy().to_string());
        }
    }

    // Not cached — try multiple Steam CDN image formats in order
    let client = make_client()?;
    let cdn_candidates = [
        format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/header.jpg", app_id),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg", app_id),
        format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/capsule_616x353.jpg", app_id),
        format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/library_600x900.jpg", app_id),
        format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/library_hero.jpg", app_id),
        format!("https://steamcdn-a.akamaihd.net/steam/apps/{}/header.jpg", app_id),
    ];

    for url in &cdn_candidates {
        if let Ok(resp) = client.get(url.as_str()).send().await {
            if resp.status().is_success() {
                if let Ok(bytes) = resp.bytes().await {
                    // Steam serves a real, valid-looking JPEG (HTTP 200) even
                    // for games with no official art uploaded yet — a flat
                    // grayscale placeholder around 1.4-1.9KB (confirmed via
                    // Battlefield 6, whose header/capsule/library_600x900 all
                    // still return this exact placeholder). The old >1024
                    // threshold accepted it as "found an image" and cached it
                    // permanently, stopping before ever trying the later
                    // candidates — library_hero.jpg had a real 256KB image
                    // for the same game. Raised well above the placeholder's
                    // size so a small-but-genuine image is still vanishingly
                    // unlikely to be rejected, while placeholders reliably
                    // fall through to the next candidate.
                    if bytes.len() > 8192 {
                        write_cover_jpeg(&file_path, &bytes);
                        return Ok(file_path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    // Last resort: ask Steam appdetails for the actual header_image URL
    let details_url = format!(
        "https://store.steampowered.com/api/appdetails?appids={}&filters=basic",
        app_id
    );
    // Newer games keep their art behind a hashed path
    // (`.../apps/<id>/<hash>/header.jpg`), so none of the fixed URLs above can
    // ever find it — Control Resonant (3669870) answers 404 on every one of
    // them. Only the store knows that path, and the entry it answers with may
    // be keyed by another id, which is what made this fall through and leave
    // the card showing initials.
    if let Ok(resp) = client.get(&details_url).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            let entry = appdetails_entry(&json, &app_id);
            let urls: Vec<String> = ["header_image", "capsule_image", "capsule_imagev5"]
                .iter()
                .filter_map(|key| entry.and_then(|e| e["data"][*key].as_str()).map(String::from))
                .collect();
            for img_url in urls {
                if let Ok(img_resp) = client.get(&img_url).send().await {
                    if img_resp.status().is_success() {
                        if let Ok(bytes) = img_resp.bytes().await {
                            // Same placeholder guard as the loop above: Steam
                            // answers 200 with a ~1.5KB grey placeholder for
                            // games with no art uploaded yet.
                            if bytes.len() > 8192 {
                                write_cover_jpeg(&file_path, &bytes);
                                return Ok(file_path.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    Err(format!("No image found for app {}", app_id))
}

#[command]
async fn preload_images(app_handle: tauri::AppHandle, app_ids: Vec<String>) -> Result<(), String> {
    use tokio::sync::Semaphore;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let cache_dir = dirs::data_dir()
        .ok_or("Cannot locate AppData")?
        .join("com.ragnarok.launcher")
        .join("image_cache");
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

    // Only download images not already cached on disk
    let to_download: Vec<String> = app_ids.into_iter()
        .filter(|id| !cache_dir.join(format!("{}.jpg", id)).exists())
        .collect();

    let total = to_download.len();
    if total == 0 {
        app_handle.emit_all("image_preload_progress", serde_json::json!({ "done": 0, "total": 0 })).ok();
        return Ok(());
    }

    let completed = Arc::new(AtomicUsize::new(0));
    // Increase concurrency from 8 to 12 to download images faster
    let semaphore = Arc::new(Semaphore::new(12));
    let client = make_client()?;

    let handles: Vec<_> = to_download.into_iter().map(|app_id| {
        let sem = semaphore.clone();
        let cache_dir = cache_dir.clone();
        let app_handle = app_handle.clone();
        let completed = completed.clone();
        let client = client.clone();

        tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            let file_path = cache_dir.join(format!("{}.jpg", app_id));

            let cdn_candidates = [
                format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/header.jpg", app_id),
                format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg", app_id),
                format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/capsule_616x353.jpg", app_id),
                format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/library_600x900.jpg", app_id),
            ];

            for url in &cdn_candidates {
                if let Ok(resp) = client.get(url.as_str()).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            // Steam serves a real, valid-looking JPEG (HTTP 200) even
                    // for games with no official art uploaded yet — a flat
                    // grayscale placeholder around 1.4-1.9KB (confirmed via
                    // Battlefield 6, whose header/capsule/library_600x900 all
                    // still return this exact placeholder). The old >1024
                    // threshold accepted it as "found an image" and cached it
                    // permanently, stopping before ever trying the later
                    // candidates — library_hero.jpg had a real 256KB image
                    // for the same game. Raised well above the placeholder's
                    // size so a small-but-genuine image is still vanishingly
                    // unlikely to be rejected, while placeholders reliably
                    // fall through to the next candidate.
                    if bytes.len() > 8192 {
                                write_cover_jpeg(&file_path, &bytes);
                                break;
                            }
                        }
                    }
                }
            }

            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            // Throttle: only emit every 5 completions or on the last image
            // This reduces React re-renders from N to N/5
            if done % 5 == 0 || done == total {
                app_handle.emit_all("image_preload_progress", serde_json::json!({
                    "done": done,
                    "total": total
                })).ok();
            }
        })
    }).collect();

    futures::future::join_all(handles).await;
    Ok(())
}

// --- Discord Rich Presence ---

#[command]
#[allow(dead_code)]
fn set_discord_presence(
    state: tauri::State<'_, DiscordState>,
    game_name: String,
) -> Result<(), String> {
    let mut lock = state.0.lock().map_err(|e| e.to_string())?;

    if lock.is_none() {
        let mut client = match DiscordIpcClient::new(DISCORD_CLIENT_ID) {
            Ok(c) => c,
            Err(_) => { return Ok(()); }
        };
        match client.connect() {
            Ok(_) => { *lock = Some(client); }
            Err(_) => { return Ok(()); } // Discord not running — fail silently
        }
    }

    if let Some(client) = lock.as_mut() {
        let start = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let _ = client.set_activity(
            activity::Activity::new()
                .details(&format!("Playing {}", game_name))
                .state("via Ragnarok Launcher")
                .timestamps(activity::Timestamps::new().start(start)),
        );
    }

    Ok(())
}

#[command]
#[allow(dead_code)]
fn clear_discord_presence(
    state: tauri::State<'_, DiscordState>,
) -> Result<(), String> {
    let mut lock = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(client) = lock.as_mut() {
        let _ = client.close();
    }
    *lock = None;
    Ok(())
}

// --- Support ticket two-way replies (via a Discord bot token, no server of
// our own needed) ---
// The app already posts tickets to a Discord webhook (frontend, SupportView).
// After that post, the frontend calls this to spin the message into its own
// thread, then periodically polls discord_check_thread_replies for that
// thread so a developer's reply in Discord shows up back inside the app.
#[derive(Serialize)]
struct SupportTicketReply {
    id: String,
    author: String,
    content: String,
    timestamp: String,
    attachments: Vec<String>,
}

/// Lets the frontend put a line in the same diagnostics file the backend
/// writes to.
///
/// Added because the webview's `console.error` goes nowhere in a release
/// build — the exact problem `diag.rs` was written to solve for Rust's old
/// `eprintln!` calls. A frontend failure the user cannot see and support
/// cannot read is a failure nobody ever learns about.
#[command]
async fn log_diagnostic(message: String) -> Result<(), String> {
    // Capped so a loop in the UI cannot write the log file full of one line.
    let trimmed: String = message.chars().take(1000).collect();
    diag_log!("[ui] {}", trimmed);
    Ok(())
}

#[command]
async fn discord_create_support_thread(
    channel_id: String,
    message_id: String,
    thread_name: String,
) -> Result<String, String> {
    let client = make_discord_bot_client()?;
    let url = format!(
        "{}/channels/{}/messages/{}/threads",
        DISCORD_API_BASE, channel_id, message_id
    );
    let name: String = thread_name.chars().take(100).collect();
    let res = client
        .post(&url)
        .json(&serde_json::json!({ "name": name, "auto_archive_duration": 1440 }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Discord API error {}: {}", status, body));
    }
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    data["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "No se recibió el ID del hilo.".to_string())
}

#[derive(Serialize)]
struct SupportThreadCheck {
    replies: Vec<SupportTicketReply>,
    archived: bool,
}

#[command]
async fn discord_check_thread_replies(
    thread_id: String,
    after_message_id: Option<String>,
) -> Result<SupportThreadCheck, String> {
    let client = make_discord_bot_client()?;

    // Archiving a thread in Discord (the native "Archive Thread" action) is
    // the developer's own signal that a ticket is resolved — surfaced here
    // so the app can close it locally without needing any in-app action.
    let archived = {
        let info_url = format!("{}/channels/{}", DISCORD_API_BASE, thread_id);
        match client
            .get(&info_url)
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => res
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v["thread_metadata"]["archived"].as_bool())
                .unwrap_or(false),
            _ => false,
        }
    };

    let mut url = format!(
        "{}/channels/{}/messages?limit=20",
        DISCORD_API_BASE, thread_id
    );
    if let Some(after) = after_message_id.filter(|s| !s.is_empty()) {
        url.push_str(&format!("&after={}", after));
    }

    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    // Thread archived/deleted or bot lacking access shouldn't be a hard
    // error shown to the user for a background poll — just report no replies.
    if !res.status().is_success() {
        return Ok(SupportThreadCheck { replies: Vec::new(), archived });
    }
    let data: Vec<serde_json::Value> = res.json().await.map_err(|e| e.to_string())?;

    let mut replies: Vec<SupportTicketReply> = data
        .iter()
        .filter(|m| {
            // Skip the original webhook-posted ticket message and anything
            // else posted by a bot/webhook — only real developer replies.
            m["webhook_id"].as_str().is_none() && !m["author"]["bot"].as_bool().unwrap_or(false)
        })
        .map(|m| SupportTicketReply {
            id: m["id"].as_str().unwrap_or_default().to_string(),
            author: m["author"]["username"].as_str().unwrap_or("Soporte").to_string(),
            content: m["content"].as_str().unwrap_or_default().to_string(),
            timestamp: m["timestamp"].as_str().unwrap_or_default().to_string(),
            attachments: m["attachments"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a["url"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();

    // A developer typing the literal text "/stops" in the thread is this
    // app's stand-in for a real Discord slash command — Discord slash
    // commands need a server listening 24/7 (Gateway or a public Interactions
    // endpoint), which this desktop app can't provide since it only runs
    // while a user has it open. This endpoint is already polled every few
    // seconds while the app is open, so a plain message works the same way
    // in practice: it actually archives the thread (so the native
    // archived-flag check above reflects it on every future poll, from any
    // client — not just something tracked locally), and is stripped out of
    // `replies` so the end user never sees the raw command as if it were a
    // real reply.
    let stops_requested = replies.iter().any(|r| r.content.trim().eq_ignore_ascii_case("/stops"));
    if stops_requested {
        replies.retain(|r| !r.content.trim().eq_ignore_ascii_case("/stops"));
        let _ = discord_archive_thread(thread_id.clone()).await;
    }
    let archived = archived || stops_requested;

    // Discord returns messages newest-first regardless of before/after — flip
    // to chronological order for display.
    replies.reverse();
    Ok(SupportThreadCheck { replies, archived })
}

#[derive(Serialize)]
struct SentSupportReply {
    id: String,
    timestamp: String,
    attachments: Vec<String>,
}

#[derive(Serialize)]
struct SentSupportMessage {
    id: String,
    channel_id: String,
}

/// Posts the FIRST support report (the one that spins up a new Discord
/// thread), with optional image/file attachments — this used to only be a
/// plain `fetch()` from the frontend straight to the webhook URL with a
/// JSON body, which meant there was never any way to attach a screenshot to
/// the very first report (only replies to an already-open ticket could,
/// via discord_send_thread_reply above). Reported by a user who specifically
/// wanted to attach a screenshot of the bug they were describing and found
/// no way to. Discord webhooks support file uploads the same way the bot API
/// does — multipart/form-data with a payload_json field — so this mirrors
/// discord_send_thread_reply's approach instead of duplicating the embed
/// construction in Rust: the frontend builds the exact same embed JSON it
/// always did and just hands it over as a string.
/// Whether a URL really points at Discord's webhook API.
///
/// Parsed rather than pattern-matched: `https://evil.com/?x=discord.com/api/`
/// contains the right text, and `https://discord.com.evil.com/` starts with a
/// convincing prefix. Only the parsed host counts.
fn is_discord_webhook_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else { return false };
    matches!(host, "discord.com" | "discordapp.com" | "ptb.discord.com" | "canary.discord.com")
        && parsed.path().starts_with("/api/webhooks/")
}

#[command]
async fn discord_send_support_webhook(
    webhook_url: String,
    payload_json: String,
    attachment_paths: Vec<String>,
) -> Result<SentSupportMessage, String> {
    // The destination has to be Discord, and nothing else.
    //
    // `webhook_url` arrived straight from the frontend and was used unchecked,
    // while `attachment_paths` was read with `tokio::fs::read` on any path
    // given. Together that is "read an arbitrary file and POST it to an
    // arbitrary host" exposed as an IPC command — so any script that reaches
    // the webview (this app renders publisher-supplied HTML from Steam) could
    // ship the user's Google Drive tokens to a server of its choosing, or use
    // the app to probe 127.0.0.1.
    //
    // Pinning the host does not restrict any real use: the only caller posts
    // to the project's own Discord webhook.
    if !is_discord_webhook_url(&webhook_url) {
        return Err("Destino no permitido: los reportes solo se envían a Discord.".to_string());
    }

    let client = make_client()?;
    let url = format!("{}?wait=true", webhook_url);

    // Files are renamed to something Discord can reference from an embed
    // (`attachment://name`) — a screenshot called "Captura de pantalla (3).png"
    // cannot be. The first image is what the report's embed displays inline,
    // via the `__FIRST_IMAGE__` placeholder the frontend leaves in the payload,
    // so the screenshot sits inside the report instead of floating above it.
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut first_image: Option<String> = None;
    for (i, path) in attachment_paths.iter().enumerate() {
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| format!("No se pudo leer el archivo '{}': {}", path, e))?;
        let p = std::path::Path::new(&path);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp");
        let filename = if is_image {
            format!("captura_{}.{}", i + 1, ext)
        } else {
            let original = p
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("archivo_{}", i + 1));
            original
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
                .collect()
        };
        if is_image && first_image.is_none() {
            first_image = Some(filename.clone());
        }
        files.push((filename, bytes));
    }
    let payload_json = payload_json.replace("__FIRST_IMAGE__", first_image.as_deref().unwrap_or(""));

    let res = if files.is_empty() {
        client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(payload_json)
            .send()
            .await
            .map_err(|e| e.to_string())?
    } else {
        let mut form = reqwest::multipart::Form::new().text("payload_json", payload_json);

        for (i, (filename, bytes)) in files.into_iter().enumerate() {
            let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
            form = form.part(format!("files[{i}]"), part);
        }

        client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())?
    };

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Discord API error {}: {}", status, body));
    }

    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(SentSupportMessage {
        id: data["id"].as_str().unwrap_or_default().to_string(),
        channel_id: data["channel_id"].as_str().unwrap_or_default().to_string(),
    })
}

/// Posts the user's own follow-up (text and/or image attachments) into their
/// support thread, so the conversation is two-way from the app itself instead
/// of requiring the user to go open Discord. Sent as the bot account — that's
/// also why discord_check_thread_replies filters out bot/webhook messages
/// when looking for "new developer replies": otherwise the app would notify
/// itself about its own just-sent message on the next poll.
#[command]
async fn discord_send_thread_reply(
    thread_id: String,
    content: String,
    attachment_paths: Vec<String>,
) -> Result<SentSupportReply, String> {
    let client = make_discord_bot_client()?;
    let url = format!("{}/channels/{}/messages", DISCORD_API_BASE, thread_id);

    let res = if attachment_paths.is_empty() {
        client
            .post(&url)
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(|e| e.to_string())?
    } else {
        let mut form = reqwest::multipart::Form::new()
            .text("payload_json", serde_json::json!({ "content": content }).to_string());

        for (i, path) in attachment_paths.iter().enumerate() {
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|e| format!("No se pudo leer la imagen '{}': {}", path, e))?;
            let filename = std::path::Path::new(path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("image{i}.png"));
            let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
            form = form.part(format!("files[{i}]"), part);
        }

        client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())?
    };

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Discord API error {}: {}", status, body));
    }

    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(SentSupportReply {
        id: data["id"].as_str().unwrap_or_default().to_string(),
        timestamp: data["timestamp"].as_str().unwrap_or_default().to_string(),
        attachments: data["attachments"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|a| a["url"].as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default(),
    })
}

/// Lets the user mark their own ticket as resolved from the app, instead of
/// only being able to close it by waiting for the developer to archive the
/// Discord thread. The bot created the thread, so it has permission to
/// archive it directly.
#[command]
async fn discord_archive_thread(thread_id: String) -> Result<(), String> {
    let client = make_discord_bot_client()?;
    let url = format!("{}/channels/{}", DISCORD_API_BASE, thread_id);
    let res = client
        .patch(&url)
        .json(&serde_json::json!({ "archived": true, "locked": true }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Discord API error {}: {}", status, body));
    }
    Ok(())
}

// ── "/RAGNAROK LEGENDS" — a text stand-in for a real Discord slash command ──
// This app can't host a real slash command (needs a server listening 24/7,
// which a desktop app that's only sometimes open can't provide — see the
// same reasoning on "/stops" above). Instead: a dedicated Discord channel
// (created specifically for this) gets polled the same way ticket replies
// are, for plain messages starting with "/RAGNAROK LEGENDS <game name or
// AppID>". The list itself lives in a JSON file on GitHub instead of being
// compiled into the app, so adding a game doesn't need a new app release —
// every user's launcher fetches that file directly (raw.githubusercontent.com
// is already in the CSP's connect-src).
//
// The GitHub write token never goes through Settings/localStorage and is
// never compiled into the app (a token shipped inside the binary would be
// extractable from every single user's copy — a much bigger blast radius
// than the read-only Discord bot token already hardcoded elsewhere in this
// file). Instead it's set ONCE via a hidden CLI flag
// (`Ragnarok-launcher.exe --set-legends-token <token>`, handled at the very
// top of main() before Tauri even starts) that encrypts it (AES-256-GCM)
// into a file under this PC's own AppData folder. has_legends_token() is a
// cheap existence check the frontend uses to decide whether to even start
// polling — for every other user (no file present) this whole feature never
// makes a single network call.
fn legends_token_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("legends_token.enc"))
}

// Fixed passphrase, not a real secret — this only obscures the token at
// rest against something casually browsing this PC's AppData folder (e.g.
// malware scraping for plaintext credentials). It does NOT protect against
// someone with the skill to reverse-engineer this binary, but that's an
// acceptable tradeoff here: unlike a token embedded directly in the binary,
// this key alone is useless without also having the encrypted file, which
// only exists on the one PC that ran the CLI flag above.
fn legends_token_key() -> aes_gcm::Key<aes_gcm::Aes256Gcm> {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(b"ragnarok-legends-token-v1");
    *aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(&hash)
}

fn read_legends_token() -> Option<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    let path = legends_token_path()?;
    let data = std::fs::read(&path).ok()?;
    if data.len() < 12 {
        return None;
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher = aes_gcm::Aes256Gcm::new(&legends_token_key());
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext).ok()?;
    String::from_utf8(plaintext).ok()
}

fn write_legends_token(token: &str) -> Result<(), String> {
    use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
    let path = legends_token_path().ok_or("No se pudo resolver AppData")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let cipher = aes_gcm::Aes256Gcm::new(&legends_token_key());
    let nonce = aes_gcm::Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, token.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ciphertext);
    std::fs::write(&path, out).map_err(|e| e.to_string())
}

/// `strip_prefix`, case-insensitively, without ever slicing mid-character.
///
/// Both the prefix and the comparison stay in ASCII, so only the boundary
/// matters — and taking it from the prefix's own byte length is safe precisely
/// because a match means those bytes were ASCII.
fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

/// The largest index at or below `idx` that is a character boundary.
///
/// Rust's own `floor_char_boundary` is still unstable. Slicing a byte offset
/// that a regex or an arithmetic window produced is a panic waiting for the
/// first non-ASCII byte, and on this codebase that panic takes down the async
/// task and hangs the caller's promise rather than surfacing an error.
fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
const RAGNAROK_LEGENDS_REPO_OWNER: &str = "RagnarokManifests";
const RAGNAROK_LEGENDS_REPO_NAME: &str = "games";

// ── Ragnarok's own GitHub-hosted ticket cache ───────────────────────────────
// install_game normally builds a game's .lua + .manifest tickets fresh every
// time from SteamCMD's live API + Ryuu's ticket generator — both third-party
// services outside Ragnarok's control, so an install fails if either is down
// or rate-limited. This mirrors the exact same pattern already built for
// Ragnarok Legends: a small per-app zip bundle stored in the same GitHub
// repo, read by every user (no token needed, plain raw.githubusercontent.com
// fetch), written only from whichever PC has the GitHub token configured
// (reused from Legends — same repo, same permission scope, nothing new to
// set up). Every successful non-cached install backfills the cache for that
// game, so it grows organically as games get installed instead of needing a
// one-time bulk export of the whole catalog.
const LUA_CACHE_REPO_PATH_PREFIX: &str = "lua-cache";

/// Fetches `lua-cache/<appid>.zip` from GitHub if present — a zip containing
/// `<appid>.lua` at its root and any depot `.manifest` files under
/// `depotcache/`. None on any miss (404, network failure): install_game
/// always falls through to the normal SteamCMD/Ryuu flow in that case, this
/// is purely a fast path, never a hard dependency.
async fn try_fetch_lua_cache(app_id: &str) -> Option<Vec<u8>> {
    let client = crate::make_client().ok()?;
    let url = format!(
        "https://raw.githubusercontent.com/{}/{}/main/{}/{}.zip",
        RAGNAROK_LEGENDS_REPO_OWNER, RAGNAROK_LEGENDS_REPO_NAME, LUA_CACHE_REPO_PATH_PREFIX, app_id
    );
    if !download_guard::is_allowed_url(&url, GITHUB_HOSTS) {
        return None;
    }
    let res = client.get(&url).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let bytes = res.bytes().await.ok()?;

    // The contents of this land in Steam's own config/lua and depotcache. A
    // 404 page served as a zip would fail at ZipArchive::new anyway, but the
    // caller treats every failure here as "no fast path available" and moves
    // on silently — so the reason is worth printing rather than losing.
    if let Err(e) = download_guard::verify_download(
        "el paquete de manifiestos",
        &bytes,
        download_guard::Payload::Zip,
        None,
    ) {
        diag_log!("[lua-cache] {}", e);
        return None;
    }
    Some(bytes.to_vec())
}

/// Unzips a ticket bundle (see try_fetch_lua_cache) into the same
/// destinations install_game's own SteamCMD/Ryuu path writes to — `.lua` to
/// `config/lua/`, everything under `depotcache/` to Steam's own
/// `depotcache/`. Returns the set of depot ids it wrote (parsed from each
/// `<depotid>_<gid>.manifest` filename), which install_game treats exactly
/// like Ryuu ticket depot ids for the rest of its existing logic.
fn apply_lua_cache_bundle(bytes: &[u8], steam_path: &str, app_id: &str) -> Result<HashSet<String>, String> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;
    let lua_dir = std::path::PathBuf::from(steam_path).join("config").join("lua");
    let depotcache = std::path::PathBuf::from(steam_path).join("depotcache");
    std::fs::create_dir_all(&lua_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&depotcache).map_err(|e| e.to_string())?;

    let mut depot_ids = HashSet::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        let dest = if name == format!("{}.lua", app_id) {
            lua_dir.join(&name)
        } else if let Some(rest) = name.strip_prefix("depotcache/") {
            if let Some(depot_id) = rest.split('_').next() {
                if depot_id.chars().all(|c| c.is_ascii_digit()) {
                    depot_ids.insert(depot_id.to_string());
                }
            }
            depotcache.join(rest)
        } else {
            continue; // Unexpected shape inside the bundle — skip rather than write somewhere unintended.
        };
        let mut buf = Vec::new();
        std::io::copy(&mut entry, &mut buf).map_err(|e| e.to_string())?;
        if dest.extension().and_then(|ext| ext.to_str()) == Some("manifest") {
            crate::managers::manifest_vault::write_protected_file(&dest, &buf).map_err(|e| e.to_string())?;
        } else {
            std::fs::write(&dest, &buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(depot_ids)
}

// ── Static / pinned versions ("versiones ancladas") ─────────────────────────
// A curated, per-game .lua ticket the RagnarokManifests maintainers pin to a
// specific BuildID (baked into its own setManifestid lines) instead of
// whatever SteamCMD/Ryuu would generate live at install time — for titles
// where the current build is broken, has worse DRM than an older one, etc.
// Just the raw .lua, nothing else: depot keys and manifest ids are already
// embedded in the file, so unlike the general lua-cache fast path above
// there's no separate zip/manifest bundle to unpack.
const STATIC_VERSIONS_REPO_PATH: &str = "versiones ancladas";

/// Lists every AppID that has a pinned .lua published in
/// `<STATIC_VERSIONS_REPO_PATH>/`, straight from GitHub's Contents API for
/// that one folder — so the gallery can show every static version that
/// EXISTS, not just the ones this particular PC already downloaded (that
/// used to be `pinned_manifests.json`, which is empty on a fresh install and
/// left the gallery looking broken/empty for a first-time user). Empty
/// vec on any failure (network error, folder missing) rather than an error,
/// since this only feeds a UI listing — no reason to hard-fail the whole
/// tab over it.
#[command]
/// Last good answer from the listing, so a throttled API is not the same as
/// an empty shelf.
fn static_versions_cache() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("static_versions.json"))
}

fn read_static_versions_cache() -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(static_versions_cache()?).ok()?;
    let ids: Vec<String> = serde_json::from_str(&raw).ok()?;
    (!ids.is_empty()).then_some(ids)
}

fn write_static_versions_cache(ids: &[String]) {
    let Some(path) = static_versions_cache() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string(ids) {
        let _ = std::fs::write(path, body);
    }
}

/// The app ids that have a pinned build published.
///
/// Every failure used to come back as an empty list, so "todavía no hay
/// versiones estáticas publicadas" was equally what the user saw when the
/// folder held five of them and GitHub had simply throttled us. The Contents
/// API allows 60 unauthenticated calls an hour *per IP*, which this app shares
/// with its catalog sync, its update check and its emulator lookups — running
/// out is routine, not exceptional.
///
/// So: the CDN is asked first (raw.githubusercontent is not rate limited, and
/// serves an index file if the repo carries one), the API is the fallback, and
/// the last good answer is kept on disk for when neither can be reached. An
/// error is only returned when there is genuinely nothing to show, and it says
/// what went wrong rather than pretending the shelf is empty.
#[command]
async fn list_static_versions() -> Result<Vec<String>, String> {
    let client = crate::make_client().map_err(|e| e.to_string())?;

    // 1. index.json over the CDN, if the repo publishes one.
    let index_url = format!(
        "https://raw.githubusercontent.com/{}/{}/main/{}/index.json",
        RAGNAROK_LEGENDS_REPO_OWNER,
        RAGNAROK_LEGENDS_REPO_NAME,
        urlencoding::encode(STATIC_VERSIONS_REPO_PATH)
    );
    if let Ok(res) = client.get(&index_url).send().await {
        if res.status().is_success() {
            if let Ok(ids) = res.json::<Vec<String>>().await {
                if !ids.is_empty() {
                    write_static_versions_cache(&ids);
                    return Ok(ids);
                }
            }
        }
    }

    // 2. The Contents API.
    let url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}",
        RAGNAROK_LEGENDS_REPO_OWNER,
        RAGNAROK_LEGENDS_REPO_NAME,
        urlencoding::encode(STATIC_VERSIONS_REPO_PATH)
    );
    let failure = match client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => match res.json::<Vec<serde_json::Value>>().await {
            Ok(entries) => {
                let ids: Vec<String> = entries
                    .iter()
                    .filter_map(|e| e["name"].as_str())
                    .filter_map(|name| name.strip_suffix(".lua"))
                    .map(|s| s.to_string())
                    .collect();
                write_static_versions_cache(&ids);
                return Ok(ids);
            }
            Err(e) => format!("respuesta ilegible de GitHub: {}", e),
        },
        Ok(res) if res.status().as_u16() == 403 || res.status().as_u16() == 429 => {
            "GitHub limitó las peticiones desde tu conexión. Se reintenta solo en un rato."
                .to_string()
        }
        Ok(res) => format!("GitHub respondió {}", res.status().as_u16()),
        Err(e) => format!("sin conexión con GitHub: {}", e),
    };

    // 3. Whatever worked last time.
    match read_static_versions_cache() {
        Some(ids) => Ok(ids),
        None => Err(failure),
    }
}

/// Fetches `<STATIC_VERSIONS_REPO_PATH>/<appid>.lua` from GitHub if present.
/// None on any miss (404, network failure, no pinned version for this game).
async fn fetch_static_version_lua(app_id: &str) -> Option<String> {
    let client = crate::make_client().ok()?;
    let url = format!(
        "https://raw.githubusercontent.com/{}/{}/main/{}/{}.lua",
        RAGNAROK_LEGENDS_REPO_OWNER,
        RAGNAROK_LEGENDS_REPO_NAME,
        urlencoding::encode(STATIC_VERSIONS_REPO_PATH),
        app_id
    );
    let res = client.get(&url).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    res.text().await.ok()
}

#[derive(serde::Serialize)]
struct StaticVersionInfo {
    build_id: Option<String>,
    /// The human patch/version number from the header's "Patch Notes
    /// Version X.Y.Z" comment, trimmed to "X.Y" (drops a trailing ".0"/".00"
    /// patch segment — RagnarokManifests' own convention for a version with
    /// no hotfix number yet). None if the header doesn't have that comment.
    version_label: Option<String>,
}

/// Pulls the BuildID and human patch version out of the pinned .lua's own
/// header comment (e.g. "-- Version-locked to Build 24236217 (Patch Notes
/// Version 1.14.00) ...") for display in the editor/gallery — the file is
/// the single source of truth, no separate metadata sidecar.
fn parse_static_version_info(lua_text: &str) -> StaticVersionInfo {
    let build_id = regex::Regex::new(r"Build\s+(\d+)")
        .ok()
        .and_then(|re| re.captures(lua_text))
        .map(|c| c[1].to_string());
    let version_label = regex::Regex::new(r"Patch Notes Version\s+(\d+\.\d+)(?:\.\d+)?")
        .ok()
        .and_then(|re| re.captures(lua_text))
        .map(|c| c[1].to_string());
    StaticVersionInfo { build_id, version_label }
}

/// Manual "Descargar" action from the Static Versions editor: pulls the
/// curated, version-pinned .lua for `app_id` from RagnarokManifests/games'
/// "versiones ancladas" folder and drops it straight into config/lua/ —
/// self-contained (depot keys + a fixed setManifestid per depot already
/// baked in), so nothing else needs writing. Returns the BuildID/version
/// parsed from the file's own header, if present.
#[command]
async fn download_static_version(steam_path: String, app_id: String) -> Result<StaticVersionInfo, String> {
    let lua_text = fetch_static_version_lua(&app_id)
        .await
        .ok_or_else(|| "Todavía no hay una versión estática publicada para este juego.".to_string())?;

    let lua_dir = std::path::PathBuf::from(&steam_path).join("config").join("lua");
    std::fs::create_dir_all(&lua_dir).map_err(|e| e.to_string())?;
    std::fs::write(lua_dir.join(format!("{}.lua", app_id)), &lua_text).map_err(|e| e.to_string())?;

    // Same sidecar registration install_game does (its Step 4/5) — without
    // this the game never shows up in the Library and "Desinstalar" has
    // nothing to offer, even though Steam now has a valid ticket for it.
    let mut apps = load_apps_map(&steam_path);
    apps.entry(app_id.clone()).or_insert_with(Vec::new);
    save_apps_map(&steam_path, &apps);
    rebuild_steam_tools(&steam_path).map_err(|e| format!("SteamTools update failed: {}", e))?;

    // Pin it: record that this build was deliberately version-locked, so
    // the Static Versions gallery can show the "FIJO" badge.
    let mut exclusions = load_manifest_exclusions();
    exclusions.insert(app_id.clone());
    if let Err(e) = save_manifest_exclusions(&exclusions) {
        diag_log!("[pins] No se pudo anclar {}: {}", app_id, e);
    }


    Ok(parse_static_version_info(&lua_text))
}

/// Read-only lookup for the gallery/editor header: what BuildID/version (if
/// any) RagnarokManifests has pinned for `app_id`, without downloading or
/// applying anything. Lets the editor show a game's known static version
/// immediately when it's opened, before the user ever presses Descargar.
#[command]
async fn get_static_version_info(app_id: String) -> Option<StaticVersionInfo> {
    let lua_text = fetch_static_version_lua(&app_id).await?;
    Some(parse_static_version_info(&lua_text))
}

// ── Switch emulators ───────────────────────────────────────────────────────
// See managers/emulators.rs for the scope note: emulator binaries, placing
// keys the user already has, and listing games already on disk with cover
// art. Nothing fetches keys, firmware or game files.

#[command]
async fn list_emulators(english: Option<bool>) -> Vec<managers::emulators::EmulatorInfo> {
    let mut list = managers::emulators::list_emulators().await;
    if english.unwrap_or(false) {
        for e in &mut list {
            e.description = managers::emulators::description_for(&e.id, true);
        }
    }
    list
}

#[command]
async fn install_emulator(app: tauri::AppHandle, emulator_id: String) -> Result<String, String> {
    managers::emulators::install_emulator(Some(&app), &emulator_id).await
}

#[command]
async fn uninstall_emulator(emulator_id: String) -> Result<(), String> {
    managers::emulators::uninstall_emulator(&emulator_id)
}

#[command]
async fn launch_emulator(emulator_id: String) -> Result<(), String> {
    managers::emulators::launch_emulator(&emulator_id)
}

#[command]
async fn open_emulator_folder(emulator_id: String) -> Result<(), String> {
    managers::emulators::open_install_folder(&emulator_id)
}

/// Copies a prod.keys/title.keys the user already has into the folder the
/// chosen emulator reads keys from. Nothing is downloaded.
#[command]
async fn install_switch_keys(emulator_id: String, source_path: String) -> Result<String, String> {
    managers::emulators::install_keys(&emulator_id, &source_path)
}

/// Installs a firmware ZIP the user already has into the chosen emulator.
/// Nothing is downloaded.
#[command]
async fn install_switch_firmware(emulator_id: String, source_path: String) -> Result<String, String> {
    managers::emulators::install_firmware(&emulator_id, &source_path)
}

/// Downloads prod.keys + title.keys automatically from prodkeys.net and
/// installs them into the correct folder for the chosen emulator.
/// The ZIP is fetched in memory — no file picker, no temp files left behind.
#[command]
async fn download_and_install_keys(emulator_id: String) -> Result<String, String> {
    use std::io::Read;

    // Latest known direct-download link from https://prodkeys.net/yuzu-prod-keys-update-5/
    const KEYS_URL: &str = "https://files.prodkeys.net/ProdKeys.NET-v22.5.0.zip";

    if !download_guard::is_allowed_url(KEYS_URL, &["files.prodkeys.net"]) {
        return Err("La URL de las keys no apunta a prodkeys.net.".to_string());
    }

    let client = make_client()?;
    let bytes = client
        .get(KEYS_URL)
        .send()
        .await
        .map_err(|e| format!("No se pudo descargar las keys: {}", e))?
        .error_for_status()
        .map_err(|e| format!("El servidor rechazó la descarga: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Error leyendo la respuesta: {}", e))?;

    // prodkeys.net is a single third-party host with no mirror and no
    // published checksum. What it serves can at least be confirmed to be a
    // ZIP and not the interstitial/ad page the site is otherwise full of,
    // which is the failure users would hit here.
    download_guard::verify_download("las keys", &bytes, download_guard::Payload::Zip, None)?;

    let cursor = std::io::Cursor::new(bytes.as_ref());
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| format!("El ZIP descargado no es válido: {}", e))?;

    // Collect prod.keys and title.keys from the archive
    let mut key_files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() { continue; }
        let raw_name = entry.name().to_lowercase();
        let fname = std::path::Path::new(&raw_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&raw_name)
            .to_string();
        if fname == "prod.keys" || fname == "title.keys" {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            key_files.push((fname, buf));
        }
    }

    if key_files.is_empty() {
        return Err("No se encontró prod.keys ni title.keys en el ZIP descargado.".to_string());
    }

    managers::emulators::install_keys_from_bytes(&emulator_id, &key_files)
}

#[command]
async fn scan_switch_games(folder: String) -> Result<Vec<managers::emulators::SwitchGame>, String> {
    managers::emulators::scan_games(&folder)
}

/// Scans every fixed drive for Switch game files, so the user doesn't have to
/// know or pick where they are. Blocking work, moved off the async runtime.
#[command]
async fn autodetect_switch_games() -> Result<Vec<managers::emulators::SwitchGame>, String> {
    tokio::task::spawn_blocking(managers::emulators::autodetect_games)
        .await
        .map_err(|e| e.to_string())
}

/// Adds a folder to the emulator's own game-directory list so its games show
/// up inside the emulator without manual configuration.
#[command]
async fn register_games_dir(emulator_id: String, folder: String) -> Result<String, String> {
    managers::emulators::register_games_dir(&emulator_id, &folder)
}

#[command]
async fn fetch_switch_cover(name: String) -> Option<String> {
    let client = make_client().ok()?;
    managers::emulators::fetch_cover(&client, &name).await
}

#[command]
async fn fetch_switch_online_catalog(
    source: String,
    query: Option<String>,
    page: Option<usize>,
    per_page: Option<usize>,
) -> Result<Vec<managers::switch_catalog::SwitchCatalogItem>, String> {
    let client = make_client()?;
    managers::switch_catalog::fetch_catalog(
        &client,
        &source,
        query,
        page.unwrap_or(1),
        per_page.unwrap_or(24),
    )
    .await
}

#[command]
#[allow(dead_code)]
async fn fetch_switch_game_details(
    source: String,
    page_url: String,
) -> Result<managers::switch_catalog::SwitchGameDetails, String> {
    let client = make_client()?;
    managers::switch_catalog::fetch_game_details(&client, &source, &page_url).await
}

/// Resolves a pasted Steam Workshop link/id to its title, owning AppID and
/// preview art — the frontend uses the AppID to tell the user which of their
/// installed games this mod is for before they commit to downloading it.
#[command]
async fn resolve_workshop_item(input: String) -> Result<managers::workshop::WorkshopItemInfo, String> {
    let item_id = managers::workshop::parse_workshop_id(&input)
        .ok_or_else(|| "No se pudo reconocer un ID de Workshop en ese link.".to_string())?;
    let client = make_client()?;
    managers::workshop::resolve_workshop_item(&client, &item_id).await
}

/// Downloads a Workshop item's content through a third-party mirror (see
/// managers::workshop for why — this app never runs a real Steam client's
/// own Workshop subscribe/download path) and drops it into
/// steamapps/workshop/content/<appid>/<itemid>/, same as real Steam would.
/// `title`/`preview_url` come from the frontend's earlier resolve step —
/// purely for Ragnarok's own installed-mods list, not sent anywhere else.
#[command]
async fn install_workshop_item(
    steam_path: String,
    app_id: String,
    item_id: String,
    title: String,
    preview_url: String,
) -> Result<managers::workshop::WorkshopInstallResult, String> {
    let client = make_client()?;
    let bytes = managers::workshop::download_workshop_item(&client, &app_id, &item_id).await?;
    let library_paths = ManifestManager::get_library_folders(&steam_path);
    managers::workshop::install_workshop_content(&library_paths, &steam_path, &app_id, &item_id, &title, &preview_url, &bytes)
}

/// Lists what Ragnarok has installed for this game via the Workshop panel —
/// backs the "installed mods" list so the user can see and remove them
/// without hunting through folders manually.
#[command]
async fn list_installed_workshop_items(steam_path: String, app_id: String) -> Vec<managers::workshop::InstalledWorkshopItem> {
    let library_paths = ManifestManager::get_library_folders(&steam_path);
    managers::workshop::list_installed_workshop_items(&library_paths, &steam_path, &app_id)
}

/// Removes a previously-installed Workshop item — content folder, Ragnarok's
/// own index entry, the addons/ mirror if any, and the appworkshop_<appid>.acf
/// entry (best-effort, same as when it's written).
#[command]
async fn uninstall_workshop_item(steam_path: String, app_id: String, item_id: String) -> Result<(), String> {
    let library_paths = ManifestManager::get_library_folders(&steam_path);
    managers::workshop::uninstall_workshop_item(&library_paths, &steam_path, &app_id, &item_id)
}

/// Owner-only — no-ops instantly (before any network call) if this PC has no
/// GitHub token configured, so it's inert for every user except whoever set
/// that up (reusing the exact same encrypted token file as Ragnarok
/// Legends). Bundles the ticket files install_game just wrote for `app_id`
/// into a zip and commits it to the shared cache, so the next install of
/// this same game — by anyone — hits try_fetch_lua_cache's fast path instead
/// of depending on Ryuu's live generator. Best-effort throughout: never
/// surfaces an error to the caller, a failed cache write just means this
/// game stays on the normal path next time too.
/// Owner-only — no-ops instantly (before any network call) if this PC has no
/// GitHub token configured, exactly like cache_lua_ticket_to_github. Uploads
/// the freshly-synced catalog to `catalog.json` in the shared repo so users
/// whose ISP blocks generator.ryuu.lol outright can still load it from
/// GitHub/jsDelivr (see game.rs's try_fetch_catalog_mirror). Best-effort:
/// a failed upload just means the mirror stays one sync behind.
async fn publish_catalog_mirror_to_github(catalog: &[managers::game::GameMetadata]) {
    let Some(token) = read_legends_token() else { return };
    if catalog.is_empty() {
        return;
    }
    let Ok(json) = serde_json::to_vec(catalog) else { return };

    let Ok(client) = crate::make_client() else { return };
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/contents/catalog.json",
        RAGNAROK_LEGENDS_REPO_OWNER, RAGNAROK_LEGENDS_REPO_NAME
    );

    // Same sha-first dance the Contents API requires to overwrite a file
    // that already exists (see cache_lua_ticket_to_github).
    let sha = match client
        .get(&api_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Ragnarok-Launcher-App")
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["sha"].as_str().map(|s| s.to_string())),
        _ => None,
    };

    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    let mut payload = serde_json::json!({
        "message": format!("chore: actualizar catalog.json ({} juegos)", catalog.len()),
        "content": BASE64.encode(&json),
    });
    if let Some(sha) = sha {
        payload["sha"] = serde_json::Value::String(sha);
    }

    match client
        .put(&api_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Ragnarok-Launcher-App")
        .json(&payload)
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => {
            diag_log!("[publish_catalog_mirror] catalog.json actualizado ({} juegos)", catalog.len());
        }
        Ok(res) => diag_log!("[publish_catalog_mirror] GitHub respondió HTTP {}", res.status()),
        Err(e) => diag_log!("[publish_catalog_mirror] Falló la subida: {}", e),
    }
}

async fn cache_lua_ticket_to_github(steam_path: &str, app_id: &str, depot_ids: &[String]) {
    let Some(token) = read_legends_token() else { return };

    let lua_path = std::path::PathBuf::from(steam_path).join("config").join("lua").join(format!("{}.lua", app_id));
    let Ok(lua_bytes) = std::fs::read(&lua_path) else { return };

    let mut zip_bytes: Vec<u8> = Vec::new();
    {
        use std::io::Write;
        let cursor = std::io::Cursor::new(&mut zip_bytes);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        if zip.start_file(format!("{}.lua", app_id), options).is_err() {
            return;
        }
        if zip.write_all(&lua_bytes).is_err() {
            return;
        }

        let depotcache = std::path::PathBuf::from(steam_path).join("depotcache");
        if let Ok(entries) = std::fs::read_dir(&depotcache) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().into_owned();
                let matches_depot = depot_ids.iter().any(|d| fname.starts_with(&format!("{}_", d)));
                if matches_depot && fname.ends_with(".manifest") {
                    if let Ok(bytes) = std::fs::read(entry.path()) {
                        if zip.start_file(format!("depotcache/{}", fname), options).is_ok() {
                            let _ = zip.write_all(&bytes);
                        }
                    }
                }
            }
        }
        if zip.finish().is_err() {
            return;
        }
    }
    if zip_bytes.is_empty() {
        return;
    }

    let Ok(client) = crate::make_client() else { return };
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}/{}.zip",
        RAGNAROK_LEGENDS_REPO_OWNER, RAGNAROK_LEGENDS_REPO_NAME, LUA_CACHE_REPO_PATH_PREFIX, app_id
    );

    // Look up an existing sha first — the Contents API requires it to
    // overwrite a file that's already there (e.g. re-caching after a game
    // update), and rejects the write outright without it.
    let sha = match client
        .get(&api_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Ragnarok-Launcher-App")
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["sha"].as_str().map(|s| s.to_string())),
        _ => None,
    };

    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    let encoded = BASE64.encode(&zip_bytes);
    let mut payload = serde_json::json!({
        "message": format!("Cache ticket for AppID {}", app_id),
        "content": encoded,
    });
    if let Some(sha) = sha {
        payload["sha"] = serde_json::Value::String(sha);
    }

    let _ = client
        .put(&api_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Ragnarok-Launcher-App")
        .json(&payload)
        .send()
        .await;

    // Best-effort metadata sidecar: whatever buildid the ACF happens to show
    // locally right now. May be empty (game not fully installed by Steam
    // yet) — that's fine, the sidecar just won't have a buildId in that case
    // and the frontend shows "no disponible" instead of a number.
    let Ok(build_id) = read_local_build_id(steam_path, app_id) else { return };
    if build_id.trim().is_empty() {
        return;
    }
    let meta_api_url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}/{}.json",
        RAGNAROK_LEGENDS_REPO_OWNER, RAGNAROK_LEGENDS_REPO_NAME, LUA_CACHE_REPO_PATH_PREFIX, app_id
    );
    let meta_sha = match client
        .get(&meta_api_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Ragnarok-Launcher-App")
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => res
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["sha"].as_str().map(|s| s.to_string())),
        _ => None,
    };
    let meta_content = serde_json::json!({ "buildId": build_id }).to_string();
    let mut meta_payload = serde_json::json!({
        "message": format!("Cache ticket metadata for AppID {}", app_id),
        "content": BASE64.encode(meta_content.as_bytes()),
    });
    if let Some(sha) = meta_sha {
        meta_payload["sha"] = serde_json::Value::String(sha);
    }
    let _ = client
        .put(&meta_api_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Ragnarok-Launcher-App")
        .json(&meta_payload)
        .send()
        .await;
}

#[derive(Serialize, Deserialize, Clone)]
struct MediafireFile {
    filename: String,
    size: String,
    created: String,
    download_url: String,
}

// Short-lived in-memory cache for both bypass sources, keyed by
// "mediafire:{key}" / "gdrive:{folder}". Without this, opening the Bypass
// tab always re-scraped Mediafire's API and re-downloaded + re-parsed the
// whole Google Drive folder listing from scratch — every single time,
// including just tabbing away and back. A 3-minute cache makes every visit
// after the first one within that window instant, which is what actually
// made the tab feel slow to open repeatedly (the parsing fix above helps
// the first load, this helps every load after it).
static BYPASS_CACHE: Lazy<Mutex<HashMap<String, (std::time::Instant, Vec<MediafireFile>)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const BYPASS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(180);

fn bypass_cache_get(key: &str) -> Option<Vec<MediafireFile>> {
    let cache = BYPASS_CACHE.lock().ok()?;
    let (cached_at, files) = cache.get(key)?;
    if cached_at.elapsed() < BYPASS_CACHE_TTL {
        Some(files.clone())
    } else {
        None
    }
}

fn bypass_cache_set(key: &str, files: &[MediafireFile]) {
    if let Ok(mut cache) = BYPASS_CACHE.lock() {
        cache.insert(key.to_string(), (std::time::Instant::now(), files.to_vec()));
    }
}

// ── Bypass lists kept on disk ───────────────────────────────────────────────
//
// The in-memory cache above lasts three minutes and dies with the process, so
// every launch opened the Bypass tab on skeletons while both sources were
// fetched from scratch — Google Drive's listing plus one HEAD request per file
// for its size. The last good lists are now kept on disk and painted at once;
// the network fetch only refreshes them.

#[derive(Serialize, Deserialize, Default)]
struct BypassDiskCache {
    #[serde(default)]
    mediafire: Vec<MediafireFile>,
    #[serde(default)]
    gdrive: Vec<MediafireFile>,
    #[serde(default)]
    fetched_at: u64,
}

static BYPASS_DISK_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn bypass_data_file(name: &str) -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join(name))
}

fn bypass_disk_load() -> BypassDiskCache {
    bypass_data_file("bypass_cache.json")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Updates the saved lists in place. Serialised, because the two sources
/// finish at their own times and each rewrites the same file.
fn bypass_disk_update(update: impl FnOnce(&mut BypassDiskCache)) {
    let Ok(_guard) = BYPASS_DISK_LOCK.lock() else { return };
    let Some(path) = bypass_data_file("bypass_cache.json") else { return };
    let mut cache = bypass_disk_load();
    update(&mut cache);
    cache.fetched_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(json) = serde_json::to_string(&cache) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// The lists saved by the last successful fetch, for an instant first paint.
#[command]
async fn get_cached_bypass() -> Result<Option<BypassDiskCache>, String> {
    let cache = bypass_disk_load();
    Ok((!cache.mediafire.is_empty() || !cache.gdrive.is_empty()).then_some(cache))
}

/// Google Drive file sizes already looked up, by file id. A file id's size
/// never changes, so each is asked for once.
fn gdrive_sizes_load() -> HashMap<String, u64> {
    bypass_data_file("gdrive_sizes.json")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn gdrive_sizes_store(sizes: &HashMap<String, u64>) {
    let Some(path) = bypass_data_file("gdrive_sizes.json") else { return };
    if let Ok(json) = serde_json::to_string(sizes) {
        let _ = std::fs::write(path, json);
    }
}

fn gdrive_file_id(download_url: &str) -> Option<&str> {
    download_url.rsplit("id=").next().filter(|id| !id.is_empty() && !id.contains('/'))
}

const DEFAULT_MEDIAFIRE_FOLDERS: &[&str] = &["1ukdsqzvsdokn", "vhonhj3luhxo3"];

async fn fetch_single_mediafire_folder(
    client: &reqwest::Client,
    folder_key: &str,
) -> Result<Vec<MediafireFile>, String> {
    let cache_key = format!("mediafire:{}", folder_key);
    if let Some(cached) = bypass_cache_get(&cache_key) {
        return Ok(cached);
    }

    let mut chunk = 1;
    let mut files = Vec::new();

    loop {
        let url = format!(
            "https://www.mediafire.com/api/1.4/folder/get_content.php?content_type=files&filter=all&order_by=name&order_direction=asc&chunk={}&version=1.5&folder_key={}&response_format=json",
            chunk, folder_key
        );

        let res = client.get(url).send().await.map_err(|e| e.to_string())?;
        let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

        if let Some(files_array) = data["response"]["folder_content"]["files"].as_array() {
            for f in files_array {
                if let (Some(filename), Some(size), Some(created), Some(link)) = (
                    f["filename"].as_str(),
                    f["size"].as_str(),
                    f["created"].as_str(),
                    f["links"]["normal_download"].as_str(),
                ) {
                    files.push(MediafireFile {
                        filename: filename.to_string(),
                        size: size.to_string(),
                        created: created.to_string(),
                        download_url: link.to_string(),
                    });
                }
            }
        }

        let more = data["response"]["folder_content"]["more_chunks"]
            .as_str()
            .unwrap_or("no");
        if more == "yes" && chunk < 20 {
            chunk += 1;
        } else {
            break;
        }
    }

    bypass_cache_set(&cache_key, &files);
    Ok(files)
}

#[command]
async fn fetch_mediafire_bypass(folder_id: Option<String>) -> Result<Vec<MediafireFile>, String> {
    let client = make_client()?;

    if let Some(key) = folder_id {
        return fetch_single_mediafire_folder(&client, &key).await;
    }

    let mut all_files = Vec::new();
    let mut seen_links = std::collections::HashSet::new();
    let mut errors = Vec::new();

    // Both folders at once. They were fetched one after the other, and the
    // tab waited for the sum of the two.
    let results = futures::future::join_all(
        DEFAULT_MEDIAFIRE_FOLDERS
            .iter()
            .map(|key| fetch_single_mediafire_folder(&client, key)),
    )
    .await;
    for (&key, result) in DEFAULT_MEDIAFIRE_FOLDERS.iter().zip(results) {
        match result {
            Ok(files) => {
                for file in files {
                    if seen_links.insert(file.download_url.clone()) {
                        all_files.push(file);
                    }
                }
            }
            Err(e) => {
                eprintln!("[Bypass] Failed to fetch Mediafire folder {}: {}", key, e);
                errors.push(e);
            }
        }
    }

    if all_files.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }

    // Only a complete answer replaces the saved list: one folder failing must
    // not shrink it.
    if errors.is_empty() {
        let saved = all_files.clone();
        bypass_disk_update(move |c| c.mediafire = saved);
    }
    Ok(all_files)
}

/// Google's embeddedfolderview HTML entity-escapes filenames (confirmed
/// against the real bypass folder: "Assassin's Creed Odyssey FIX.zip" comes
/// back as "Assassin&#39;s Creed Odyssey FIX.zip"). Left undecoded, that
/// breaks both the displayed title and any search match against a filename
/// containing an apostrophe, ampersand, or quote — decode the handful of
/// entities Google actually emits for plain-text filenames before the name
/// is ever shown or filtered against.
fn decode_html_entities(s: &str) -> String {
    s.replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[command]
async fn fetch_gdrive_bypass(app_handle: tauri::AppHandle, folder_id: Option<String>) -> Result<Vec<MediafireFile>, String> {
    let folder = folder_id.unwrap_or_else(|| "13iry6bKKpBR75EcpCzOMA8rIlr4zIZO6".to_string());
    let cache_key = format!("gdrive:{}", folder);
    if let Some(cached) = bypass_cache_get(&cache_key) {
        return Ok(cached);
    }

    let client = make_client()?;

    // Use Google Drive embedded folder view (works for public folders, no API key needed)
    let url = format!(
        "https://drive.google.com/embeddedfolderview?id={}#list",
        folder
    );

    let html = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let mut files = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // Pattern 1: Modern Google Drive embedded folder format
    // Entry format: <div class="flip-entry" id="entry-FILE_ID" ...>
    // Filename is in: <div class="flip-entry-name">FILENAME</div>
    // We find each entry by its id="entry-<FILE_ID>" and then extract the filename from the flip-entry-name div
    let re_entry = regex::Regex::new(r#"id="entry-([a-zA-Z0-9_-]{25,})"#)
        .map_err(|e| e.to_string())?;
    // Compiled once and reused for every entry below — this used to be
    // recompiled from scratch on every single loop iteration (a real cost:
    // regex compilation isn't free), which is exactly the kind of thing
    // that made this bypass tab feel slow to load for folders with a lot
    // of files in them.
    let re_name = regex::Regex::new(r#"class="flip-entry-(?:name|title)"[^>]*>([^<]+)</div>"#)
        .map_err(|e| e.to_string())?;

    for cap in re_entry.captures_iter(&html) {
        let file_id = cap[1].to_string();
        if seen_ids.contains(&file_id) { continue; }

        // The whole match's own position is exactly where "id=\"entry-{id}\""
        // starts — reusing it instead of re-scanning the entire HTML string
        // with html.find() (which used to happen once per entry, so a
        // folder with N files did roughly N full-document scans).
        let entry_start = cap.get(0).map(|m| m.start()).unwrap_or(0);
        // The 3000-byte window is arithmetic, not a character offset, so it
        // lands mid-character the moment the page carries an accent or a ™ —
        // which Drive's HTML does, in file names and in its own localised
        // strings. That panic killed the command, left `invoke` unresolved, and
        // showed up as the Bypass tab spinning forever with no error at all.
        let entry_end = floor_char_boundary(&html, entry_start.saturating_add(3000));
        let entry_section = &html[entry_start..entry_end.max(entry_start)];

        if let Some(name_cap) = re_name.captures(entry_section) {
            let name = decode_html_entities(name_cap[1].trim());
            if !name.is_empty() {
                seen_ids.insert(file_id.clone());
                files.push(MediafireFile {
                    filename: name,
                    size: "-".to_string(),
                    created: "-".to_string(),
                    download_url: format!("https://drive.google.com/uc?export=download&id={}", file_id),
                });
            }
        }
    }

    // Pattern 2: Legacy JSON array data embedded in HTML (fallback)
    if files.is_empty() {
        let re_id = regex::Regex::new(r#"\["([a-zA-Z0-9_-]{25,})"[^]]*,"([^"]+\.[a-zA-Z0-9]+)","#)
            .map_err(|e| e.to_string())?;
        for cap in re_id.captures_iter(&html) {
            let id = cap[1].to_string();
            let name = decode_html_entities(cap[2].trim());
            if name.is_empty() || seen_ids.contains(&id) { continue; }
            seen_ids.insert(id.clone());
            files.push(MediafireFile {
                filename: name,
                size: "-".to_string(),
                created: "-".to_string(),
                download_url: format!("https://drive.google.com/uc?export=download&id={}", id),
            });
        }
    }

    // Pattern 3: Alternative JSON format with application mimetype (fallback)
    if files.is_empty() {
        let re_json = regex::Regex::new(r#"\["([a-zA-Z0-9_-]{25,})",null,"([^"]+)","application"#)
            .map_err(|e| e.to_string())?;
        for cap in re_json.captures_iter(&html) {
            let id = cap[1].to_string();
            let name = decode_html_entities(cap[2].trim());
            if name.is_empty() || seen_ids.contains(&id) { continue; }
            seen_ids.insert(id.clone());
            files.push(MediafireFile {
                filename: name,
                size: "-".to_string(),
                created: "-".to_string(),
                download_url: format!("https://drive.google.com/uc?export=download&id={}", id),
            });
        }
    }

    // Google's embedded folder view never exposes file size, unlike Mediafire's
    // API. Sizes already looked up are filled in from disk; the rest are
    // resolved in the background — a HEAD request per file — and sent to the
    // window as `bypass_sizes` when they arrive. The list used to wait for all
    // of them, which was most of the time the tab took to open.
    let mut known = gdrive_sizes_load();
    for f in files.iter_mut() {
        if let Some(size) = gdrive_file_id(&f.download_url).and_then(|id| known.get(id)) {
            f.size = size.to_string();
        }
    }

    bypass_cache_set(&cache_key, &files);
    if !files.is_empty() {
        let saved = files.clone();
        bypass_disk_update(move |c| c.gdrive = saved);
    }

    let mut pending: Vec<MediafireFile> = files.iter().filter(|f| f.size == "-").cloned().collect();
    if !pending.is_empty() {
        tauri::async_runtime::spawn(async move {
            fetch_gdrive_file_sizes(&client, &mut pending).await;
            let resolved: HashMap<String, String> = pending
                .iter()
                .filter(|f| f.size != "-")
                .map(|f| (f.download_url.clone(), f.size.clone()))
                .collect();
            if resolved.is_empty() {
                return;
            }
            for f in &pending {
                if let (Some(id), Ok(bytes)) = (gdrive_file_id(&f.download_url), f.size.parse::<u64>()) {
                    known.insert(id.to_string(), bytes);
                }
            }
            gdrive_sizes_store(&known);

            let patch = |list: &mut Vec<MediafireFile>| {
                for f in list.iter_mut() {
                    if let Some(size) = resolved.get(&f.download_url) {
                        f.size = size.clone();
                    }
                }
            };
            if let Some(mut cached) = bypass_cache_get(&cache_key) {
                patch(&mut cached);
                bypass_cache_set(&cache_key, &cached);
            }
            bypass_disk_update(|c| patch(&mut c.gdrive));
            let _ = app_handle.emit_all("bypass_sizes", &resolved);
        });
    }

    Ok(files)
}

/// Fills in real byte sizes for Google Drive bypass files (in place) by
/// issuing a HEAD request per file to the direct-download endpoint and
/// reading Content-Length. Best-effort: entries that fail to resolve keep
/// their "-" placeholder instead of failing the whole fetch.
async fn fetch_gdrive_file_sizes(client: &reqwest::Client, files: &mut [MediafireFile]) {
    use tokio::sync::Semaphore;
    use std::sync::Arc;

    let sem = Arc::new(Semaphore::new(5));
    let handles: Vec<_> = files.iter().enumerate().map(|(idx, f)| {
        let client = client.clone();
        let sem = sem.clone();
        let id = f.download_url.rsplit("id=").next().unwrap_or("").to_string();
        tokio::spawn(async move {
            if id.is_empty() { return (idx, None); }
            let _permit = sem.acquire().await.ok();
            let direct_url = format!(
                "https://drive.usercontent.google.com/download?id={}&export=download&confirm=t",
                id
            );
            let size = client.head(&direct_url).send().await.ok()
                .and_then(|res| res.content_length());
            (idx, size)
        })
    }).collect();

    for handle in handles {
        if let Ok((idx, Some(bytes))) = handle.await {
            if bytes > 0 {
                files[idx].size = bytes.to_string();
            }
        }
    }
}


// ── Bypass file download ────────────────────────────────────────────────────────

/// Downloads any file URL (Mediafire direct link or Google Drive uc?export=download)
/// and saves it to disk, emitting bypass_download_progress events.
#[command]
async fn download_bypass_file(
    app: tauri::AppHandle,
    url: String,
    filename: String,
    save_path: String,
) -> Result<(), String> {
    use crate::managers::unlockers::RustUnlockerManager;
    use std::path::Path;

    let client = RustUnlockerManager::build_download_client()?;

    // Mediafire/GDrive occasionally serve an HTML "please wait" / rate-limit
    // interstitial instead of the real file for a request or two, even though
    // the same link works fine moments later — stream_download_to_file
    // already detects this (by content-type and by sniffing the first bytes)
    // and fails instead of silently saving a corrupt archive, but a real user
    // then has to notice the error and manually click "Descargar" again.
    // Retry a few times with a short wait and a freshly re-resolved link
    // before giving up, so a transient interstitial self-heals.
    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(4 * attempt as u64)).await;
        }

        // Force the real Google Drive confirm-page probe from the second
        // attempt onward — the fast-path guess (confirm=t) already failed
        // once, and blindly retrying the exact same guessed URL just
        // reproduces the same "returned a web page" failure every time.
        let resolved_url = match RustUnlockerManager::resolve_bypass_download_url(&client, &url, attempt > 0).await {
            Ok(u) => u,
            Err(e) => { last_err = e; continue; }
        };

        match RustUnlockerManager::stream_download_to_file(
            Some(&app),
            &client,
            &resolved_url,
            Path::new(&save_path),
            &filename,
            "bypass_download_progress",
            "filename",
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => last_err = e,
        }
    }

    Err(last_err)
}

/// True if `rel` (an archive-supplied relative path) contains no `..`,
/// drive-prefix or root component — used to reject zip-slip-style entries
/// from formats (RAR) whose extraction API doesn't already guard against it
/// the way `zip::ZipArchive::enclosed_name()` does.
fn bypass_path_is_safe(rel: &Path) -> bool {
    !rel.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::Prefix(_) | std::path::Component::RootDir
        )
    })
}

// Most releases from this Bypass feed put every file directly at the
// archive's root — but some (reported on Discord: Pragmata's update file)
// wrap everything in a single subfolder instead (e.g. "Pragmata (Update)
// CW.FIX/<actual files>"). Extracting that literally creates a brand-new
// subfolder inside the game's install directory instead of patching the
// game itself, which is exactly why the person reporting it had to open
// that folder and move the files out by hand every time. Detecting and
// stripping a single common top-level folder — when EVERY entry in the
// archive sits under the exact same one — makes both layouts extract to
// the same place automatically. Requires at least 2 entries so a lone
// single-file archive never has its own filename mistaken for a wrapping
// folder (which would strip it down to an empty, unusable path).
//
// IMPORTANT: a single common top-level folder is NOT always a release
// wrapper to strip — reported on Discord: Crimson Desert's file puts every
// single entry under a real `bin64\` folder (the game's own actual install
// subfolder, where its .exe genuinely belongs), and stripping that put the
// files loose in the game's root instead of where they needed to go. The
// two cases are only distinguishable by what the common folder is actually
// NAMED and by what the destination already contains — see is_release_wrapper
// below, which every call site must check before trusting this result.
/// Loose files a release drops beside its payload that say nothing about
/// where the payload goes.
///
/// Releases very often ship `Fix/` next to a `readme.txt` or an `.nfo`. Those
/// used to defeat the wrapper detection entirely — two different top-level
/// entries meant "no common root", so nothing was stripped and every file
/// landed one folder too deep. Reported after the first fix shipped: a user
/// still had to open the folder and drag the contents into the game himself.
fn is_root_noise(path: &Path) -> bool {
    // Only ever applies to a file sitting directly at the archive root.
    if path.components().count() != 1 {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "txt" | "nfo" | "diz" | "url" | "md" | "log")
}

fn detect_common_root(paths: &[PathBuf]) -> Option<std::ffi::OsString> {
    if paths.is_empty() {
        return None;
    }
    let mut root: Option<std::ffi::OsString> = None;
    // A folder is only a wrapping root if something actually lives inside it.
    // Without this an archive holding one loose file would treat that FILE as
    // the root and strip it away to nothing.
    let mut has_child = false;

    for path in paths {
        if is_root_noise(path) {
            continue;
        }
        let mut components = path.components();
        let first = match components.next() {
            Some(std::path::Component::Normal(c)) => c.to_os_string(),
            _ => return None,
        };
        if components.next().is_some() {
            has_child = true;
        }
        match &root {
            None => root = Some(first),
            Some(seen) if *seen == first => {}
            Some(_) => return None, // more than one real top-level entry
        }
    }

    if has_child {
        root
    } else {
        None
    }
}

/// Folder names that are real parts of a game's install layout.
///
/// Kept so a fix that legitimately targets one of these is never flattened
/// even on a game that has not created the folder yet — an Unreal title that
/// ships without `Plugins` still wants a `Plugins/` payload put there.
const KNOWN_GAME_SUBDIRS: &[&str] = &[
    "bin", "bin32", "bin64", "binaries", "win32", "win64", "x64", "x86",
    "game", "engine", "data", "content", "plugins", "mods", "system",
    "redist", "_commonredist", "steamapps",
];

/// Whether the single top-level folder inside an archive is a wrapper to strip
/// or a real path to keep.
///
/// The name alone cannot answer this, which is what the previous version got
/// wrong: it stripped only when the folder's name overlapped the archive's own
/// filename. That holds for a release that names its wrapper after itself
/// (Pragmata), but the far more common wrapper is generic — `Crack`, `Fix`,
/// `NoDenuvo`, `Files` — and shares no words with the archive name at all. So
/// nothing was stripped, every file landed in `<game>/Crack/`, and the game
/// never looked there. A user on Discord had to open the folder and drag the
/// files out by hand for it to work.
///
/// Asking the destination is what makes this reliable: `bin64` is a real
/// subfolder because Crimson Desert's install already HAS a `bin64`, not
/// because of how it is spelled.
fn is_release_wrapper(folder_name: &str, archive_stem: &str, game_folder: &Path) -> bool {
    // Named after the release. The original signal, and still the strongest
    // one — it stays right even for a wrapper whose name collides with a real
    // folder, so it is checked first.
    if looks_like_release_wrapper(folder_name, archive_stem) {
        return true;
    }
    // The game already has this folder, so the archive is aiming at it.
    if game_folder.join(folder_name).is_dir() {
        return false;
    }
    // A standard engine folder is a real destination even when absent.
    if KNOWN_GAME_SUBDIRS.contains(&folder_name.to_ascii_lowercase().as_str()) {
        return false;
    }
    // Everything else wrapping an entire archive is a wrapper.
    true
}

/// Loose normalization for comparing an archive's own filename against a
/// candidate wrapping-folder name — lowercase, every run of non-alphanumeric
/// characters collapsed to a single space, trimmed.
fn normalize_loose(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim_end().to_string()
}

/// True if `folder_name` looks like it was named after the release itself
/// (`archive_stem`, the downloaded archive's filename without extension) —
/// scene/bypass releases conventionally wrap their contents in a folder
/// matching the release name (confirmed: Pragmata's file does exactly
/// this) — rather than a real subfolder of the game's own install layout
/// (`bin64`, confirmed on Crimson Desert's file, whose folder name has
/// nothing to do with the game's title at all). Compares by shared
/// significant words rather than requiring an exact match, since release
/// tags (CW.FIX, Update, scene group names) don't always survive
/// identically on both sides.
fn looks_like_release_wrapper(folder_name: &str, archive_stem: &str) -> bool {
    let folder_norm = normalize_loose(folder_name);
    let archive_norm = normalize_loose(archive_stem);
    if folder_norm.is_empty() || archive_norm.is_empty() {
        return false;
    }
    let folder_words: std::collections::HashSet<&str> =
        folder_norm.split_whitespace().filter(|w| w.len() >= 3).collect();
    if folder_words.is_empty() {
        return false;
    }
    let archive_words: std::collections::HashSet<&str> =
        archive_norm.split_whitespace().filter(|w| w.len() >= 3).collect();
    let shared = folder_words.intersection(&archive_words).count();
    // A real install subfolder's name essentially never overlaps with a
    // release's own filename at all (0 shared words) — a genuine wrapper
    // folder's name is near-identical to the archive's, so requiring most
    // of the folder name's own words to also appear in the archive name
    // cleanly separates the two cases against both confirmed real examples.
    (shared as f32) / (folder_words.len() as f32) >= 0.6
}

fn strip_common_root(rel: &Path, common_root: Option<&std::ffi::OsStr>) -> PathBuf {
    let Some(root) = common_root else { return rel.to_path_buf() };
    let mut components = rel.components();
    if let Some(std::path::Component::Normal(first)) = components.next() {
        if first == root {
            return components.as_path().to_path_buf();
        }
    }
    rel.to_path_buf()
}

/// Where auto-applied Bypass backups live: Ragnarok's own AppData folder,
/// NOT inside the game's install directory. It used to be
/// `<game>/.ragnarok_bypass_backup/<appid>/`, which meant every auto-apply
/// left a stray folder sitting in the game's own files — visible clutter
/// the user never asked for, and something a game's own integrity check or
/// a future repack update could trip over. The backup itself is still worth
/// keeping (it's the only way to undo a bad fix), it just doesn't belong in
/// the game folder.
fn bypass_backup_dir(app_id: &str) -> Result<PathBuf, String> {
    let base = dirs::data_dir().ok_or("No se pudo resolver AppData")?;
    Ok(base
        .join("com.ragnarok.launcher")
        .join("bypass_backups")
        .join(app_id))
}

/// Copies `target`'s current contents to `backup_dir/rel` before it gets
/// overwritten by an auto-applied Bypass file — but only the first time,
/// so re-applying the same fix doesn't clobber the pre-bypass original with
/// an already-patched copy.
fn backup_before_overwrite(target: &Path, backup_dir: &Path, rel: &Path) -> Result<(), String> {
    if !target.exists() {
        return Ok(());
    }
    let backup_target = backup_dir.join(rel);
    if backup_target.exists() {
        return Ok(());
    }
    if let Some(p) = backup_target.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::copy(target, &backup_target).map_err(|e| e.to_string())?;
    Ok(())
}

// Shared by apply_rar_bypass and apply_7z_bypass — both extract to a scratch
// staging directory on disk first (unlike ZIP's by-index loop, neither
// underlying library lets a per-entry callback redirect the actual
// destination path), so the "single wrapping folder" fix from
// detect_common_root's doc comment is done differently here: check the
// already-extracted staging dir for exactly one top-level entry that's
// itself a directory, and treat that as the real root instead — but only
// when looks_like_release_wrapper agrees that folder is actually a release
// wrapper and not a real install subfolder (bin64, confirmed on Crimson
// Desert's file — see looks_like_release_wrapper's doc comment).
fn flatten_single_wrapping_dir(dir: &Path, archive_stem: &str, game_folder: &Path) -> PathBuf {
    let Ok(entries) = std::fs::read_dir(dir) else { return dir.to_path_buf() };

    // A readme or an .nfo beside the folder is not a second entry worth
    // counting — same reason detect_common_root skips them. Requiring exactly
    // one entry made a stray text file enough to leave the payload nested.
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut real_files = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        } else if !is_root_noise(Path::new(path.file_name().unwrap_or_default())) {
            real_files += 1;
        }
    }

    if dirs.len() == 1 && real_files == 0 {
        let path = &dirs[0];
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_release_wrapper(name, archive_stem, game_folder) {
            return path.clone();
        }
    }
    dir.to_path_buf()
}

fn copy_staged_bypass_files(base: &Path, dir: &Path, folder: &Path, backup_dir: &Path, applied: &mut usize) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            copy_staged_bypass_files(base, &path, folder, backup_dir, applied)?;
            continue;
        }
        let rel = path.strip_prefix(base).map_err(|e| e.to_string())?;
        let target = folder.join(rel);
        backup_before_overwrite(&target, backup_dir, rel)?;
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&path, &target).map_err(|e| e.to_string())?;
        *applied += 1;
    }
    Ok(())
}

// The archive password this Bypass feed uses when a file is protected —
// most filenames literally carry "CW.FIX" as a watermark, which is the
// source's standard password, not something that varies per file. Safe to
// pass unconditionally to every archive: all three crates below simply
// ignore a supplied password on entries/archives that aren't actually
// encrypted (verified against zip 0.6.6's by_index_with_optional_password,
// which discards the password when `data.encrypted == false`).
const BYPASS_ARCHIVE_PASSWORD: &str = "CW.FIX";

/// What applying one bypass archive produced.
///
/// `rejected` counts entries skipped because their path would have escaped
/// the game folder. That number existed before but only ever reached a
/// console no release build has, so an archive that was half-refused was
/// reported to the user as a clean "bypass aplicado".
struct BypassApply {
    applied: usize,
    rejected: usize,
}

fn apply_zip_bypass(archive_path: &Path, folder: &Path, backup_dir: &Path) -> Result<BypassApply, String> {
    let zip_file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| e.to_string())?;

    let archive_stem = archive_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let names: Vec<PathBuf> = archive.file_names().map(PathBuf::from).collect();
    let common_root = detect_common_root(&names).filter(|root| {
        root.to_str().map(|r| is_release_wrapper(r, archive_stem, folder)).unwrap_or(false)
    });

    let mut applied = 0usize;
    let mut rejected = 0usize;
    for i in 0..archive.len() {
        let mut entry = match archive.by_index_decrypt(i, BYPASS_ARCHIVE_PASSWORD.as_bytes()).map_err(|e| e.to_string())? {
            Ok(entry) => entry,
            Err(_invalid_password) => {
                return Err(format!(
                    "'{}' está protegido con una contraseña distinta a la esperada (CW.FIX).",
                    archive_path.file_name().and_then(|n| n.to_str()).unwrap_or("archivo")
                ));
            }
        };
        // enclosed_name() already rejects zip-slip entries (`..`, absolute paths).
        let Some(raw_name) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            diag_log!("[bypass] Entrada zip rechazada por ruta insegura: {}", entry.name());
            rejected += 1;
            continue;
        };
        let name = strip_common_root(&raw_name, common_root.as_deref());
        if name.as_os_str().is_empty() {
            continue; // The wrapping folder's own directory entry — nothing to create.
        }
        let target = folder.join(&name);

        if entry.name().ends_with('/') {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }

        backup_before_overwrite(&target, backup_dir, &name)?;
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut outfile = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut outfile).map_err(|e| e.to_string())?;
        applied += 1;
    }
    Ok(BypassApply { applied, rejected })
}

fn apply_rar_bypass(archive_path: &Path, folder: &Path, backup_dir: &Path) -> Result<BypassApply, String> {
    use unrar_ng::{Archive, ExtractEvent};

    // Extracted to a scratch staging dir first, not directly into `folder`
    // — extract_all_with_callback's callback can only notify/cancel the
    // whole operation, the actual on-disk destination path is decided by
    // native UnRAR code and can't be redirected per-entry from here, unlike
    // ZIP's by-index loop. Staging first also means a cancelled/failed
    // extraction never touches a single real game file (strictly safer than
    // the old direct-to-`folder` approach, which could leave a partially
    // patched game if extraction failed partway through).
    let staging = std::env::temp_dir().join(format!("rl_bypass_stage_rar_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    let archive = Archive::with_password(archive_path, BYPASS_ARCHIVE_PASSWORD.as_bytes())
        .open_for_processing()
        .map_err(|e| e.to_string())?;

    // The RAR path aborts outright on an unsafe entry (first_error below),
    // so nothing is ever silently skipped here.
    let rejected = 0usize;
    let mut first_error: Option<String> = None;
    let extract_result = archive
        .extract_all_with_callback(&staging, |event| match event {
            ExtractEvent::Start { filename, .. } => {
                if !bypass_path_is_safe(&filename) {
                    first_error = Some(format!("Entrada insegura en el RAR: {}", filename.display()));
                    return false;
                }
                true
            }
            _ => true,
        })
        .map_err(|e| e.to_string());

    if let Err(e) = extract_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if let Some(e) = first_error {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // See detect_common_root's doc comment — some releases from this feed
    // (reported: Pragmata's update file) wrap everything in one subfolder
    // instead of putting files at the archive root.
    let archive_stem = archive_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let effective_root = flatten_single_wrapping_dir(&staging, archive_stem, folder);

    let mut applied = 0usize;
    let copy_result = copy_staged_bypass_files(&effective_root, &effective_root, folder, backup_dir, &mut applied);
    let _ = std::fs::remove_dir_all(&staging);
    copy_result?;
    Ok(BypassApply { applied, rejected })
}

fn apply_7z_bypass(archive_path: &Path, folder: &Path, backup_dir: &Path) -> Result<BypassApply, String> {
    // sevenz-rust only offers "extract everything to a directory", no
    // per-file callback — extract to a scratch staging dir first, then copy
    // each file over one at a time so the same backup-before-overwrite path
    // as zip/rar applies here too.
    let staging = std::env::temp_dir().join(format!("rl_bypass_stage_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    // Refuse any entry that would land outside the staging folder.
    //
    // This was the one extraction path with no path checking at all: ZIP goes
    // through `enclosed_name()` and RAR through `bypass_path_is_safe`, but
    // sevenz-rust's `decompress_file_*` joins `entry.name()` onto the
    // destination with no sanitising whatsoever (verified in the crate's
    // de_funcs.rs). A `..\..\..\Windows\...` entry — or an absolute path,
    // which `Path::join` swaps in wholesale — writes straight out of the
    // sandbox, skips backup_before_overwrite entirely, is not counted in
    // `applied`, and survives the `remove_dir_all` cleanup. The user is told
    // the bypass applied fine.
    //
    // The per-entry callback hands us the resolved destination, so the check
    // happens before a single byte is written. A bad entry is skipped rather
    // than aborting: the rest of the archive is still worth having, and the
    // count reported to the user reflects only what really landed.
    let staging_root = staging.clone();
    let mut rejected = 0usize;
    let extract_result = (|| -> Result<(), String> {
        let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
        sevenz_rust::decompress_with_extract_fn_and_password(
            file,
            &staging,
            BYPASS_ARCHIVE_PASSWORD.into(),
            |entry, reader, dest| {
                if !dest.starts_with(&staging_root) {
                    diag_log!(
                        "[bypass] Entrada 7z rechazada por salir del directorio: {}",
                        entry.name()
                    );
                    rejected += 1;
                    return Ok(true); // keep going with the rest
                }
                sevenz_rust::default_entry_extract_fn(entry, reader, dest)
            },
        )
        .map_err(|e| e.to_string())
    })();

    if let Err(e) = extract_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if rejected > 0 {
        diag_log!("[bypass] {} entrada(s) del .7z se descartaron por ruta insegura.", rejected);
    }

    // See detect_common_root's doc comment — some releases from this feed
    // wrap everything in one subfolder instead of putting files at the
    // archive root.
    let archive_stem = archive_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let effective_root = flatten_single_wrapping_dir(&staging, archive_stem, folder);

    let mut applied = 0usize;
    let copy_result = copy_staged_bypass_files(&effective_root, &effective_root, folder, backup_dir, &mut applied);
    let _ = std::fs::remove_dir_all(&staging);
    copy_result?;
    Ok(BypassApply { applied, rejected })
}

/// Downloads a Bypass file straight into the matched game's own install
/// folder instead of an arbitrary user-chosen path — the "automatic" apply
/// path the manual `download_bypass_file` command doesn't offer. Supports
/// .zip, .rar and .7z (.exe is never auto-extracted or auto-run — there's
/// nothing to "extract", running an unknown downloaded binary automatically
/// would be its own can of worms). Refuses to touch a folder Steam doesn't
/// report as fully installed (find_game_folder's StateFlags/content check)
/// or that the game is currently running in, and backs up any file it's
/// about to overwrite (once) so an apply can be undone by hand if it turns
/// out wrong — kept in Ragnarok's own AppData dir (see
/// `bypass_backup_dir`), never inside the game folder itself.
#[command]
async fn apply_bypass_file(
    app: tauri::AppHandle,
    steam_path: String,
    app_id: String,
    url: String,
    filename: String,
) -> Result<String, String> {
    use crate::managers::crackers;
    use crate::managers::unlockers::RustUnlockerManager;

    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    if !["zip", "rar", "7z"].contains(&ext.as_str()) {
        return Err(format!(
            "El auto-apply no soporta archivos .{}. Descargalo con el botón manual.",
            ext.to_uppercase()
        ));
    }

    let folder = crackers::find_game_folder(&steam_path, &app_id)
        .ok_or_else(|| "No se encontró una instalación completa de este juego en Steam.".to_string())?;

    if crackers::is_game_running(&folder) {
        return Err("Cerrá el juego antes de aplicar el bypass.".to_string());
    }

    let client = RustUnlockerManager::build_download_client()?;
    let temp_archive = std::env::temp_dir().join(format!("rl_bypass_{}.{}", app_id, ext));

    let mut last_err = String::new();
    let mut downloaded = false;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(4 * attempt as u64)).await;
        }
        let resolved_url = match RustUnlockerManager::resolve_bypass_download_url(&client, &url, attempt > 0).await {
            Ok(u) => u,
            Err(e) => { last_err = e; continue; }
        };
        match RustUnlockerManager::stream_download_to_file(
            Some(&app),
            &client,
            &resolved_url,
            &temp_archive,
            &filename,
            "bypass_download_progress",
            "filename",
        )
        .await
        {
            Ok(()) => { downloaded = true; break; }
            Err(e) => last_err = e,
        }
    }
    if !downloaded {
        return Err(last_err);
    }

    let backup_dir = bypass_backup_dir(&app_id)?;
    let apply_result = match ext.as_str() {
        "zip" => apply_zip_bypass(&temp_archive, &folder, &backup_dir),
        "rar" => apply_rar_bypass(&temp_archive, &folder, &backup_dir),
        "7z" => apply_7z_bypass(&temp_archive, &folder, &backup_dir),
        _ => unreachable!(),
    };

    let _ = std::fs::remove_file(&temp_archive);
    let outcome = apply_result?;
    let mut msg = format!("Bypass aplicado: {} archivo(s) en {}", outcome.applied, folder.display());
    // An archive that tried to write outside the game folder is worth saying
    // out loud: the files it wanted to place are not there, so if the bypass
    // still doesn't work this is the reason.
    if outcome.rejected > 0 {
        msg.push_str(&format!(
            ". {} entrada(s) se descartaron por apuntar fuera de la carpeta del juego",
            outcome.rejected
        ));
    }
    let result: Result<String, String> = Ok(msg);
    result
}

// ── Cloud Saves (Google Drive) commands ────────────────────────────────────────

/// Alias for add_gdrive_account: runs OAuth and returns the authed email.
#[command]
async fn add_gdrive_account() -> Result<String, String> {
    cloud_saves::add_account().await
}

#[command]
async fn auth_google_drive() -> Result<(), String> {
    cloud_saves::do_auth().await.map(|_| ())

}

#[command]
async fn backup_game_saves(app_id: String, steam_path: String, account_email: Option<String>) -> Result<String, String> {
    cloud_saves::do_backup(&app_id, &steam_path, account_email.as_deref().unwrap_or("")).await
}

#[command]
fn backup_saves_local(app_id: String, steam_path: String) -> Result<String, String> {
    cloud_saves::do_local_backup(&app_id, &steam_path)
}



#[command]
async fn list_cloud_backups(app_id: String, account_email: Option<String>) -> Result<Vec<CloudBackupInfo>, String> {
    cloud_saves::list_backups(&app_id, account_email.as_deref().unwrap_or("")).await
}

#[command]
async fn restore_game_saves(
    app_id: String,
    backup_id: String,
    steam_path: String,
    account_email: Option<String>,
) -> Result<(), String> {
    cloud_saves::do_restore(&app_id, &backup_id, &steam_path, account_email.as_deref().unwrap_or("")).await
}

#[command]
async fn delete_cloud_backup(backup_id: String, account_email: Option<String>) -> Result<(), String> {
    cloud_saves::delete_backup(&backup_id, account_email.as_deref().unwrap_or("")).await
}

#[command]
fn get_save_modified_time(app_id: String, steam_path: String) -> u64 {
    let save_path = cloud_saves::find_save_path(&steam_path, &app_id).unwrap_or_else(|| {
        std::path::Path::new(&steam_path)
            .join("userdata")
            .join("0")
            .join(&app_id)
            .join("remote")
    });

    fn get_latest(dir: &std::path::Path) -> u64 {
        let mut latest = 0;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let t = get_latest(&path);
                    if t > latest { latest = t; }
                } else if let Ok(meta) = entry.metadata() {
                    if let Ok(sys_time) = meta.modified() {
                        if let Ok(duration) = sys_time.duration_since(std::time::UNIX_EPOCH) {
                            let secs = duration.as_secs();
                            if secs > latest { latest = secs; }
                        }
                    }
                }
            }
        }
        latest
    }

    let latest_save = get_latest(&save_path);
    // Also considers the Goldberg achievements/stats folder — a game whose
    // save file hasn't changed but whose achievements did should still be
    // picked up by the auto-sync effect that calls this.
    let latest_goldberg = cloud_saves::find_goldberg_saves_dir(&app_id)
        .map(|p| get_latest(&p))
        .unwrap_or(0);
    latest_save.max(latest_goldberg)
}

/// Which of these games currently have a process running out of their own
/// install folder.
///
/// The auto-backup needs this to know when a session is over. Copying a save
/// while the game still owns it risks bundling a half-written file, and on a
/// long session it also means uploading the same progress over and over; the
/// moment worth catching is the one right after the game exits, when
/// everything has been flushed to disk.
///
/// Takes the whole list rather than one app id so the process table is
/// refreshed once per poll instead of once per game — this runs on a timer,
/// and a full refresh per watched title adds up fast.
#[command]
fn running_game_app_ids(app_ids: Vec<String>, steam_path: String) -> Vec<String> {
    use crate::managers::crackers;

    let folders: Vec<(String, PathBuf)> = app_ids
        .into_iter()
        .filter_map(|id| crackers::find_game_folder(&steam_path, &id).map(|f| (id, f)))
        .collect();
    if folders.is_empty() {
        return Vec::new();
    }

    let mut sys = sysinfo::System::new();
    sys.refresh_processes();
    let exes: Vec<PathBuf> = sys
        .processes()
        .values()
        .filter_map(|p| p.exe().map(|e| e.to_path_buf()))
        .collect();

    folders
        .into_iter()
        .filter(|(_, folder)| exes.iter().any(|exe| exe.starts_with(folder)))
        .map(|(id, _)| id)
        .collect()
}

#[command]
fn is_gdrive_connected() -> bool {
    cloud_saves::is_connected()
}

#[command]
fn disconnect_gdrive() {
    cloud_saves::disconnect();
}

#[command]
fn list_gdrive_accounts() -> Vec<String> {
    cloud_saves::list_accounts()
}

#[command]
fn get_default_gdrive_account() -> Option<String> {
    cloud_saves::get_default_account()
}

#[command]
fn set_default_gdrive_account(email: String) -> Result<(), String> {
    cloud_saves::set_default_account(&email)
}

#[command]
fn remove_gdrive_account(email: String) {
    cloud_saves::remove_account(&email);
}

#[command]
#[allow(dead_code)]
async fn check_save_conflict(app_id: String, steam_path: String) -> Result<SaveConflict, String> {
    cloud_saves::check_save_conflict(&app_id, &steam_path, "").await
}

#[derive(Serialize)]
struct SystemSpecs {
    cpu_name: String,
    /// PHYSICAL cores, not logical processors. sysinfo's cpus().len() counts
    /// logical ones, so a 6-core/12-thread CPU reported 12 while the UI
    /// labelled it "núcleos" — and the Specs tab's "meets recommended" check
    /// is a bare `cores >= 4`, so hyperthreading alone was passing it.
    cpu_cores: usize,
    cpu_threads: usize,
    ram_gb: f64,
    gpu_name: String,
    /// Free space on `disk_drive`, NOT always C:. See get_system_specs.
    disk_space_gb: f64,
    /// Which drive disk_space_gb refers to (e.g. "D:"), so the UI can say
    /// which one it's talking about instead of implying "your disk".
    disk_drive: String,
}

#[command]
async fn get_system_specs(steam_path: Option<String>) -> Result<SystemSpecs, String> {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_cpu();
    sys.refresh_memory();

    let cpu_name = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_else(|| "Procesador Genérico".to_string());

    let cpu_threads = sys.cpus().len();
    // physical_core_count() is the real core count; cpus().len() is logical
    // processors (threads). Falls back to the logical count on the rare
    // platform where sysinfo can't determine it.
    let cpu_cores = sys.physical_core_count().unwrap_or(cpu_threads);
    let ram_gb = (sys.total_memory() as f64) / 1024.0 / 1024.0 / 1024.0;
    
    let mut gpu_name = "Gráficos Integrados / Genérico".to_string();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Ok(out) = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name"
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                let gpus: Vec<&str> = name.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
                if !gpus.is_empty() {
                    gpu_name = gpus.join(" / ");
                }
            }
        }
    }
    
    // Free space used to be hardcoded to C: — wrong for anyone whose games
    // don't live there. A Steam install commonly spans several drives (this
    // was reported by a user with libraries on C:, D:, E: and F:), so the
    // question the Specs tab is really answering is "is there room for this
    // game ANYWHERE Steam can install it?" — hence: look at every library
    // folder's drive and report the one with the most space free, along with
    // which drive that is so the UI can name it instead of implying "your
    // disk". Falls back to C: when no Steam path is known.
    let mut candidate_drives: Vec<String> = Vec::new();
    if let Some(sp) = steam_path.as_deref() {
        let mut libs = ManifestManager::get_library_folders(sp);
        if libs.is_empty() {
            libs.push(sp.to_string());
        }
        for lib in libs {
            // "D:\SteamLibrary" -> "D:"
            if let Some(drive) = lib.split(['\\', '/']).next() {
                let drive = drive.trim().to_uppercase();
                if drive.ends_with(':') && !candidate_drives.contains(&drive) {
                    candidate_drives.push(drive);
                }
            }
        }
    }
    if candidate_drives.is_empty() {
        candidate_drives.push("C:".to_string());
    }

    let mut disk_space_gb = 0.0;
    let mut disk_drive = candidate_drives[0].clone();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for drive in &candidate_drives {
            // Get-PSDrive takes the bare letter ("D"), not "D:".
            let letter = drive.trim_end_matches(':');
            if let Ok(out) = std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!("[Math]::Round((Get-PSDrive {}).Free / 1GB, 1)", letter),
                ])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .output()
            {
                let space_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let space_clean = space_str.replace(',', ".");
                if let Ok(space) = space_clean.parse::<f64>() {
                    if space > disk_space_gb {
                        disk_space_gb = space;
                        disk_drive = drive.clone();
                    }
                }
            }
        }
    }

    Ok(SystemSpecs {
        cpu_name,
        cpu_cores,
        cpu_threads,
        ram_gb,
        gpu_name,
        disk_space_gb,
        disk_drive,
    })
}

#[command]
#[allow(dead_code)]
async fn fetch_hv_games() -> Result<Vec<crate::managers::unlockers::HVGame>, String> {
    crate::managers::unlockers::RustUnlockerManager::fetch_hv_games().await
}

// ── Auto Cloud-Sync: Game Watcher ──────────────────────────────────────────
/// Polls the Steam registry every 4s to detect when a running game starts/closes.
/// Emits "game-started" (appID) when a game launches and "game-closed" (appID)
/// when it exits, so the frontend can react (auto cloud-sync, Discord presence).
#[cfg(windows)]
fn spawn_game_watcher(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        let mut last_app_id: u32 = 0;

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

            let current_app_id: u32 = {
                let hkcu = RegKey::predef(HKEY_CURRENT_USER);
                hkcu.open_subkey("Software\\Valve\\Steam")
                    .and_then(|k| k.get_value::<u32, _>("RunningAppID"))
                    .unwrap_or(0)
            };

            // Transition: game was running → now it's closed (0)
            if last_app_id != 0 && current_app_id == 0 {
                let closed_id = last_app_id.to_string();
                let _ = app_handle.emit_all("game-closed", &closed_id);
            }
            // Transition: nothing running → a game just started
            if last_app_id == 0 && current_app_id != 0 {
                let started_id = current_app_id.to_string();
                let _ = app_handle.emit_all("game-started", &started_id);
            }

            last_app_id = current_app_id;
        }
    });
}

// ── Steam Self-Heal Watcher ─────────────────────────────────────────────────
/// Runs continuously in the background so the user never has to manually
/// click "Reparar" for Steam to recover after a bad PC shutdown, Windows Fast
/// Startup hibernation, or a hung bootstrapper:
///
/// - If steam.exe is fully closed, clear any stale "already running" state
///   left behind in the registry (see `clear_stale_state`), so the next
///   Steam launch isn't silently swallowed.
/// - If steam.exe is alive but steamwebhelper.exe (the real client UI) never
///   shows up after ~90s, treat it as a hung bootstrapper: force-kill every
///   Steam process/service and clear the stale state so the next launch
///   starts clean.
#[cfg(windows)]
fn spawn_steam_selfheal_watcher() {
    tauri::async_runtime::spawn(async move {
        use crate::managers::steam::RustSteamManager;

        let mut stuck_ticks: u32 = 0;

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;

            let (steam_alive, webhelper_alive) = RustSteamManager::process_status();

            if !steam_alive {
                RustSteamManager::clear_stale_state();
                stuck_ticks = 0;
                continue;
            }

            if webhelper_alive {
                stuck_ticks = 0;
                continue;
            }

            // steam.exe alive without steamwebhelper.exe: could just be a
            // normal few-second startup window, so require ~90s of this
            // before concluding it's actually hung.
            stuck_ticks += 1;
            if stuck_ticks >= 6 {
                RustSteamManager::kill_all_steam_processes();
                tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
                RustSteamManager::clear_stale_state();
                stuck_ticks = 0;
            }
        }
    });
}

// ── Achievement Watcher ─────────────────────────────────────────────────────
/// Watches whichever game `spawn_game_watcher`'s "game-started"/"game-closed"
/// events say is currently running, and polls its real Steam achievement
/// state periodically, recording each new unlock locally so the Achievements
/// tab reflects it — and bridging anything the emulator earned that Steam
/// does not have. It raises no on-screen notice; see the note at the unlock
/// site below for why, and what putting one back would take.
/// Needs that game's achievement schema already cached locally (from having
/// opened its Achievements page in Ragnarok at least once) — silently does
/// nothing for a game that was never cached, rather than requiring every
/// user to have a Steam Web API key just to get popups.
// Temporary diagnostic log for spawn_achievement_watcher — the watcher runs
// entirely headless (no UI needs to be open for it to work), so when a user
// reports "unlocks while actually playing never popup" there's no other way
// to see which step is silently failing (RunningAppID never detected, schema
// never cached, ach_helper returning empty state, etc.) without asking them
// to reproduce it live with a debugger attached. Appends to a plain file
// instead of only emitting a Tauri event, since the Achievements tab (the
// only place `achievement_sync_log` is displayed) is very likely NOT open
// while the user is actually in-game — that's the whole scenario being
// diagnosed.
fn watcher_log(msg: impl AsRef<str>) {
    if let Some(dir) = dirs::data_dir().map(|d| d.join("com.ragnarok.launcher")) {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("achievement_watcher.log");
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("[{}] {}\n", ts, msg.as_ref());
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Where we remember which achievements the background watcher has already
/// popped a toast for, per app_id — separate from both the Steam Web API
/// schema cache and the GSE/Goldberg local achievement record (that one's
/// for a different concern — emulated games — and conflating the two risks
/// side effects on features that check its presence to mean "this is a
/// Goldberg game"). Needed because tracked_app_id/known_earned live only in
/// memory and get rebuilt from scratch whenever the watcher (wrongly)
/// concludes the game "restarted" — without a durable record, that reseed
/// re-reads "already earned" as brand new and fires a duplicate popup for
/// something the player unlocked minutes ago.
fn notified_set_path(app_id: &str) -> Option<std::path::PathBuf> {
    Some(
        dirs::data_dir()?
            .join("com.ragnarok.launcher")
            .join("achievement_watcher_notified")
            .join(format!("{}.json", app_id)),
    )
}

fn load_notified_set(app_id: &str) -> HashSet<String> {
    notified_set_path(app_id)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_notified_set(app_id: &str, set: &HashSet<String>) {
    if let Some(path) = notified_set_path(app_id) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(set) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[cfg(windows)]
#[derive(Serialize)]
struct DenuvoCheckResult {
    denuvo_detected: bool,
    method: Option<String>,
    section_name: Option<String>,
    // "legacy_ok" | "encrypted_unsupported" | "unknown" — see
    // denuvo_scan::infer_ticket_compat_from_logs for what each means. Always
    // None when denuvo_detected is false.
    ticket_compat: Option<String>,
}

/// Static Denuvo scan + best-effort achievement-compatibility verdict for an
/// installed game, so Ragnarok can warn the user up front instead of them
/// discovering it via a crash mid-session the way we did with Planet Zoo
/// (appid 703080) — see that debugging session for how each ticket path was
/// confirmed. `ticket_compat` stays "unknown" until the game has actually
/// been launched at least once through Ragnarok/OpenSteamTool.
#[command]
#[allow(dead_code)]
fn check_denuvo_compatibility(app_id: String, steam_path: String) -> Result<DenuvoCheckResult, String> {
    let folder = crate::managers::crackers::find_game_folder(&steam_path, &app_id)
        .ok_or_else(|| "No se encontró la carpeta del juego instalado.".to_string())?;
    let exe_path = crate::managers::unlockers::RustUnlockerManager::detect_game_exe(
        &folder.to_string_lossy(),
    )?;

    let scan = crate::managers::denuvo_scan::scan_file_for_denuvo(std::path::Path::new(&exe_path));
    let ticket_compat = if scan.detected {
        Some(crate::managers::denuvo_scan::infer_ticket_compat_from_logs(&steam_path, &app_id))
    } else {
        None
    };

    Ok(DenuvoCheckResult {
        denuvo_detected: scan.detected,
        method: scan.method.map(|m| m.to_string()),
        section_name: scan.section_name,
        ticket_compat,
    })
}

/// Fallback for when Steam's RunningAppID registry value doesn't reveal which
/// game is actually running — observed with The Forest, which showed only
/// 480 (the OnlineFix placeholder) or 0 for its entire session, never the
/// real appid (242760), even confirmed via direct registry checks while the
/// process was genuinely running. Matches every running process's executable
/// path against the install folder of each game Ragnarok has installed: a
/// process whose exe lives under steamapps/common/<installdir for X> is
/// running appid X regardless of what the registry says. Only scans games
/// Ragnarok actually knows are installed, so this stays cheap even with a
/// large catalog.
fn resolve_running_appid_via_process_match(steam_path: &str) -> Option<String> {
    use sysinfo::System;

    let installed_ids = crate::managers::manifest::ManifestManager::list_installed_ids(steam_path);
    if installed_ids.is_empty() {
        return None;
    }

    let mut folders: Vec<(String, PathBuf)> = Vec::new();
    for id in &installed_ids {
        if let Some(folder) = crate::managers::crackers::find_game_folder(steam_path, id) {
            folders.push((id.clone(), folder));
        }
    }
    if folders.is_empty() {
        return None;
    }

    let mut sys = System::new();
    sys.refresh_processes();

    for process in sys.processes().values() {
        let Some(exe_path) = process.exe() else {
            continue;
        };
        if let Some((app_id, _)) = folders.iter().find(|(_, folder)| exe_path.starts_with(folder)) {
            return Some(app_id.clone());
        }
    }
    None
}

fn spawn_achievement_watcher(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        let steam_path = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Valve\\Steam")
            .and_then(|k| k.get_value::<String, _>("SteamPath"))
            .map(|p| p.replace('/', "\\"))
            .unwrap_or_else(|_| r"C:\Program Files (x86)\Steam".to_string());

        let mut tracked_app_id: Option<String> = None;
        let mut known_earned: HashSet<String> = HashSet::new();
        // Games already warned about missing a cached schema this run —
        // without this, a game left running for hours with its Achievements
        // tab never opened logged the same "no cached schema" line every
        // single tick (every 10s) for the entire session, once filling this
        // log with hundreds of identical lines for one game in one evening.
        let mut warned_no_schema: HashSet<String> = HashSet::new();
        // Debounces RunningAppID momentarily reading 0 — observed happening
        // for a single 20s tick at a time during otherwise-continuous
        // gameplay (likely Steam briefly re-validating the ownership-flag
        // game's ticket), which was resetting tracked_app_id/known_earned
        // every ~40-60s and causing the same already-earned achievement to
        // be reseeded as "new" and re-popped repeatedly. Require two
        // consecutive misses (40s of the game genuinely appearing gone)
        // before actually treating it as closed.
        let mut consecutive_not_running: u32 = 0;
        // Rate-limits resolve_running_appid_via_process_match — it enumerates
        // every running process and reads an ACF per installed game, so it's
        // too heavy to run on every 10s tick while genuinely idle. Only
        // matters while tracked_app_id is None (see below); once a game is
        // tracked, this path isn't consulted at all.
        let mut process_match_cooldown: u32 = 0;
        // Achievements this run has already handed to Steam. A push that
        // works removes itself from the pending set on the next tick anyway
        // (Steam then reports it earned), so this only exists to stop a push
        // that *fails* from being retried every 10s for the whole session —
        // each attempt is another ownership validation against the account.
        let mut bridged: HashSet<String> = HashSet::new();

        watcher_log("achievement watcher loop started");

        loop {
            // Was 20s — a real unlock earned right before the player quits
            // the game could sit undetected for up to a full tick, then
            // (with the 2-miss debounce needed to survive the RunningAppID
            // flicker) take up to another ~40s before popping, by which
            // point the game is long closed and the toast reads as
            // "achievement unlocked" despite nothing currently running. 10s
            // roughly halves that worst case while still being an easy load
            // for read_steam_achievement_state / ach_helper to keep up with.
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            process_match_cooldown = process_match_cooldown.saturating_sub(1);

            // Read RunningAppID directly instead of relying on
            // spawn_game_watcher's game-started/game-closed events —
            // diagnostic logging showed those events never reached this
            // watcher's listen_global handlers in practice (zero
            // "game-started event received" lines ever logged despite the
            // user actively playing a tracked game), so cross-task event
            // relay was silently the entire failure. A direct registry read
            // has no such dependency.
            let current_app_id: u32 = RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey("Software\\Valve\\Steam")
                .and_then(|k| k.get_value::<u32, _>("RunningAppID"))
                .unwrap_or(0);

            // Tools like OpenSteamTool's "-onlinefix" launch option make
            // Steam report the running game as appid 480 (Spacewar) instead
            // of the real one, for as long as the game's process stays
            // registered that way — confirmed via achievement_watcher.log
            // showing RunningAppID flip to 480 for extended stretches while
            // a real, already-tracked game (e.g. PEAK, 3527290) was
            // continuously running. Without this, every tick landing on 480
            // found no cached schema for "480" and skipped achievement
            // detection entirely for that tick, silently missing any unlock
            // that happened to land in one of those windows. If we already
            // know which real game is running (from a prior tick), 480 isn't
            // "unknown" — it's that same game, still in progress.
            const ONLINEFIX_PLACEHOLDER_APP_ID: u32 = 480;

            let app_id = if current_app_id == 0 || current_app_id == ONLINEFIX_PLACEHOLDER_APP_ID {
                if let Some(t) = tracked_app_id.clone() {
                    if current_app_id == ONLINEFIX_PLACEHOLDER_APP_ID {
                        // Same as before: the OnlineFix placeholder with an
                        // already-tracked game just means that game is still
                        // running under its spoofed identity.
                        consecutive_not_running = 0;
                        t
                    } else {
                        consecutive_not_running += 1;
                        if consecutive_not_running < 2 {
                            // Could be a momentary blip — don't act on a single miss.
                            continue;
                        }
                        watcher_log("no game currently running (confirmed over 2 ticks), clearing tracked state");
                        tracked_app_id = None;
                        known_earned.clear();
                        continue;
                    }
                } else {
                    // Nothing tracked yet this run and the registry isn't
                    // telling us anything useful — either genuinely nothing
                    // is running, or (the gap found with The Forest)
                    // RunningAppID never once reveals the real appid for
                    // this game even though it's actually running. Rate-
                    // limited process-path match as a last resort.
                    if process_match_cooldown > 0 {
                        continue;
                    }
                    process_match_cooldown = 6; // ~60s between attempts
                    match resolve_running_appid_via_process_match(&steam_path) {
                        Some(matched) => {
                            consecutive_not_running = 0;
                            watcher_log(format!(
                                "RunningAppID gave no usable appid (raw={}) but matched a running process to app_id={} via install-folder path",
                                current_app_id, matched
                            ));
                            matched
                        }
                        None => continue,
                    }
                }
            } else {
                consecutive_not_running = 0;
                current_app_id.to_string()
            };

            let schema = match load_cached_schema(&app_id) {
                Some(s) => s,
                None => {
                    if warned_no_schema.insert(app_id.clone()) {
                        watcher_log(format!("app_id={} has no cached achievement schema — open its Achievements tab in Ragnarok at least once so popups can work for it", app_id));
                    }
                    continue;
                }
            };

            let names: Vec<String> = schema.iter().map(|s| s.name.clone()).collect();
            let names_count = names.len();
            let steam_path_clone = steam_path.clone();
            let app_id_clone = app_id.clone();
            let state = tokio::task::spawn_blocking(move || {
                read_steam_achievement_state(&steam_path_clone, &app_id_clone, &names)
            })
            .await
            .unwrap_or_default();

            watcher_log(format!(
                "app_id={} tick: schema has {} achievements, read_steam_achievement_state returned {} entries",
                app_id, names_count, state.len()
            ));

            // ── emulator → Steam ──────────────────────────────────────────
            // The Steam emulator records unlocks in its own save file and
            // never tells Steam about them, so a game played entirely under
            // it sits at 0/N on the profile no matter how far the player
            // gets. Nothing in the app bridged the two: this watcher reads
            // Steam's side to raise popups, and the Achievements tab could
            // push, but only one achievement per click, by hand.
            //
            // So: take whatever the emulator has earned that Steam does not,
            // and push it. Only ever unlocks — a locked achievement is never
            // cleared from here, so this can't fight the player or undo
            // anything they did deliberately.
            let local_state = load_local_achievement_state(&steam_path, &app_id);
            let pending: Vec<String> = local_state
                .iter()
                .filter(|(name, (earned, _))| {
                    *earned
                        && !bridged.contains(name.as_str())
                        // Absent from Steam's read means it isn't in the
                        // schema either; pushing a name Steam doesn't know
                        // would just fail, so leave it alone.
                        && state.get(name.as_str()).map(|(e, _)| !*e).unwrap_or(false)
                })
                .map(|(name, _)| name.clone())
                .collect();

            if !pending.is_empty() {
                watcher_log(format!(
                    "app_id={} bridge: {} logro(s) del emulador que Steam no tiene — {}",
                    app_id,
                    pending.len(),
                    pending.join(", ")
                ));
                let handle = app_handle.clone();
                let sp = steam_path.clone();
                let aid = app_id.clone();
                let batch = pending.clone();
                let outcome = sync_steam_achievements_batch(handle, sp, aid, batch, true).await;

                // Marked as bridged only once the push actually landed.
                // These three lines used to run BEFORE the call, which meant
                // a failed push — Steam closed, the helper timing out, the
                // game not owned — permanently recorded those achievements as
                // done. They were never retried for the rest of the session,
                // and the popup suppression below had already hidden them, so
                // the player's profile stayed at 0/N with nothing on screen
                // ever mentioning it again.
                let fully_synced = matches!(&outcome, Ok(o) if o.failed == 0);
                if fully_synced {
                    for name in &pending {
                        bridged.insert(name.clone());
                    }

                    // Suppress the popups for these. Without it, the next
                    // tick sees them newly earned on Steam's side and fires a
                    // cartel for every one at once — which, the first time
                    // this runs on a game with a backlog, is a wall of popups
                    // for achievements earned hours ago. Safe to do after the
                    // push: the next tick is a sleep away.
                    let mut notified = load_notified_set(&app_id);
                    for name in &pending {
                        notified.insert(name.clone());
                        known_earned.insert(name.clone());
                    }
                    save_notified_set(&app_id, &notified);
                }

                match outcome {
                    Ok(o) if o.failed == 0 => {
                        watcher_log(format!("app_id={} bridge ok: {}", app_id, o.message))
                    }
                    // A partial push leaves the whole batch unmarked so the
                    // next tick tries again. Re-pushing an achievement Steam
                    // already has is a no-op, so retrying the ones that did
                    // land costs nothing.
                    Ok(o) => watcher_log(format!(
                        "app_id={} bridge parcial ({} de {}): se reintenta en el próximo tick — {}",
                        app_id,
                        o.done,
                        o.done + o.failed,
                        o.message
                    )),
                    Err(e) => watcher_log(format!(
                        "app_id={} bridge fallo: {} — se reintenta en el próximo tick",
                        app_id, e
                    )),
                }
            }

            let is_new_game = tracked_app_id.as_deref() != Some(app_id.as_str());
            if is_new_game {
                // Just started tracking this game — seed with whatever's
                // already earned, PLUS everything we've ever already
                // notified for it (durable across watcher restarts/resets),
                // so we don't fire popups for old progress or re-fire ones
                // already shown.
                tracked_app_id = Some(app_id.clone());
                known_earned = state
                    .iter()
                    .filter(|(_, (earned, _))| *earned)
                    .map(|(name, _)| name.clone())
                    .collect();
                known_earned.extend(load_notified_set(&app_id));
                watcher_log(format!(
                    "app_id={} now tracked as new game, seeded {} already-earned/notified achievements",
                    app_id, known_earned.len()
                ));
                // Snapshot whatever's already earned at the start of this
                // session too, not just newly-detected unlocks below — a
                // game finished (100%) in a session before Ragnarok's own
                // Achievements tab was ever opened for it would otherwise
                // have nothing durable recorded once uninstalled.
                for (name, (earned, earned_time)) in &state {
                    if *earned {
                        let _ = write_local_achievement(&app_id, name, true, *earned_time);
                    }
                }
                continue;
            }

            for (name, (earned, earned_time)) in &state {
                if *earned && !known_earned.contains(name) {
                    known_earned.insert(name.clone());
                    save_notified_set(&app_id, &known_earned);
                    let _ = write_local_achievement(&app_id, name, true, *earned_time);
                    // No on-screen notice is raised here, and that is now
                    // deliberate rather than an accident.
                    //
                    // There were two attempts. The native Windows toast came
                    // first and does not work: Focus Assist's automatic
                    // "playing a game" rule withholds it for the whole
                    // session and only releases it once the user alt-tabs
                    // out, which reads as "nothing happened until I opened
                    // Ragnarok". It was replaced by an `emit_all` of
                    // `achievement_unlocked_popup`, meant to be caught by an
                    // in-app cartel — and that cartel was never built. The
                    // event has been emitted into nothing since the first
                    // commit: no listener for it has ever existed in the
                    // frontend, in any version.
                    //
                    // Emitting it anyway only made the code look like the
                    // feature was there. The unlock is still detected,
                    // recorded locally and shown in the Achievements tab;
                    // what is missing is a live indicator, and putting one
                    // back needs a real overlay window (borderless/windowed
                    // only — nothing draws over exclusive fullscreen), not
                    // another event.
                    if let Some(entry) = schema.iter().find(|s| &s.name == name) {
                        watcher_log(format!("NEW UNLOCK detected: {} ({})", name, entry.display_name));
                        queue_achievement_toast(AchievementToastPayload {
                            name: entry.display_name.clone(),
                            description: entry.description.clone(),
                            icon: entry.icon.clone(),
                            percent: entry.global_percent,
                        });
                    }
                }
            }
        }
    });
}

// ── Crash detection ─────────────────────────────────────────────────────
// A "session marker" file is written at the start of every run and deleted
// on a clean exit (RunEvent::Exit, see the very end of main()) — if it's
// still there the NEXT time the app starts, the previous run never reached
// a clean exit (force-killed, crashed, power loss, antivirus terminated it,
// etc.). Combined with a panic hook that captures the actual Rust panic
// message (when there is one) before the process dies, this lets the app
// offer to report what happened next time it opens instead of the user
// having to describe a crash from memory in a support ticket.
fn crash_marker_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("session_marker.txt"))
}

// ── Tray popup ──────────────────────────────────────────────────────────
// A small borderless window (React view `TrayPopup`, loaded via the `#tray`
// hash) shown next to the tray icon on right-click. Replaces the native menu,
// which Windows draws with its own theme and Tauri 1.x cannot restyle.

const TRAY_W: f64 = 264.0;
const TRAY_H: f64 = 296.0;
static TRAY_HIDDEN_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn build_tray_popup(app: &tauri::AppHandle) {
    let built = tauri::WindowBuilder::new(app, "tray", tauri::WindowUrl::App("index.html#tray".into()))
        .title("Ragnarok Tray")
        .inner_size(TRAY_W, TRAY_H)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build();
    if let Err(e) = built {
        diag_log!("[tray] No se pudo crear el popup de la bandeja: {}", e);
    }
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_tray_popup(window: &tauri::Window) {
    if window.is_visible().unwrap_or(false) {
        TRAY_HIDDEN_AT.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
        let _ = window.hide();
    }
}

fn show_tray_popup(app: &tauri::AppHandle, click: tauri::PhysicalPosition<f64>) {
    let Some(win) = app.get_window("tray") else { return };
    // Right-clicking the icon while the popup is open first steals focus
    // (which hides it), then delivers the click — without this the popup
    // would flash closed and open again instead of toggling.
    if now_ms().saturating_sub(TRAY_HIDDEN_AT.load(std::sync::atomic::Ordering::Relaxed)) < 250 {
        return;
    }
    let scale = win.scale_factor().unwrap_or(1.0);
    let (w, h, gap) = ((TRAY_W * scale) as i32, (TRAY_H * scale) as i32, (10.0 * scale) as i32);
    let (cx, cy) = (click.x as i32, click.y as i32);

    // Monitor that contains the icon (falls back to the window's own one).
    let monitor = win
        .available_monitors()
        .ok()
        .and_then(|list| {
            list.into_iter().find(|m| {
                let (p, sz) = (m.position(), m.size());
                cx >= p.x && cx < p.x + sz.width as i32 && cy >= p.y && cy < p.y + sz.height as i32
            })
        })
        .or_else(|| win.current_monitor().ok().flatten());

    let mut x = cx - w / 2;
    let mut y = cy - h - gap; // taskbar at the bottom: open upwards
    if let Some(m) = monitor {
        let (p, sz) = (m.position(), m.size());
        let (mx, my, mw, mh) = (p.x, p.y, sz.width as i32, sz.height as i32);
        if y < my {
            y = cy + gap; // taskbar at the top: open downwards
        }
        x = x.clamp(mx + gap, (mx + mw - w - gap).max(mx + gap));
        y = y.clamp(my + gap, (my + mh - h - gap).max(my + gap));
    }
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.emit("tray-shown", ());
}

#[command]
fn tray_open_main(app: tauri::AppHandle) {
    show_main_window(&app);
    if let Some(tray) = app.get_window("tray") {
        hide_tray_popup(&tray);
    }
}

#[command]
fn tray_navigate(app: tauri::AppHandle, tab: String) {
    show_main_window(&app);
    let _ = app.emit_to("main", "tray-navigate", tab);
    if let Some(tray) = app.get_window("tray") {
        hide_tray_popup(&tray);
    }
}

#[command]
fn tray_hide(app: tauri::AppHandle) {
    if let Some(tray) = app.get_window("tray") {
        hide_tray_popup(&tray);
    }
}

#[command]
fn tray_quit(app: tauri::AppHandle) {
    // Cleared here directly instead of only relying on RunEvent::Exit —
    // app.exit() isn't guaranteed to let that handler run before the process
    // actually terminates, which would make "Salir" from the tray (a real,
    // intentional close) look like a crash on the next launch, same false
    // positive as the X button.
    if let Some(marker) = crash_marker_path() {
        let _ = std::fs::remove_file(&marker);
    }
    app.exit(0);
}

// ── Achievement toast ───────────────────────────────────────────────────
// A small always-on-top window ("ach_toast", React view `AchievementToast`,
// loaded via the #ach-toast hash) that pops up when the watcher sees a new
// unlock, with the Xbox unlock sound. It never takes focus and lets clicks
// through, so it can sit over a game without pulling the player out of it.
// Like anything drawn by another process it cannot appear over *exclusive*
// fullscreen — borderless/windowed games are fine.

// Kept in step with the layout in AchievementToast.tsx, which is drawn at this size.
const TOAST_W: f64 = 372.0;
const TOAST_H: f64 = 103.0;
// A little above AchievementToast's VISIBLE_MS so its exit animation finishes.
const TOAST_VISIBLE_MS: u64 = 5600;
// Per toast when several unlock together.
const TOAST_BURST_MS: u64 = 3200;

// The Xbox "Rare Achievement" sound. The recording is a jingle followed, a
// second or so later, by a separate closing sound. Three cuts of it:
//   * SND_SINGLE — jingle + closing sound mixed, the closing one timed to
//     land when a lone popup starts to slide away (5 s after it opens);
//   * SND_UNLOCK / SND_CLOSE — the two halves on their own, for a burst of
//     unlocks, where the popup stays up for as long as the burst lasts and the
//     closing sound has to wait for the last one.
static SND_SINGLE: &[u8] = include_bytes!("../sounds/ach_single.wav");
static SND_UNLOCK: &[u8] = include_bytes!("../sounds/ach_unlock.wav");
static SND_CLOSE: &[u8] = include_bytes!("../sounds/ach_close.wav");
// How long before a popup leaves its exit animation starts (AchievementToast's EXIT_MS).
const TOAST_EXIT_LEAD_MS: u64 = 650;

// A sound the player picked in Settings, decoded to WAV once and kept for the
// life of the process. PlaySound reads straight from the buffer while it
// plays, so the buffer is leaked rather than freed when replaced.
static CUSTOM_SND: Lazy<Mutex<Option<&'static [u8]>>> = Lazy::new(|| Mutex::new(None));
const CUSTOM_SND_MAX_SECS: f64 = 12.0;
const CUSTOM_SND_MAX_BYTES: u64 = 25 * 1024 * 1024;

fn custom_sound() -> Option<&'static [u8]> {
    CUSTOM_SND.lock().ok().and_then(|g| *g)
}

#[derive(Serialize, Deserialize, Clone)]
struct AchievementToastSettings {
    enabled: bool,
    sound: bool,
    /// Name of the copy kept inside AppData, so the sound survives the
    /// original being moved or deleted.
    #[serde(default)]
    custom_sound_file: Option<String>,
    /// What the player picked, for showing in Settings.
    #[serde(default)]
    custom_sound_name: Option<String>,
}

impl Default for AchievementToastSettings {
    fn default() -> Self {
        Self { enabled: true, sound: true, custom_sound_file: None, custom_sound_name: None }
    }
}

fn toast_settings_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("achievement_toast.json"))
}

fn load_toast_settings() -> AchievementToastSettings {
    toast_settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

#[command]
fn get_achievement_toast_settings() -> AchievementToastSettings {
    load_toast_settings()
}

fn save_toast_settings(settings: &AchievementToastSettings) -> Result<(), String> {
    let path = toast_settings_path().ok_or("Cannot locate AppData")?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = serde_json::to_string(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[command]
fn set_achievement_toast_settings(enabled: bool, sound: bool) -> Result<(), String> {
    let mut settings = load_toast_settings();
    settings.enabled = enabled;
    settings.sound = sound;
    save_toast_settings(&settings)
}

/// Decodes any audio file symphonia understands into an in-memory 16-bit PCM
/// WAV, which is the one thing PlaySound can play from memory.
fn decode_sound_to_wav(path: &Path) -> Result<Vec<u8>, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|_| "Formato de audio no reconocido. Usa mp3, wav, ogg, flac o m4a.".to_string())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("El archivo no contiene audio.")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| "No se pudo leer ese formato de audio.".to_string())?;

    let (mut rate, mut channels) = (0u32, 0usize);
    let mut pcm: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                channels = spec.channels.count();
                let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
                buf.copy_interleaved_ref(decoded);
                pcm.extend_from_slice(buf.samples());
                if pcm.len() as f64 > CUSTOM_SND_MAX_SECS * rate as f64 * channels as f64 {
                    return Err(format!("El sonido dura más de {} segundos.", CUSTOM_SND_MAX_SECS as u32));
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    if pcm.is_empty() || rate == 0 {
        return Err("No se pudo leer audio de ese archivo.".to_string());
    }
    if !(1..=2).contains(&channels) {
        return Err("El sonido debe ser mono o estéreo.".to_string());
    }

    let data_len = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&(channels as u16).to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    wav.extend_from_slice(&(channels as u16 * 2).to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in &pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(wav)
}

fn set_active_custom_sound(wav: Option<Vec<u8>>) {
    let leaked: Option<&'static [u8]> = wav.map(|w| &*Box::leak(w.into_boxed_slice()));
    if let Ok(mut guard) = CUSTOM_SND.lock() {
        *guard = leaked;
    }
}

/// Picks the player's own sound up again at startup.
fn load_custom_sound_from_settings() {
    let settings = load_toast_settings();
    let Some(file) = settings.custom_sound_file else { return };
    let Some(dir) = toast_settings_path().and_then(|p| p.parent().map(Path::to_path_buf)) else { return };
    match decode_sound_to_wav(&dir.join(&file)) {
        Ok(wav) => set_active_custom_sound(Some(wav)),
        Err(e) => diag_log!("[logros] No se pudo cargar el sonido personalizado {}: {}", file, e),
    }
}

/// Settings → "Elegir sonido": validates the file, keeps a copy in AppData and
/// starts using it. Returns the name to show.
#[command]
fn set_custom_achievement_sound(path: String) -> Result<String, String> {
    let src = PathBuf::from(&path);
    let meta = std::fs::metadata(&src).map_err(|e| e.to_string())?;
    if meta.len() > CUSTOM_SND_MAX_BYTES {
        return Err("El archivo pesa demasiado (máximo 25 MB).".to_string());
    }
    let wav = decode_sound_to_wav(&src)?;

    let dir = toast_settings_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or("Cannot locate AppData")?;
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("snd").to_ascii_lowercase();
    let stored = format!("achievement_sound_custom.{}", ext);
    let mut settings = load_toast_settings();
    if let Some(old) = settings.custom_sound_file.as_deref() {
        if old != stored {
            let _ = std::fs::remove_file(dir.join(old));
        }
    }
    std::fs::copy(&src, dir.join(&stored)).map_err(|e| e.to_string())?;

    let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(stored.clone());
    settings.custom_sound_file = Some(stored);
    settings.custom_sound_name = Some(name.clone());
    save_toast_settings(&settings)?;
    set_active_custom_sound(Some(wav));
    Ok(name)
}

#[command]
fn clear_custom_achievement_sound() -> Result<(), String> {
    let mut settings = load_toast_settings();
    if let (Some(file), Some(dir)) = (
        settings.custom_sound_file.take(),
        toast_settings_path().and_then(|p| p.parent().map(Path::to_path_buf)),
    ) {
        let _ = std::fs::remove_file(dir.join(file));
    }
    settings.custom_sound_name = None;
    save_toast_settings(&settings)?;
    set_active_custom_sound(None);
    Ok(())
}

/// "Escuchar": plays whatever the popup would play for a single unlock.
#[command]
fn preview_achievement_sound() {
    #[cfg(windows)]
    toast_win::play_wav(custom_sound().unwrap_or(SND_SINGLE));
}

#[derive(Serialize, Clone)]
struct AchievementToastPayload {
    name: String,
    description: String,
    icon: String,
    percent: Option<f64>,
}

/// What the window receives: the achievement plus how long to keep it up and
/// where it sits in a burst ("2 / 7").
#[derive(Serialize, Clone)]
struct AchievementToastEvent {
    #[serde(flatten)]
    payload: AchievementToastPayload,
    duration_ms: u64,
    index: usize,
    total: usize,
}

static TOAST_QUEUE: Lazy<Mutex<Option<std::sync::mpsc::Sender<AchievementToastPayload>>>> =
    Lazy::new(|| Mutex::new(None));

fn queue_achievement_toast(payload: AchievementToastPayload) {
    if let Ok(guard) = TOAST_QUEUE.lock() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(payload);
        }
    }
}

#[cfg(windows)]
mod toast_win {
    // Raw user32/winmm calls: nothing else in the app needs the `windows`
    // crate, so two tiny declarations beat a new dependency.
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    }
    #[link(name = "winmm")]
    extern "system" {
        fn PlaySoundW(sound: *const u16, module: isize, flags: u32) -> i32;
    }

    /// Never activated, never in Alt-Tab.
    pub fn make_passive(hwnd: isize) {
        const GWL_EXSTYLE: i32 = -20;
        const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
        const WS_EX_NOACTIVATE: isize = 0x0800_0000;
        unsafe {
            let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, cur | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE);
        }
    }

    /// Shows the window on top without stealing focus from the game.
    pub fn show_topmost_no_activate(hwnd: isize) {
        const SW_SHOWNOACTIVATE: i32 = 4;
        const HWND_TOPMOST: isize = -1;
        const FLAGS: u32 = 0x0001 | 0x0002 | 0x0010; // NOSIZE | NOMOVE | NOACTIVATE
        unsafe {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, FLAGS);
        }
    }

    /// Plays an in-memory PCM .wav without blocking. The buffer has to
    /// outlive playback, which is why callers only pass `'static` data.
    pub fn play_wav(bytes: &'static [u8]) {
        const SND_ASYNC: u32 = 0x0001;
        const SND_NODEFAULT: u32 = 0x0002;
        const SND_MEMORY: u32 = 0x0004;
        unsafe {
            PlaySoundW(bytes.as_ptr() as *const u16, 0, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

fn build_achievement_toast(app: &tauri::AppHandle) {
    let built = tauri::WindowBuilder::new(app, "ach_toast", tauri::WindowUrl::App("index.html#ach-toast".into()))
        .title("Ragnarok Achievement")
        .inner_size(TOAST_W, TOAST_H)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build();
    match built {
        Ok(win) => {
            let _ = win.set_ignore_cursor_events(true);
            #[cfg(windows)]
            if let Ok(hwnd) = win.hwnd() {
                toast_win::make_passive(hwnd.0 as isize);
            }
        }
        Err(e) => diag_log!("[logros] No se pudo crear la ventana del aviso: {}", e),
    }

    // One worker owns the window, so unlocks never stack on top of each other.
    //
    // A burst (a game that grants ten achievements at once, or a backlog the
    // watcher finds) is handled as one unit: it is collected first, then shown
    // in sequence with each toast on screen for a shorter time, and the sound
    // plays ONCE for the whole batch (and the closing sound once, at the end). Playing one per toast cut each
    // fanfare off with the next (PlaySound only plays one sound at a time) and
    // turned ten unlocks into a minute of noise.
    let (tx, rx) = std::sync::mpsc::channel::<AchievementToastPayload>();
    if let Ok(mut guard) = TOAST_QUEUE.lock() {
        *guard = Some(tx);
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            // The watcher queues a burst back to back; give the rest a moment to arrive.
            std::thread::sleep(std::time::Duration::from_millis(250));
            while let Ok(next) = rx.try_recv() {
                batch.push(next);
            }

            let settings = load_toast_settings();
            if !settings.enabled {
                continue;
            }
            let Some(win) = handle.get_window("ach_toast") else { continue };

            let total = batch.len();
            let custom = custom_sound();
            // The player's own sound plays as-is. The built-in one is the
            // single-popup mix, or just the jingle for a burst (its closing
            // sound is played at the end, below).
            #[cfg(windows)]
            if settings.sound {
                toast_win::play_wav(match custom {
                    Some(own) => own,
                    None if total == 1 => SND_SINGLE,
                    None => SND_UNLOCK,
                });
            }

            let per_toast = if total == 1 { TOAST_VISIBLE_MS } else { TOAST_BURST_MS };
            place_achievement_toast(&win);
            #[cfg(windows)]
            if let Ok(hwnd) = win.hwnd() {
                toast_win::show_topmost_no_activate(hwnd.0 as isize);
            }
            #[cfg(not(windows))]
            let _ = win.show();

            for (i, payload) in batch.into_iter().enumerate() {
                let _ = win.emit(
                    "achievement-toast",
                    AchievementToastEvent { payload, duration_ms: per_toast, index: i + 1, total },
                );
                let closing_burst = i + 1 == total && total > 1 && custom.is_none() && settings.sound;
                if closing_burst {
                    // Closing sound as the last popup of a burst starts to leave.
                    let lead = per_toast.saturating_sub(TOAST_EXIT_LEAD_MS);
                    std::thread::sleep(std::time::Duration::from_millis(lead));
                    #[cfg(windows)]
                    toast_win::play_wav(SND_CLOSE);
                    std::thread::sleep(std::time::Duration::from_millis(per_toast - lead));
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(per_toast));
                }
            }
            let _ = win.hide();
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

/// Bottom left of the primary monitor, just above the taskbar.
fn place_achievement_toast(win: &tauri::Window) {
    let Ok(Some(m)) = win.primary_monitor() else { return };
    let scale = m.scale_factor();
    let (mp, ms) = (m.position(), m.size());
    let (w, h) = ((TOAST_W * scale) as i32, (TOAST_H * scale) as i32);
    let x = mp.x + (24.0 * scale) as i32;
    let y = mp.y + ms.height as i32 - h - (72.0 * scale) as i32;
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
}

/// The "Probar" button: shows a real achievement — icon, name, description and
/// rarity — picked at random from the schemas already cached for the player's
/// games, so what they preview is what an unlock will actually look like.
/// Falls back to a placeholder only when no game has a cached schema yet.
#[command]
fn test_achievement_toast(count: Option<u32>) {
    let real = dirs::data_dir()
        .map(|d| d.join("com.ragnarok.launcher").join("achievement_schema_cache"))
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|t| serde_json::from_str::<Vec<AchievementSchemaEntry>>(&t).ok())
                .flatten()
                .filter(|a| !a.icon.is_empty() && !a.display_name.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|all| !all.is_empty());

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    // `count` > 1 sends a burst, to see how several unlocks at once behave.
    for i in 0..count.unwrap_or(1).clamp(1, 12) as usize {
        let payload = match &real {
            Some(all) => {
                let a = &all[(nanos + i * 7919) % all.len()];
                AchievementToastPayload {
                    name: a.display_name.clone(),
                    description: a.description.clone(),
                    icon: a.icon.clone(),
                    percent: a.global_percent,
                }
            }
            None => AchievementToastPayload {
                name: format!("Ragnarok {}", i + 1),
                description: "Prueba del aviso de logros".to_string(),
                icon: String::new(),
                percent: Some(4.2),
            },
        };
        queue_achievement_toast(payload);
    }
}

fn panic_log_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("panic_log.txt"))
}

fn pending_crash_report_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("com.ragnarok.launcher").join("pending_crash_report.txt"))
}

/// Marks %APPDATA%\com.ragnarok.launcher with the FILE_ATTRIBUTE_HIDDEN attribute
/// so it remains hidden in Windows File Explorer.
#[cfg(target_os = "windows")]
fn hide_app_data_dir() {
    if let Some(dir) = dirs::data_dir().map(|d| d.join("com.ragnarok.launcher")) {
        let _ = std::fs::create_dir_all(&dir);
        use std::os::windows::ffi::OsStrExt;
        let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
        wide.push(0);
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x00000002;
        extern "system" {
            fn GetFileAttributesW(lpFileName: *const u16) -> u32;
            fn SetFileAttributesW(lpFileName: *const u16, dwFileAttributes: u32) -> i32;
        }
        unsafe {
            let attrs = GetFileAttributesW(wide.as_ptr());
            if attrs != 0xFFFFFFFF && (attrs & FILE_ATTRIBUTE_HIDDEN) == 0 {
                SetFileAttributesW(wide.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_app_data_dir() {}

/// Called once, very early in main(), before the Tauri app starts. Detects
/// whether the previous run crashed (marker still present) and stages a
/// report for the frontend to pick up via get_pending_crash_report(), then
/// arms the marker + panic hook for THIS run.
fn setup_crash_detection() {
    hide_app_data_dir();

    // Capture Rust panics to a file before the default handler prints (and
    // the process dies) — the most useful detail a report can have.
    std::panic::set_hook(Box::new(|panic_info| {
        if let Some(path) = panic_log_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = std::fs::write(&path, format!("[{}] {}", ts, panic_info));
        }
    }));

    if let Some(marker) = crash_marker_path() {
        if marker.exists() {
            // Previous run didn't shut down cleanly — stage whatever we know.
            let panic_text = panic_log_path().and_then(|p| std::fs::read_to_string(&p).ok());
            let report = match panic_text {
                Some(text) if !text.trim().is_empty() => format!(
                    "El programa se cerró de forma inesperada. Detalle técnico capturado:\n\n{}",
                    text.trim()
                ),
                _ => "El programa se cerró de forma inesperada la última vez (no se pudo capturar más detalle — puede que se haya forzado el cierre, se haya ido la luz, o un antivirus lo haya terminado).".to_string(),
            };
            if let Some(pending) = pending_crash_report_path() {
                let _ = std::fs::write(&pending, report);
            }
        }
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let _ = std::fs::write(&marker, ts);
    }
    // This run's own panic log (if any, from the check above) has already
    // been folded into the pending report — clear it so a crash gets
    // reported once, not repeated on every future launch.
    if let Some(p) = panic_log_path() {
        let _ = std::fs::remove_file(&p);
    }
}

#[command]
async fn get_pending_crash_report() -> Result<Option<String>, String> {
    let Some(path) = pending_crash_report_path() else { return Ok(None) };
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let _ = std::fs::remove_file(&path); // Consumed — only offered once.
            Ok(Some(content))
        }
        Err(_) => Ok(None),
    }
}

#[command]
#[allow(dead_code)]
async fn get_launch_options(steam_path: String, app_id: String) -> Result<Option<String>, String> {
    crate::managers::launch_options::get_launch_options(&steam_path, &app_id)
}

#[command]
#[allow(dead_code)]
async fn set_launch_options(steam_path: String, app_id: String, value: String) -> Result<(), String> {
    crate::managers::launch_options::set_launch_options(&steam_path, &app_id, &value)
}

fn has_online_fix(options: &str) -> bool {
    options.split_whitespace().any(|p| p.eq_ignore_ascii_case("-onlinefix"))
}

/// A launch-options string with `-onlinefix` added or removed, leaving
/// whatever else the user put there as it was.
fn with_online_fix(options: &str, enabled: bool) -> String {
    let mut parts: Vec<&str> = options
        .split_whitespace()
        .filter(|p| !p.eq_ignore_ascii_case("-onlinefix"))
        .collect();
    if enabled {
        parts.push("-onlinefix");
    }
    parts.join(" ")
}

/// Which of these games have `-onlinefix` in their Steam launch options.
#[command]
async fn online_fix_status(steam_path: String, app_ids: Vec<String>) -> Result<Vec<String>, String> {
    Ok(app_ids
        .into_iter()
        .filter(|id| {
            matches!(
                crate::managers::launch_options::get_launch_options(&steam_path, id),
                Ok(Some(options)) if has_online_fix(&options)
            )
        })
        .collect())
}

/// Turns `-onlinefix` on or off for one game.
///
/// Other games are left as they are. OpenSteamTool's limit is one such game
/// *running* at a time, not one configured: it reads the flag from the
/// command line of whichever game Steam is starting. This first version took
/// the flag off every other game, which was not needed and undid what users
/// had set in Steam themselves.
///
/// The Online page used to ask the user to type the flag into Steam's
/// Properties themselves, with a screenshot showing where.
///
/// Refused while Steam runs: Steam keeps localconfig.vdf in memory and writes
/// it back when it exits, so a change made underneath it would vanish.
#[command]
async fn set_online_fix(steam_path: String, app_id: String, enabled: bool) -> Result<(), String> {
    use crate::managers::launch_options::{get_launch_options, set_launch_options};
    if RustSteamManager::is_running() {
        return Err("Cierra Steam primero: Steam reescribe este ajuste al cerrarse y el cambio se perdería.".to_string());
    }
    let current = get_launch_options(&steam_path, &app_id)?.unwrap_or_default();
    set_launch_options(&steam_path, &app_id, &with_online_fix(&current, enabled))
}

/// Covers keys and tokens in log text before it leaves the machine.
///
/// A network error's message carries the full URL, query string included, so
/// a failed Hubcap or Steam Web API call wrote the user's key into the log.
fn redact_secrets(text: &str) -> String {
    static RES: once_cell::sync::Lazy<[regex::Regex; 3]> = once_cell::sync::Lazy::new(|| {
        [
            regex::Regex::new(r"(?i)\b(api_key|apikey|key|token|access_token)=[^&\s)\]]+").expect("patrón literal"),
            regex::Regex::new(r"smm_[0-9a-fA-F]{8,}").expect("patrón literal"),
            regex::Regex::new(r"(?i)\b(bearer|bot)\s+[A-Za-z0-9._-]{20,}").expect("patrón literal"),
        ]
    });
    let text = RES[0].replace_all(text, "$1=***");
    let text = RES[1].replace_all(&text, "smm_***");
    RES[2].replace_all(&text, "$1 ***").into_owned()
}

/// The last lines of ragnarok.log, secrets covered, saved as a file for a
/// support report to attach. A report reading only "it closes every time I
/// open it" had nothing to go on; this is what says why.
#[command]
async fn support_log_excerpt() -> Result<String, String> {
    let path = crate::managers::diag::log_path().ok_or_else(|| "No se encontró el registro.".to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("No se pudo leer el registro: {}", e))?;
    // The manifest guardian and the vault write a line per file they restore,
    // and on a machine where Steam keeps pruning the depot cache that is the
    // whole tail of the log: a real report arrived as 300 lines of
    // "Restaurado: ….manifest" with the actual error nowhere in them. They are
    // counted and left out so the excerpt is made of what explains a problem.
    let is_noise = |l: &str| l.contains("[guardian]") || (l.contains("[vault]") && l.contains("Restaurado"));
    let kept: Vec<&str> = text.lines().filter(|l| !is_noise(l)).collect();
    let omitted = text.lines().count() - kept.len();
    let tail = kept[kept.len().saturating_sub(300)..].join("\n");
    let header = format!(
        "Ragnarok Launcher v{} — registro adjunto al reporte de soporte\n{} — últimas {} líneas útiles ({} líneas repetitivas de [guardian]/[vault] omitidas)\n{}\n",
        APP_VERSION,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        kept.len().min(300),
        omitted,
        "─".repeat(72)
    );
    let out = std::env::temp_dir().join("ragnarok_registro_soporte.txt");
    std::fs::write(&out, format!("{}{}", header, redact_secrets(&tail))).map_err(|e| e.to_string())?;
    Ok(out.to_string_lossy().to_string())
}

#[derive(Serialize)]
struct SupportSystemSummary {
    os: String,
    ram_gb: f64,
    cpu: String,
}

/// What a support reply usually asks for first. No user name, machine name or
/// paths — only the OS, memory and processor model.
#[command]
fn support_system_summary() -> SupportSystemSummary {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_cpu();
    SupportSystemSummary {
        os: sysinfo::System::long_os_version().unwrap_or_else(|| "Windows".to_string()),
        ram_gb: (sys.total_memory() as f64 / 1_073_741_824.0 * 10.0).round() / 10.0,
        cpu: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "—".to_string()),
    }
}

#[cfg(test)]
mod support_log_tests {
    use super::redact_secrets;

    #[test]
    fn keys_and_tokens_never_leave_in_a_report() {
        let key = format!("smm_{}", "ab12".repeat(24));
        let line = format!(
            "No se pudo conectar con Hubcap: error sending request for url (https://hubcapmanifest.com/api/v1/manifest/1?api_key={key})"
        );
        let out = redact_secrets(&line);
        assert!(!out.contains("ab12ab12"), "{out}");
        assert!(out.contains("api_key=***"));

        let steam = redact_secrets("GetSchemaForGame/v2/?key=ABCDEF0123456789&appid=730 falló");
        assert_eq!(steam, "GetSchemaForGame/v2/?key=***&appid=730 falló");
        assert!(!redact_secrets(&format!("clave {key} guardada")).contains("ab12ab12"));
        assert_eq!(redact_secrets("Authorization: Bot abcdefghijklmnopqrstuvwxyz.123"), "Authorization: Bot ***");
        // Ordinary lines stay as they were.
        assert_eq!(redact_secrets("[vault] 115 manifest(s) protegidos"), "[vault] 115 manifest(s) protegidos");
    }
}

/// Closes Steam, for changes that cannot be made while it runs.
#[command]
async fn close_steam() -> Result<(), String> {
    tokio::task::spawn_blocking(RustSteamManager::kill_all_steam_processes)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod online_fix_tests {
    use super::{has_online_fix, with_online_fix};

    #[test]
    fn the_flag_is_added_and_removed_without_touching_the_rest() {
        assert_eq!(with_online_fix("", true), "-onlinefix");
        assert_eq!(with_online_fix("-dx11 -novid", true), "-dx11 -novid -onlinefix");
        assert_eq!(with_online_fix("-dx11 -onlinefix -novid", true), "-dx11 -novid -onlinefix", "sin duplicarlo");
        assert_eq!(with_online_fix("-dx11  -OnlineFix -novid", false), "-dx11 -novid");
        assert_eq!(with_online_fix("-onlinefix", false), "");
        assert!(has_online_fix("-dx11 -onlinefix"));
        assert!(!has_online_fix("-onlinefixed"), "otra opción que empieza igual no cuenta");
    }

    /// Reads the real localconfig.vdf of a Steam install and prints which of
    /// the given games have `-onlinefix`. Read-only. Run with
    /// `RAGNAROK_STEAM_PATH=... RAGNAROK_APP_IDS=1,2,3 cargo test --bin Ragnarok-launcher online_fix_smoke -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn online_fix_smoke() {
        let (Ok(steam), Ok(ids)) = (std::env::var("RAGNAROK_STEAM_PATH"), std::env::var("RAGNAROK_APP_IDS")) else {
            return;
        };
        for id in ids.split(',') {
            let got = crate::managers::launch_options::get_launch_options(&steam, id.trim());
            eprintln!("{id}: {got:?}");
        }
    }
fn load_local_env() {
    let mut candidates = vec![
        std::path::PathBuf::from(".env"),
        std::path::PathBuf::from("src-tauri/.env"),
    ];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join(".env"));
        }
    }
    if let Some(data_dir) = dirs::data_dir() {
        candidates.push(data_dir.join("ragnarok").join(".env"));
    }
    for path in &candidates {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with('#') || line.is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = line.split_once('=') {
                        let k = k.trim();
                        let v = v.trim().trim_matches('"').trim_matches('\'');
                        if std::env::var(k).is_err() {
                            std::env::set_var(k, v);
                        }
                    }
                }
            }
        }
    }
}

fn main() {
    load_local_env();

    // Hidden one-time setup for the Ragnarok Legends GitHub token — run
    // manually from a terminal as:
    //   Ragnarok-launcher.exe --set-legends-token <token>
    // Writes the encrypted file and exits immediately, no window shown, no
    // Tauri/WebView startup at all. Never logged, never sent anywhere.
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--set-legends-token") {
        match args.get(pos + 1) {
            Some(token) if !token.trim().is_empty() => {
                match write_legends_token(token.trim()) {
                    Ok(()) => diag_log!("Token guardado en {:?}", legends_token_path().unwrap_or_default()),
                    Err(e) => eprintln!("Error al guardar el token: {}", e),
                }
            }
            _ => eprintln!("Uso: Ragnarok-launcher.exe --set-legends-token <token>"),
        }
        return;
    }

    // Before anything else writes state: a copy that is only here to wake the
    // one already in the tray must not touch the crash marker on its way out.
    {
        use crate::managers::single_instance::{self, Startup};
        match single_instance::claim() {
            Startup::First(listener) => single_instance::serve(listener),
            Startup::AlreadyRunning => return,
            Startup::Unguarded => {
                diag_log!("[instancia] El puerto de instancia única está ocupado por otro programa; se abre igual.")
            }
        }
    }

    setup_crash_detection();

    tauri::Builder::default()
        .manage(DiscordState(Mutex::new(None)))
        // No native menu on purpose: right-click opens the styled popup
        // window ("tray") instead, see show_tray_popup.
        .system_tray(SystemTray::new().with_tooltip("Ragnarok Launcher"))
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::LeftClick { .. } | SystemTrayEvent::DoubleClick { .. } => {
                show_main_window(app);
            }
            SystemTrayEvent::RightClick { position, .. } => show_tray_popup(app, position),
            _ => {}
        })
        // The X sends the app to the tray; it does not quit.
        //
        // Fixed behaviour, with no setting to turn it off — the same thing
        // Steam and Discord do. It is what keeps the achievement popups and
        // Auto Cloud-Sync's game watcher alive while someone is actually
        // playing, which is the only time they are of any use: both live in
        // this process, so quitting on the X would mean no popups for anyone
        // who closes the launcher before starting a game.
        //
        // "Salir" in the tray menu is the way out, and it is the path that
        // clears the crash marker. Nothing is cleared here on purpose: the
        // process is still running, so treating a hide as a clean shutdown
        // would disarm crash detection for the entire rest of the session.
        .on_window_event(|event| match event.event() {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = event.window().hide();
            }
            // The tray popup behaves like a native menu: clicking anywhere
            // else dismisses it.
            WindowEvent::Focused(false) if event.window().label() == "tray" => {
                hide_tray_popup(event.window());
            }
            _ => {}
        })
        .on_page_load(|window, payload| {
            let label = window.label();
            // On Windows the bundled app is served from https://tauri.localhost,
            // so the "tray" popup must be exempt from the external-link check.
            if label != "main" && label != "tray" && label != "ach_toast" {
                let url = payload.url();
                if url.starts_with("http://") || url.starts_with("https://") {
                    let _ = opener::open(url);
                    let _ = window.close();
                }
            }
        })
        .setup(|_app| {
            crate::managers::single_instance::attach(_app.handle());
            build_tray_popup(&_app.handle());
            build_achievement_toast(&_app.handle());
            load_custom_sound_from_settings();

            // Failsafe for the hidden-at-startup window.
            //
            // The window is created with `visible: false` so the user never
            // sees an empty black frame while the webview boots — React calls
            // appWindow.show() once it has actually painted something. That
            // hands the frontend the power to leave the app permanently
            // invisible if it throws before getting there, which is a far
            // worse failure than a black flash. This shows the window anyway
            // after 8s; on a normal start React has shown it long before, and
            // show() on an already-visible window does nothing.
            {
                let handle = _app.handle();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(8));
                    if let Some(window) = handle.get_window("main") {
                        if !window.is_visible().unwrap_or(true) {
                            diag_log!("[startup] La ventana seguía oculta a los 8s — se muestra desde Rust.");
                            let _ = window.show();
                        }
                    }
                });
            }

            // Spawn the game watcher for Auto Cloud-Sync feature
            #[cfg(windows)]
            spawn_game_watcher(_app.handle());

            // Self-heal Steam's "already running" stale state automatically,
            // so users don't need to click a repair button after every bad
            // shutdown or hung bootstrapper.
            #[cfg(windows)]
            spawn_steam_selfheal_watcher();

            // Records unlocks for whichever game spawn_game_watcher reports
            // as running, and pushes emulator-earned achievements up to
            // Steam. Runs whether or not the Achievements tab is open, which
            // is the whole point of it living out here — and the reason the
            // X button hides to tray instead of quitting.
            #[cfg(windows)]
            spawn_achievement_watcher(_app.handle());

            // Back up Steam's depotcache before Steam has a chance to prune it.
            //
            // These files are what let a spoofed game install at all: Steam
            // reads the depot's file list from disk instead of asking the CDN,
            // which is the request that comes back 401 for an app the account
            // does not own. Steam treats the folder as its own cache and
            // deletes from it — an uninstall took one out on a real install,
            // and a build change took three more — after which the game can
            // never be set up again without going back to the network.
            //
            // Off the main thread and best-effort: a copy that fails costs
            // nothing today, and the whole point is that it runs unattended
            // rather than after someone notices a game is broken.
            #[cfg(windows)]
            std::thread::spawn(|| {
                // First detected install, same fallback the rest of the
                // backend uses. A machine with two Steams gets the other one
                // backed up on the next run through the Fix tab.
                let Some(steam_path) = RustSteamManager::detect_steam_installations().into_iter().next()
                else {
                    return;
                };
                // Protect existing manifests in depotcache with Read-Only attribute
                // so Steam's uninstaller cannot delete them.
                crate::managers::manifest_vault::protect_depotcache_manifests(&steam_path);

                // Restore first, then back up. Steam may have pruned a
                // manifest since the last run, and putting those back before
                // the snapshot means the vault never records a gap as if it
                // were the normal state.
                let (copied, seen) = crate::managers::manifest_vault::back_up(&steam_path);
                if seen > 0 && copied == 0 {
                    diag_log!("[vault] {} manifest(s) ya estaban respaldados.", seen);
                }
                // The restore half needs the network for its mirror fallback,
                // so it runs on the async runtime rather than this thread.
                // NOTE: use tauri::async_runtime::spawn, NOT tokio::spawn —
                // std::thread has no Tokio context so tokio::spawn panics here.
                tauri::async_runtime::spawn(async move {
                    crate::managers::manifest_vault::restore_missing(&steam_path).await;
                    crate::managers::manifest_vault::protect_depotcache_manifests(&steam_path);
                });
            });

        // Persistent depotcache guardian: polls every 3s and restores any manifest
        // that Steam (or SteamService as SYSTEM) deletes. The function spawns its
        // own background thread and returns immediately.
        if let Some(steam_path) = RustSteamManager::detect_steam_installations().into_iter().next() {
            crate::managers::manifest_vault::start_depotcache_guardian(steam_path, 3);
        }

            // Keep the cover-art cache from growing without end. Off the main
            // thread because it has to stat every file in the folder, and on a
            // machine that already accumulated the old unbounded cache that is
            // tens of thousands of them.
            //
            // 1.5 GiB is deliberately generous: at the ~20 KB a cover now costs
            // after write_cover_jpeg re-encodes it, it holds around 78,000 of
            // them — more than the 69,560 titles the catalog currently has. So
            // in practice nothing is ever purged, and this stays a backstop
            // against the unbounded growth that reached 3.1 GB on a real
            // install rather than a limit anyone runs into.
            std::thread::spawn(|| {
                const IMAGE_CACHE_MAX_BYTES: u64 = 1536 * 1024 * 1024;
                // Recompress first, so the size cap below is measured against
                // what the cache actually weighs afterwards.
                recompress_image_cache();
                prune_image_cache(IMAGE_CACHE_MAX_BYTES);
            });

            // Heal any "0 B" install sizes left over from a one-shot task that
            // gave up. Runs once per launch, in the background, and touches
            // only ACFs already showing zero — so it is silent and free on a
            // library that is already correct, and quietly fixes the entries a
            // user would otherwise see stuck at 0 B in Steam's Storage page.
            #[cfg(windows)]
            {
                let handle = _app.handle();
                tauri::async_runtime::spawn(async move {
                    let steam_path = get_steam_path().await.unwrap_or_else(|_| {
                        r"C:\Program Files (x86)\Steam".to_string()
                    });
                    match repair_install_sizes(steam_path).await {
                        Ok(n) if n > 0 => {
                            diag_log!("[install_sizes] {} juego(s) con tamaño 0 B corregidos.", n);
                            let _ = handle.emit_all("install_sizes_repaired", n);
                        }
                        // Nothing to fix is the normal steady state and runs
                        // hourly, so it stays quiet. A failure did not: it was
                        // swallowed by the same catch-all, which meant a user
                        // still staring at "0 B" had nothing anywhere saying
                        // the repair had even tried, let alone why it failed.
                        Ok(_) => {}
                        Err(e) => diag_log!("[install_sizes] No se pudieron corregir los tamaños: {}", e),
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tray_open_main,
            get_achievement_toast_settings,
            set_achievement_toast_settings,
            test_achievement_toast,
            preview_achievement_sound,
            set_custom_achievement_sound,
            clear_custom_achievement_sound,
            tray_navigate,
            tray_hide,
            tray_quit,
            launch_game,
            fetch_game_names,
            search_youtube_trailer,
            sync_games,
            get_cached_catalog_path,
            backup_depot_manifests,
            check_manifest_integrity,
            get_pinned_manifests,
            set_manifest_pinned,
            download_static_version,
            get_static_version_info,
            list_static_versions,
            list_emulators,
            install_emulator,
            uninstall_emulator,
            launch_emulator,
            open_emulator_folder,
            install_switch_keys,
            install_switch_firmware,
            download_and_install_keys,
            scan_switch_games,
            autodetect_switch_games,
            register_games_dir,
            fetch_switch_cover,
            fetch_switch_online_catalog,
            resolve_workshop_item,
            install_workshop_item,
            list_installed_workshop_items,
            uninstall_workshop_item,
            restart_steam,
            disable_steam_cloud_for_app,
            get_game_achievements,
            list_achievement_history,
            get_cached_game_achievements,
            set_local_achievement,
            sync_steam_achievement,
            install_emulator_achievements,
            is_steam_running,
            install_game,
            fetch_steam_data,
            get_hover_trailer,
            get_saved_dlcs,
            get_cached_bypass,
            install_readiness,
            get_game_size,
            get_library_stats,
            online_fix_status,
            set_online_fix,
            close_steam,
            support_log_excerpt,
            run_fixer,
            get_dlcs,
            check_adult_content_batch,
            get_steam_rankings,
            delete_game,
            update_dlcs,
            get_steam_path,
            check_for_updates,
            download_and_apply_update,
            check_previous_update_failure,
            deep_repair,
            install_prerequisites,

            get_installed_app_ids,
            get_really_installed_app_ids,
            get_ragnarok_managed_app_ids,
            get_cached_image_path,
            image_cache_stats,
            clear_image_cache,
            preload_images,
            install_plugin,
            check_plugin_update,
            download_plugin_update,
            dismiss_plugin_update,
            install_manual_lua,
            clean_orphan_manifests,
            hubcap_usage,
            get_install_sources,
            export_ragnarok_config,
            import_ragnarok_config,
            apply_goldberg,
            uninstall_goldberg,
            detect_unlocker_type,
            check_game_drm,
            apply_creamapi,
            uninstall_creamapi,
            apply_smokeapi,
            uninstall_smokeapi,
            apply_uplay_unlocker,
            uninstall_uplay_unlocker,
            apply_koaloader,
            uninstall_koaloader,
            run_steamless,
            open_opensteamtool_logs,
            get_manifest_provider,
            set_manifest_provider,
            ensure_manifest_chain,
            open_diagnostics_log,
            detect_steam_installations,
            log_diagnostic,
            discord_create_support_thread,
            discord_check_thread_replies,
            discord_send_thread_reply,
            discord_send_support_webhook,
            support_system_summary,
            discord_archive_thread,
            get_pending_crash_report,
            fetch_mediafire_bypass,
            fetch_gdrive_bypass,
            download_bypass_file,
            apply_bypass_file,
            add_gdrive_account,
            auth_google_drive,
            backup_game_saves,
            backup_saves_local,
            repair_install_sizes,
            get_installed_sizes,
            list_cloud_backups,
            restore_game_saves,
            delete_cloud_backup,
            is_gdrive_connected,
            disconnect_gdrive,
            list_gdrive_accounts,
            get_default_gdrive_account,
            set_default_gdrive_account,
            remove_gdrive_account,
            get_save_modified_time,
            running_game_app_ids,
            get_system_specs,
            repair_steam_plugin,

            // ── Not exposed over IPC ──────────────────────────────────────
            // Twenty-seven commands were registered here that no part of
            // the frontend invokes. Registering a command is what makes
            // it callable from the webview, and this app runs with
            // `allowlist: all`, so each one was a reachable entry point
            // bought for nothing — including several that write to Steam's
            // own folders. The functions are all still in the file, marked
            // #[allow(dead_code)]: unregistering is reversible in one line
            // per command, deleting the work is not. Add a name back here
            // the moment something in the UI actually calls it.
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // Clean exit — clear the crash marker so next launch doesn't
            // think this run crashed. Covers "Salir" from the tray and any
            // other path that actually ends the process (closing to tray
            // via the X button, handled above, doesn't reach this at all).
            if let tauri::RunEvent::Exit = event {
                if let Some(marker) = crash_marker_path() {
                    let _ = std::fs::remove_file(&marker);
                }
            }
        });
}

#[cfg(test)]
mod plugin_update_tests {
    use super::is_user_data_path;
    use std::path::Path;

    /// The published zip really does carry a copy of SteamTools.lua, and that
    /// file inside Steam is the user's installed-games list.
    #[test]
    fn refuses_to_overwrite_the_users_game_list() {
        assert!(is_user_data_path(Path::new("config/lua/SteamTools.lua")));
        assert!(is_user_data_path(Path::new("config/lua/730.lua")));
        assert!(is_user_data_path(Path::new("config/plugin/lua/anything.lua")));
    }

    /// A zip produced on Windows carries backslashes; it must be judged the
    /// same way.
    #[test]
    fn handles_windows_separators() {
        assert!(is_user_data_path(Path::new("config\\lua\\SteamTools.lua")));
    }

    /// Everything that is actually the plugin still gets installed.
    #[test]
    fn lets_the_plugin_files_through() {
        for p in [
            "OpenSteamTool.dll",
            "dwmapi.dll",
            "opensteamtool.toml",
            "opensteamtool/pattern/steamclient/abc.toml",
        ] {
            assert!(!is_user_data_path(Path::new(p)), "bloqueó {}", p);
        }
    }
}

#[cfg(test)]
mod config_backup_tests {
    use std::io::Read;

    /// A Steam folder holding more games than Ragnarok's registry knows about
    /// — the exact shape that lost a user eighty games. Measured on a real
    /// machine too: 12 files, 11 registry entries.
    fn fake_steam(dir: &std::path::Path, lua_names: &[&str], registered: &[&str]) {
        let lua = dir.join("config").join("lua");
        std::fs::create_dir_all(&lua).unwrap();
        for name in lua_names {
            std::fs::write(lua.join(name), b"addappid(1)\n").unwrap();
        }
        let map: std::collections::HashMap<&str, Vec<String>> =
            registered.iter().map(|id| (*id, Vec::new())).collect();
        std::fs::write(
            lua.join("ragnarok_apps.json"),
            serde_json::to_string(&map).unwrap(),
        )
        .unwrap();
    }

    fn entries_in(zip_path: &std::path::Path) -> Vec<String> {
        let f = std::fs::File::open(zip_path).unwrap();
        let mut a = zip::ZipArchive::new(f).unwrap();
        (0..a.len())
            .map(|i| a.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn the_backup_carries_every_lua_on_disk_not_just_the_registered_ones() {
        let base = std::env::temp_dir().join(format!("rl_backup_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // Three games present; the registry only ever heard of one of them.
        fake_steam(&base, &["100.lua", "200.lua", "300.lua"], &["100"]);

        let zip_path = base.join("out.zip");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(super::export_ragnarok_config(
            base.to_string_lossy().to_string(),
            vec!["100".to_string()],
            Vec::new(),
            "{}".to_string(),
            zip_path.to_string_lossy().to_string(),
        ))
        .unwrap();

        let names = entries_in(&zip_path);
        for expected in ["lua/100.lua", "lua/200.lua", "lua/300.lua"] {
            assert!(names.contains(&expected.to_string()), "falta {} en {:?}", expected, names);
        }
        // The registry travels too, so the new PC does not start blind.
        assert!(names.contains(&"ragnarok_apps.json".to_string()), "{:?}", names);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Nothing may be bundled twice: a duplicate entry makes some unzip tools
    /// fail outright and is silently wrong in the rest.
    #[test]
    fn a_named_game_already_on_disk_is_not_bundled_twice() {
        let base = std::env::temp_dir().join(format!("rl_backup_dup_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        fake_steam(&base, &["100.lua"], &["100"]);

        let zip_path = base.join("out.zip");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(super::export_ragnarok_config(
            base.to_string_lossy().to_string(),
            vec!["100".to_string()],
            Vec::new(),
            "{}".to_string(),
            zip_path.to_string_lossy().to_string(),
        ))
        .unwrap();

        let names = entries_in(&zip_path);
        let count = names.iter().filter(|n| n.as_str() == "lua/100.lua").count();
        assert_eq!(count, 1, "duplicado en {:?}", names);

        let _ = std::fs::remove_dir_all(&base);
    }
}




#[cfg(test)]
mod sidecar_path_tests {
    use super::sidecar_path;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rl_sidecar_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A clean install: neither file exists yet. This is the case the old code
    /// got wrong — it fell through to stplug-in, so the FIRST write of a brand
    /// new machine landed in the legacy folder and everything followed it there.
    #[test]
    fn a_clean_install_uses_config_lua() {
        let steam = scratch("clean");
        let path = sidecar_path(steam.to_str().unwrap(), "ragnarok_apps.json");
        assert!(
            path.ends_with(r"config\lua\ragnarok_apps.json") || path.ends_with("config/lua/ragnarok_apps.json"),
            "esperaba config/lua, obtuve {}",
            path.display()
        );
        let _ = std::fs::remove_dir_all(&steam);
    }

    /// An existing registry in the legacy folder is moved, not abandoned.
    #[test]
    fn a_legacy_registry_is_migrated_not_lost() {
        let steam = scratch("legacy");
        let legacy_dir = steam.join("config").join("stplug-in");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(legacy_dir.join("ragnarok_apps.json"), br#"{"480":[]}"#).unwrap();

        let path = sidecar_path(steam.to_str().unwrap(), "ragnarok_apps.json");

        assert!(path.exists(), "el archivo migrado debe existir");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"480":[]}"#,
            "el contenido tiene que sobrevivir"
        );
        assert!(path.to_string_lossy().contains("lua"), "debe quedar en config/lua");
        // El original se conserva bajo otro nombre, no se borra.
        assert!(!legacy_dir.join("ragnarok_apps.json").exists());
        assert!(legacy_dir.join("ragnarok_apps.json.migrado").exists());

        let _ = std::fs::remove_dir_all(&steam);
    }

    /// Once it is in config/lua, nothing else is consulted.
    #[test]
    fn an_existing_lua_registry_wins() {
        let steam = scratch("both");
        let lua_dir = steam.join("config").join("lua");
        let legacy_dir = steam.join("config").join("stplug-in");
        std::fs::create_dir_all(&lua_dir).unwrap();
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(lua_dir.join("ragnarok_apps.json"), b"nuevo").unwrap();
        std::fs::write(legacy_dir.join("ragnarok_apps.json"), b"viejo").unwrap();

        let path = sidecar_path(steam.to_str().unwrap(), "ragnarok_apps.json");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nuevo");

        let _ = std::fs::remove_dir_all(&steam);
    }
}

#[cfg(test)]
mod webhook_host_tests {
    use super::is_discord_webhook_url;

    #[test]
    fn accepts_a_real_discord_webhook() {
        assert!(is_discord_webhook_url("https://discord.com/api/webhooks/123/abc"));
        assert!(is_discord_webhook_url("https://discordapp.com/api/webhooks/123/abc"));
    }

    /// The shapes a plain `contains`/`starts_with` check would have let past.
    #[test]
    fn rejects_lookalikes() {
        for url in [
            "https://evil.com/?x=https://discord.com/api/webhooks/1/a",
            "https://discord.com.evil.com/api/webhooks/1/a",
            "https://evildiscord.com/api/webhooks/1/a",
            "http://discord.com/api/webhooks/1/a", // sin TLS
            "https://discord.com/api/channels/1",  // no es un webhook
            "file:///C:/Windows/System32/config/SAM",
            "http://127.0.0.1:8080/",
            "no soy una url",
        ] {
            assert!(!is_discord_webhook_url(url), "debería rechazar: {}", url);
        }
    }
}

#[cfg(test)]
mod release_host_tests {
    use super::is_trusted_release_url;

    #[test]
    fn accepts_github_release_assets() {
        assert!(is_trusted_release_url(
            "https://github.com/RagnarokManifests/games/releases/download/v2.1.8/Ragnarok_2.1.8_x64-setup.exe"
        ));
        assert!(is_trusted_release_url(
            "https://objects.githubusercontent.com/github-production-release-asset/1/2"
        ));
    }

    #[test]
    fn rejects_anything_else() {
        // Plain http, even on the right host.
        assert!(!is_trusted_release_url("http://github.com/x/y/setup.exe"));
        // A lookalike host that merely contains the trusted one.
        assert!(!is_trusted_release_url("https://github.com.evil.tld/setup.exe"));
        assert!(!is_trusted_release_url("https://evil.tld/github.com/setup.exe"));
        // Credentials trick: the real host is still evil.tld.
        assert!(!is_trusted_release_url("https://github.com@evil.tld/setup.exe"));
        assert!(!is_trusted_release_url("not a url"));
        assert!(!is_trusted_release_url("file:///C:/setup.exe"));
    }
}

#[cfg(test)]
mod utf8_boundary_tests {
    use super::{floor_char_boundary, strip_prefix_ignore_ascii_case};

    /// The exact shape that crashed the Bypass tab: a byte offset landing
    /// inside a multi-byte character.
    #[test]
    fn a_window_never_cuts_a_character_in_half() {
        let html = "aaaá bbb";
        // Byte 4 is the second half of the 'á'.
        assert!(!html.is_char_boundary(4));
        let end = floor_char_boundary(html, 4);
        assert_eq!(end, 3);
        let _ = &html[..end]; // would have panicked before
    }

    #[test]
    fn a_window_past_the_end_is_the_end() {
        let s = "ñ";
        assert_eq!(floor_char_boundary(s, 999), s.len());
        let _ = &s[..floor_char_boundary(s, 999)];
    }

    #[test]
    fn an_ascii_offset_is_left_alone() {
        assert_eq!(floor_char_boundary("hola mundo", 4), 4);
    }

    /// A Discord message whose 17th byte falls inside an accented letter used
    /// to panic before it could even be rejected as "not the command".
    #[test]
    fn a_message_with_accents_is_rejected_not_a_panic() {
        for text in [
            "Buenas, alguien tiene el fix de algún juego?",
            "¿¿¿¿¿¿¿¿¿ hola ???",
            "🎮🎮🎮🎮🎮 fix porfa",
            "corto",
            "",
        ] {
            assert!(strip_prefix_ignore_ascii_case(text, "/RAGNAROK LEGENDS").is_none());
        }
    }

    #[test]
    fn the_real_command_still_matches_in_any_case() {
        assert_eq!(
            strip_prefix_ignore_ascii_case("/ragnarok legends Crimson Desert", "/RAGNAROK LEGENDS"),
            Some(" Crimson Desert")
        );
        assert_eq!(
            strip_prefix_ignore_ascii_case("/RAGNAROK LEGENDS  FIFA 23", "/RAGNAROK LEGENDS"),
            Some("  FIFA 23")
        );
    }
}

#[cfg(test)]
mod bypass_wrapper_tests {
    use super::{detect_common_root, is_release_wrapper, is_root_noise};
    use std::path::{Path, PathBuf};

    /// A throwaway game folder, optionally with some real subfolders in it.
    fn game_dir(tag: &str, subdirs: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rl_wrap_test_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for sub in subdirs {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        dir
    }

    /// The bug as reported on Discord: a generic wrapper folder shares no
    /// words with the archive name, so the old name-only test kept it and
    /// every file landed somewhere the game never reads.
    #[test]
    fn a_generic_wrapper_folder_is_stripped() {
        let game = game_dir("generic", &["bin64"]);
        for name in ["Crack", "Fix", "NoDenuvo", "Files", "CW.FIX"] {
            assert!(
                is_release_wrapper(name, "Some.Game.Update.v1.5", &game),
                "'{}' deberia desenvolverse",
                name
            );
        }
        let _ = std::fs::remove_dir_all(&game);
    }

    /// Crimson Desert: every entry sits under `bin64`, which is the game's own
    /// real folder. Stripping it put the files loose in the root and broke it.
    #[test]
    fn a_folder_the_game_already_has_is_kept() {
        let game = game_dir("real", &["bin64"]);
        assert!(!is_release_wrapper("bin64", "Crimson.Desert.Fix", &game));
        let _ = std::fs::remove_dir_all(&game);
    }

    /// Still kept when the game has not created it yet.
    #[test]
    fn a_known_engine_folder_is_kept_even_if_absent() {
        let game = game_dir("absent", &[]);
        for name in ["bin64", "Binaries", "Plugins", "Engine"] {
            assert!(
                !is_release_wrapper(name, "Some.Game.Update", &game),
                "'{}' es una carpeta real del juego",
                name
            );
        }
        let _ = std::fs::remove_dir_all(&game);
    }

    /// The original behaviour has to survive: Pragmata's file wraps everything
    /// in a folder named after the release itself.
    #[test]
    fn a_folder_named_after_the_release_is_still_stripped() {
        let game = game_dir("release", &[]);
        assert!(is_release_wrapper(
            "Pragmata Update v1.0",
            "Pragmata.Update.v1.0.CW.FIX",
            &game
        ));
        let _ = std::fs::remove_dir_all(&game);
    }

    /// A wrapper wins over the destination check when its name matches the
    /// release — otherwise a game with a coincidentally-named folder would
    /// keep a wrapper it should have dropped.
    #[test]
    fn the_release_name_beats_an_existing_folder() {
        let game = game_dir("collide", &["Pragmata"]);
        assert!(is_release_wrapper("Pragmata", "Pragmata.CW.FIX", &game));
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn one_file_inside_a_folder_still_finds_the_root() {
        let paths = vec![PathBuf::from("Crack/steam_api64.dll")];
        assert_eq!(detect_common_root(&paths).unwrap(), "Crack");
    }

    /// A loose file at the archive root is not a wrapping folder — treating it
    /// as one would strip the file out of existence.
    #[test]
    fn a_loose_file_is_never_a_root() {
        let paths = vec![PathBuf::from("steam_api64.dll")];
        assert!(detect_common_root(&paths).is_none());
    }

    /// A folder plus a real loose file at the root: the file belongs in the
    /// game folder as-is, so there is no single wrapper to strip.
    ///
    /// This used to use `readme.txt` as the second entry, which now counts as
    /// root noise on purpose — see `a_readme_beside_the_wrapper_does_not_defeat_it`.
    #[test]
    fn two_different_top_level_entries_have_no_common_root() {
        let paths = vec![PathBuf::from("bin64/x.dll"), PathBuf::from("setup.exe")];
        assert!(detect_common_root(&paths).is_none());
    }

    #[test]
    fn every_entry_under_one_folder_is_a_root() {
        let paths = vec![
            PathBuf::from("Fix/steam_api64.dll"),
            PathBuf::from("Fix/bin64/game.exe"),
        ];
        assert_eq!(detect_common_root(&paths).unwrap(), "Fix");
    }

    /// The case that survived the first fix: a wrapper folder with a readme
    /// sitting beside it. Two top-level entries meant "no common root", so the
    /// payload stayed one folder deep and the user had to move it by hand.
    #[test]
    fn a_readme_beside_the_wrapper_does_not_defeat_it() {
        for noise in ["readme.txt", "group.nfo", "file_id.diz", "site.url", "NOTES.md"] {
            let paths = vec![
                PathBuf::from("Fix/steam_api64.dll"),
                PathBuf::from("Fix/bin64/game.exe"),
                PathBuf::from(noise),
            ];
            assert_eq!(
                detect_common_root(&paths).as_deref(),
                Some(std::ffi::OsStr::new("Fix")),
                "'{}' no debería romper la detección",
                noise
            );
        }
    }

    /// Noise is only ignored at the root. A readme INSIDE the payload is a
    /// file the game may well want, and must not change the answer.
    #[test]
    fn a_readme_inside_the_payload_is_not_noise() {
        assert!(!is_root_noise(Path::new("Fix/readme.txt")));
        assert!(is_root_noise(Path::new("readme.txt")));
    }

    /// A real second payload folder still means no single wrapper.
    #[test]
    fn two_payload_folders_still_have_no_common_root() {
        let paths = vec![
            PathBuf::from("Fix/a.dll"),
            PathBuf::from("Crack/b.dll"),
            PathBuf::from("readme.txt"),
        ];
        assert!(detect_common_root(&paths).is_none());
    }
}
