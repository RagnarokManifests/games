//! Hubcap Manifest (hubcapmanifest.com) as a second catalog and ticket source
//! beside Ryuu.
//!
//! Talks to it directly with the user's own API key, no proxy in between. The
//! contract below is taken from two independent clients that already use it —
//! LuaTools (downloads) and ArcAdder (catalog, stats) — and every route was
//! checked to exist: each answers 401 without a key. The routes that answered
//! 200 without one (`/api/v1/games`, `/catalog`, …) turned out to be the site's
//! HTML page, not an API.
//!
//!   GET /api/v1/library?limit=..&offset=..   catalog, paginated        free
//!   GET /api/v1/user/stats                    daily usage and limit     free
//!   GET /api/v1/manifest/{appid}              ticket zip (or bare .lua) counts toward the daily limit
//!
//! The daily limit is not a fixed 25: it scales with the user's Discord role
//! and can be a custom figure, so it is always read from `user/stats` and
//! never assumed.

use crate::diag_log;
use crate::managers::game::GameMetadata;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const BASE: &str = "https://hubcapmanifest.com";

/// The largest page ArcAdder's client asks for; the API clamps above it.
const PAGE_SIZE: usize = 200;
/// A stop for a server that keeps returning full pages forever.
const MAX_PAGES: usize = 1000;
/// Ceiling on one sync. What has arrived is saved as it goes, so running out
/// only pauses the download: the next sync continues where this one stopped.
const CATALOG_BUDGET_SECS: u64 = 1800;
/// How many times in a row one page may come back 429 before giving up on the
/// rest of the catalog.
const MAX_THROTTLE_RETRIES: u32 = 6;
/// Its own file, separate from Ryuu's cache, so switching sources never blends
/// two catalogs into one.
pub const CATALOG_CACHE: &str = "ragnarok_catalog_cache_hubcap_v1.json";

/// Hubcap keys are `smm_` followed by 96 lowercase hex characters.
///
/// Checked locally before any request, so a key pasted with a stray space or
/// cut short is reported as exactly that instead of a round trip to a 401.
pub fn is_valid_key_format(key: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^smm_[0-9a-f]{96}$").expect("patrón literal"))
        .is_match(key.trim())
}

const BAD_KEY_FORMAT: &str =
    "La clave de Hubcap no tiene el formato correcto: debe ser smm_ seguido de 96 caracteres.";

fn is_app_id(app_id: &str) -> bool {
    !app_id.is_empty() && app_id.chars().all(|c| c.is_ascii_digit())
}

fn client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("Ragnarok-Launcher")
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| e.to_string())
}

/// Attaches the key both ways the two reference clients send it: ArcAdder uses
/// a Bearer header everywhere, LuaTools an `api_key` query parameter for
/// downloads and stats. Sending both bets against neither.
fn authed(req: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    req.bearer_auth(key.trim()).query(&[("api_key", key.trim())])
}

/// A download's status code, as something the user can act on.
///
/// Fix the key, wait until tomorrow, or use the other source: three different
/// actions, so three different sentences.
pub fn explain_status(status: u16) -> String {
    match status {
        401 | 403 => "Hubcap rechazó la clave: puede que haya vencido. Renuévala en hubcapmanifest.com y pégala en Ajustes → Fuente de juegos.".to_string(),
        429 => "No te quedan descargas de Hubcap por hoy.".to_string(),
        404 => "Hubcap no tiene este juego.".to_string(),
        s => format!("Hubcap respondió con un error ({}).", s),
    }
}

/// The same, for the free routes. A 429 there is plain rate limiting rather
/// than the daily quota, and a 404 is not "no such game".
fn explain_listing_status(status: u16) -> String {
    match status {
        401 | 403 => explain_status(status),
        429 => "Hubcap está limitando las peticiones. Intenta de nuevo en unos minutos.".to_string(),
        s => format!("Hubcap respondió con un error ({}).", s),
    }
}

// ── Downloads (count toward the daily limit) ────────────────────────────────

/// The ticket Hubcap has for `app_id`, as raw bytes. Nothing is written.
pub async fn download_game(key: &str, app_id: &str) -> Result<Vec<u8>, String> {
    // The id is pasted into a URL path; being sure of its shape first is what
    // makes that safe.
    if !is_app_id(app_id) {
        return Err(format!("AppID no válido: {}", app_id));
    }
    if !is_valid_key_format(key) {
        return Err(BAD_KEY_FORMAT.to_string());
    }

    let res = authed(client(300)?.get(format!("{}/api/v1/manifest/{}", BASE, app_id)), key)
        .send()
        .await
        .map_err(|e| format!("No se pudo conectar con Hubcap: {}", e))?;

    let status = res.status().as_u16();
    if status != 200 {
        return Err(explain_status(status));
    }

    let bytes = res
        .bytes()
        .await
        .map_err(|e| format!("La descarga desde Hubcap se cortó: {}", e))?;
    if bytes.is_empty() {
        return Err("Hubcap respondió sin contenido.".to_string());
    }
    Ok(bytes.to_vec())
}

// ── Usage (free) ─────────────────────────────────────────────────────────────

/// A key's daily allowance, with the remainder already worked out.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct HubcapUsage {
    pub daily_usage: i64,
    pub daily_limit: i64,
    pub remaining: i64,
    pub can_make_requests: bool,
    /// Naive UTC timestamp as Hubcap sends it (no zone designator).
    pub expires_at: Option<String>,
    /// The key is past its expiry date.
    pub expired: bool,
    /// Whole days until it expires; `None` when expired or when Hubcap sends
    /// no date. 0 means it expires within the day.
    pub days_left: Option<i64>,
    pub username: Option<String>,
}

/// Hubcap's expiry as naive UTC. It is sent without a zone designator; a zoned
/// value is accepted too, in case that changes.
fn parse_expiry(s: &str) -> Option<chrono::NaiveDateTime> {
    let s = s.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.naive_utc());
    }
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .iter()
        .find_map(|f| chrono::NaiveDateTime::parse_from_str(s, f).ok())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
        })
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Reads `user/stats` the way ArcAdder does: remaining is `daily_limit` minus
/// `daily_usage`, never below zero. Tolerant of missing fields and of numbers
/// sent as strings, because the shape is not documented anywhere public.
pub fn usage_from_json(v: &Value) -> HubcapUsage {
    usage_from_json_at(v, chrono::Utc::now().naive_utc())
}

/// The same, judged at `now` (UTC).
///
/// Expiry is worked out here so the window never has to parse a zoneless date
/// — read as local time it lands hours off, which on the last day of a key
/// decides whether it reads as valid or expired.
fn usage_from_json_at(v: &Value, now: chrono::NaiveDateTime) -> HubcapUsage {
    let daily_usage = v.get("daily_usage").and_then(as_i64).unwrap_or(0).max(0);
    let daily_limit = v.get("daily_limit").and_then(as_i64).unwrap_or(0).max(0);
    let remaining = (daily_limit - daily_usage).max(0);
    let can_make_requests = v
        .get("can_make_requests")
        .and_then(|x| x.as_bool())
        .unwrap_or(remaining > 0);
    let text = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let expires_at = text("api_key_expires_at");
    // An unreadable date never marks a key expired: that would tell someone
    // with a working key to go and renew it.
    let expiry = expires_at.as_deref().and_then(parse_expiry);
    let expired = expiry.map(|e| e <= now).unwrap_or(false);
    let days_left = expiry.filter(|_| !expired).map(|e| (e - now).num_days());
    HubcapUsage {
        daily_usage,
        daily_limit,
        remaining,
        can_make_requests,
        expires_at,
        expired,
        days_left,
        username: text("username"),
    }
}

/// Usage for a key. Also the cheapest way to learn whether a key works at all,
/// since it spends no downloads.
pub async fn usage(key: &str) -> Result<HubcapUsage, String> {
    if !is_valid_key_format(key) {
        return Err(BAD_KEY_FORMAT.to_string());
    }

    let res = authed(client(20)?.get(format!("{}/api/v1/user/stats", BASE)), key)
        .send()
        .await
        .map_err(|e| format!("No se pudo conectar con Hubcap: {}", e))?;

    let status = res.status().as_u16();
    if status != 200 {
        return Err(explain_listing_status(status));
    }

    let body: Value = res
        .json()
        .await
        .map_err(|e| format!("Hubcap devolvió una respuesta que no se pudo leer: {}", e))?;
    Ok(usage_from_json(&body))
}

// ── Catalog (free) ───────────────────────────────────────────────────────────

/// The array of entries in one library page. ArcAdder accepts the list under
/// `games`, under `results`, or as the root itself, so this does too.
fn page_items(body: &Value) -> Option<&Vec<Value>> {
    body.get("games")
        .and_then(|g| g.as_array())
        .or_else(|| body.get("results").and_then(|r| r.as_array()))
        .or_else(|| body.as_array())
}

fn value_to_id(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    is_app_id(&s).then_some(s)
}

/// Whether a library entry is a game, rather than a DLC, soundtrack, video or
/// tool. The catalog is shown in the Library as installable games, and listing
/// a soundtrack there invites an install that makes no sense.
fn is_game_entry(v: &Value) -> bool {
    let flag = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    if flag("is_dlc") || flag("is_music") || flag("is_software") || flag("is_video") {
        return false;
    }
    for k in ["parent_id", "dlc_for"] {
        if v.get(k).and_then(value_to_id).is_some() {
            return false;
        }
    }
    for k in ["type", "app_type", "category"] {
        if let Some(t) = v.get(k).and_then(|x| x.as_str()) {
            let t = t.trim().to_ascii_lowercase();
            if matches!(
                t.as_str(),
                "dlc" | "music" | "soundtrack" | "video" | "software" | "tool" | "demo"
            ) {
                return false;
            }
        }
    }
    true
}

/// One library page, as Library entries.
pub fn parse_library_page(body: &Value) -> Vec<GameMetadata> {
    let Some(items) = page_items(body) else { return Vec::new() };
    items
        .iter()
        .filter(|item| is_game_entry(item))
        .filter_map(|item| {
            let id = item
                .get("game_id")
                .and_then(value_to_id)
                .or_else(|| item.get("app_id").and_then(value_to_id))?;
            let name = item
                .get("game_name")
                .and_then(|x| x.as_str())
                .or_else(|| item.get("name").and_then(|x| x.as_str()))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("AppID {}", id));
            let updated_at = item
                .get("updated")
                .and_then(|x| x.as_str())
                .or_else(|| item.get("last_updated").and_then(|x| x.as_str()))
                .map(|s| s.to_string());
            Some(GameMetadata {
                image: Some(format!(
                    "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/header.jpg",
                    id
                )),
                id,
                name,
                developers: None,
                genres: None,
                metacritic: None,
                status: None,
                // `file_size` is the size of Hubcap's ticket zip, not of the
                // game, so it is deliberately not put where the Library shows
                // an install size.
                size_bytes: None,
                is_new: None,
                updated_at,
                nsfw: item_is_adult(item),
                added_at: None,
            })
        })
        .collect()
}

/// Where a load stands, sent to the window as `catalog_progress` events.
#[derive(Serialize, Debug, Clone)]
pub struct CatalogProgress {
    pub games: usize,
    pub page: usize,
    /// Seconds Hubcap asked to wait before the next page; 0 otherwise.
    pub waiting_secs: u64,
    /// What has arrived so far was just saved and can already be shown.
    pub checkpoint: bool,
    pub done: bool,
}

/// Pages between saves while a catalog downloads. Ten pages is 2,000 games:
/// often enough that closing the app loses little, rarely enough that
/// rewriting the file does not slow the download down.
const CHECKPOINT_PAGES: usize = 10;
/// A complete catalog is used as it is for this long before Hubcap is asked
/// what changed.
const FRESH_SECS: u64 = 24 * 3600;
/// Ryuu's catalog cache, read for the marks Hubcap's entries lack.
const RYUU_CACHE: &str = "ragnarok_catalog_cache_v6.json";
const ADULT_IDS: &str = "adult_app_ids.json";

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn catalog_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("com.ragnarok.launcher").join("catalog"))
        .unwrap_or_else(std::env::temp_dir)
}

/// The Hubcap catalog file the window reads.
///
/// Kept under the app's data folder, not %TEMP% where it started. Storage
/// Sense and every temp-cleaning tool empty %TEMP%, and under Hubcap's rate
/// limit the catalog takes minutes to download, so losing it meant the whole
/// wait again. A copy left in %TEMP% by the earlier version is moved over.
pub fn cache_path() -> PathBuf {
    let cache = catalog_dir().join(CATALOG_CACHE);
    let old = std::env::temp_dir().join(CATALOG_CACHE);
    if !cache.exists() && old.is_file() && old != cache {
        if let Some(parent) = cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Rename fails across drives; copy then.
        if std::fs::rename(&old, &cache).is_err() && std::fs::copy(&old, &cache).is_ok() {
            let _ = std::fs::remove_file(&old);
        }
    }
    cache
}

/// How far the saved catalog got, stored beside it.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
struct CatalogState {
    /// The whole list was walked to its last page.
    complete: bool,
    /// Where an unfinished walk continues.
    next_offset: usize,
    /// Unix seconds when the list was last known to be up to date.
    checked_at: u64,
}

#[derive(Debug, PartialEq)]
enum Plan {
    UseCache,
    Continue { offset: usize },
    Refresh,
}

/// What a sync has to do.
///
/// The catalog used to be thrown away far too easily: considered stale after
/// an hour, downloaded again in full every time the user switched catalogs,
/// and lost entirely when the app closed before the last page arrived — which,
/// with Hubcap's rate limit stretching a download to many minutes, was most
/// of the times anyone closed it. Now a download is only ever continued, and a
/// complete catalog is only ever checked for changes.
fn plan(state: Option<&CatalogState>, has_games: bool, force: bool, now: u64) -> Plan {
    match state {
        _ if !has_games => Plan::Continue { offset: 0 },
        // Saved before progress was tracked, and possibly cut short by the
        // rate limit: walked again, keeping everything it already has.
        None => Plan::Continue { offset: 0 },
        Some(s) if !s.complete => Plan::Continue { offset: s.next_offset },
        Some(s) if force || now.saturating_sub(s.checked_at) >= FRESH_SECS => Plan::Refresh,
        Some(_) => Plan::UseCache,
    }
}

fn state_path(cache: &Path) -> PathBuf {
    cache.with_extension("state.json")
}

fn read_state(cache: &Path) -> Option<CatalogState> {
    serde_json::from_str(&std::fs::read_to_string(state_path(cache)).ok()?).ok()
}

fn read_games(path: &Path) -> Vec<GameMetadata> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Temp file plus rename, so an interrupted write leaves the old file in
/// place instead of a truncated one.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// The list being built, indexed so each page merges in place.
struct Catalog {
    /// Games found ahead of everything already known, while refreshing.
    front: Vec<GameMetadata>,
    games: Vec<GameMetadata>,
    /// id → (lives in `front`, position)
    index: HashMap<String, (bool, usize)>,
}

impl Catalog {
    fn new(known: Vec<GameMetadata>) -> Self {
        let mut c = Catalog {
            front: Vec::new(),
            games: Vec::with_capacity(known.len()),
            index: HashMap::new(),
        };
        for g in known {
            if !c.index.contains_key(&g.id) {
                c.index.insert(g.id.clone(), (false, c.games.len()));
                c.games.push(g);
            }
        }
        c
    }

    fn len(&self) -> usize {
        self.front.len() + self.games.len()
    }

    /// Adds one page and returns how many games were new or changed. A game
    /// already listed is updated where it is — never added a second time.
    fn merge(&mut self, page: Vec<GameMetadata>, in_front: bool) -> usize {
        let mut touched = 0;
        for mut g in page {
            match self.index.get(&g.id).copied() {
                Some((front, i)) => {
                    let slot = if front { &mut self.front[i] } else { &mut self.games[i] };
                    if slot.name != g.name || slot.updated_at != g.updated_at {
                        g.nsfw = g.nsfw.or(slot.nsfw);
                        if g.genres.is_none() {
                            g.genres = slot.genres.take();
                        }
                        *slot = g;
                        touched += 1;
                    }
                }
                None => {
                    let list = if in_front { &mut self.front } else { &mut self.games };
                    self.index.insert(g.id.clone(), (in_front, list.len()));
                    list.push(g);
                    touched += 1;
                }
            }
        }
        touched
    }

    fn snapshot(&self, marks: &Marks) -> Vec<GameMetadata> {
        self.front.iter().chain(self.games.iter()).map(|g| marks.apply(g)).collect()
    }
}

/// What Ryuu's catalog knows that Hubcap's entries do not say: which games
/// are +18, and their genres.
///
/// Hubcap's library route lists a name and an id. Without these every adult
/// game landed in Games for anyone on Hubcap, and the genre filter was empty.
#[derive(Default)]
struct Marks {
    adult: HashSet<String>,
    genres: HashMap<String, Vec<String>>,
}

impl Marks {
    fn load() -> Self {
        let mut marks = Marks::default();
        if let Some(ids) = std::fs::read_to_string(catalog_dir().join(ADULT_IDS))
            .ok()
            .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
        {
            marks.adult.extend(ids);
        }
        let ryuu = read_games(&std::env::temp_dir().join(RYUU_CACHE));
        if !ryuu.is_empty() {
            remember_adult_ids(&ryuu);
            for g in ryuu {
                if g.nsfw == Some(true) {
                    marks.adult.insert(g.id.clone());
                }
                if let Some(genres) = g.genres {
                    marks.genres.insert(g.id, genres);
                }
            }
        }
        marks
    }

    fn apply(&self, g: &GameMetadata) -> GameMetadata {
        let mut g = g.clone();
        if self.adult.contains(&g.id) {
            g.nsfw = Some(true);
        }
        if g.genres.is_none() {
            if let Some(genres) = self.genres.get(&g.id) {
                g.genres = Some(genres.clone());
            }
        }
        g
    }
}

/// Keeps which games Ryuu's catalog marks +18 somewhere cleaning %TEMP%
/// cannot reach, for the Hubcap catalog to use. Called whenever Ryuu's
/// catalog is saved.
pub fn remember_adult_ids(games: &[GameMetadata]) {
    // A short list is a failed or partial load, not Ryuu's real catalog, and
    // must not replace a good list.
    if cfg!(test) || games.len() < 500 {
        return;
    }
    let mut ids: Vec<&str> = games
        .iter()
        .filter(|g| g.nsfw == Some(true))
        .map(|g| g.id.as_str())
        .collect();
    if ids.is_empty() {
        return;
    }
    ids.sort_unstable();
    if let Ok(json) = serde_json::to_string(&ids) {
        let _ = write_atomic(&catalog_dir().join(ADULT_IDS), json.as_bytes());
    }
}

/// Hubcap's own adult flag, under whichever name an entry carries one.
/// Nothing guarantees the library route sends it; the marks from Ryuu's
/// catalog are what reliably fill the +18 tab.
fn item_is_adult(item: &Value) -> Option<bool> {
    ["nsfw", "is_nsfw", "adult", "is_adult", "mature"]
        .iter()
        .any(|k| item.get(*k).and_then(|x| x.as_bool()).unwrap_or(false))
        .then_some(true)
}

fn save(cache: &Path, catalog: &Catalog, marks: &Marks, state: &CatalogState) -> Result<(), String> {
    let json = serde_json::to_string(&catalog.snapshot(marks)).map_err(|e| e.to_string())?;
    write_atomic(cache, json.as_bytes())?;
    let state_json = serde_json::to_string(state).map_err(|e| e.to_string())?;
    write_atomic(&state_path(cache), state_json.as_bytes())
}

#[derive(Debug)]
struct Walked {
    complete: bool,
    next_offset: usize,
}

/// Walks Hubcap's library from `start_offset`, merging each page into
/// `catalog`. Only the first page's failure is an error; a later one ends the
/// walk with what it has, to be continued next time.
async fn walk_library(
    key: &str,
    catalog: &mut Catalog,
    start_offset: usize,
    refresh: bool,
    checkpoint: &(dyn Fn(&Catalog, usize) + Send + Sync),
    progress: &(dyn Fn(CatalogProgress) + Send + Sync),
) -> Result<Walked, String> {
    if !is_valid_key_format(key) {
        return Err(BAD_KEY_FORMAT.to_string());
    }

    let client = client(60)?;
    let deadline = Instant::now() + Duration::from_secs(CATALOG_BUDGET_SECS);
    let mut offset = start_offset;
    let mut pages = 0usize;
    let mut throttled = 0u32;
    let unfinished = |next_offset: usize| Walked { complete: false, next_offset };

    loop {
        if pages >= MAX_PAGES {
            diag_log!("[hubcap] {} páginas sin llegar al final; se sigue en la próxima carga.", MAX_PAGES);
            return Ok(unfinished(offset));
        }
        if Instant::now() > deadline {
            diag_log!(
                "[hubcap] Presupuesto de {}s agotado en el juego {}: se sigue en la próxima carga ({} juegos guardados).",
                CATALOG_BUDGET_SECS,
                offset,
                catalog.len()
            );
            return Ok(unfinished(offset));
        }

        let res = authed(client.get(format!("{}/api/v1/library", BASE)), key)
            .query(&[
                ("limit", PAGE_SIZE.to_string()),
                ("offset", offset.to_string()),
                ("sort_by", "updated".to_string()),
            ])
            .send()
            .await;

        let res = match res {
            Ok(r) => r,
            Err(e) if pages == 0 => return Err(format!("No se pudo conectar con Hubcap: {}", e)),
            Err(e) => {
                diag_log!("[hubcap] El juego {} falló ({}); se sigue en la próxima carga.", offset, e);
                return Ok(unfinished(offset));
            }
        };

        let status = res.status().as_u16();

        if status == 429 {
            // The library route is rate limited. Seen on a real key: seventy
            // pages arrived in about forty seconds, page 70 came back 429, and
            // the catalog stopped at 14,000 games as if that were all of it.
            // Waiting and retrying the same page gets the rest.
            throttled += 1;
            let wait = retry_after(&res)
                .unwrap_or_else(|| Duration::from_secs(15 * throttled as u64))
                .min(Duration::from_secs(90));
            if throttled > MAX_THROTTLE_RETRIES || Instant::now() + wait > deadline {
                diag_log!(
                    "[hubcap] El juego {} sigue limitado tras {} intento(s); se sigue en la próxima carga ({} juegos guardados).",
                    offset,
                    throttled,
                    catalog.len()
                );
                if pages == 0 {
                    return Err(explain_listing_status(429));
                }
                return Ok(unfinished(offset));
            }
            diag_log!(
                "[hubcap] Juego {} limitado (429); se reintenta en {}s ({} juegos hasta ahora).",
                offset,
                wait.as_secs(),
                catalog.len()
            );
            progress(CatalogProgress {
                games: catalog.len(),
                page: offset / PAGE_SIZE,
                waiting_secs: wait.as_secs(),
                checkpoint: false,
                done: false,
            });
            tokio::time::sleep(wait).await;
            continue;
        }
        throttled = 0;

        if status != 200 {
            if pages == 0 {
                return Err(explain_listing_status(status));
            }
            diag_log!("[hubcap] El juego {} respondió {}; se sigue en la próxima carga.", offset, status);
            return Ok(unfinished(offset));
        }

        let body: Value = match res.json().await {
            Ok(b) => b,
            Err(e) if pages == 0 => {
                return Err(format!("Hubcap devolvió un catálogo que no se pudo leer: {}", e))
            }
            Err(_) => return Ok(unfinished(offset)),
        };

        // Counted before filtering: a page made entirely of DLC is still a
        // full page, and stopping on it would cut the catalog short.
        let raw_len = page_items(&body).map(|v| v.len()).unwrap_or(0);
        let touched = catalog.merge(parse_library_page(&body), refresh);
        pages += 1;
        offset += PAGE_SIZE;

        let last = raw_len < PAGE_SIZE;
        // Hubcap lists recently updated games first, so while refreshing, a
        // page with nothing new or changed means the rest is already known.
        let caught_up = refresh && touched == 0;
        let saved = !refresh && !last && pages % CHECKPOINT_PAGES == 0;
        if saved {
            checkpoint(catalog, offset);
        }
        progress(CatalogProgress {
            games: catalog.len(),
            page: offset / PAGE_SIZE,
            waiting_secs: 0,
            checkpoint: saved,
            done: false,
        });
        if last || caught_up {
            return Ok(Walked { complete: true, next_offset: 0 });
        }
    }
}

/// The server's own wait, when it says how long in whole seconds.
fn retry_after(res: &reqwest::Response) -> Option<Duration> {
    res.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Re-applies the +18 marks when Ryuu's list changed after this catalog was
/// saved, so a catalog that is otherwise fresh does not keep adult games in
/// Games until tomorrow.
fn apply_new_marks(cache: &Path, known: Vec<GameMetadata>, state: CatalogState) {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let marks_changed = [catalog_dir().join(ADULT_IDS), std::env::temp_dir().join(RYUU_CACHE)]
        .iter()
        .filter_map(|p| modified(p))
        .max();
    if let (Some(marks_at), Some(cache_at)) = (marks_changed, modified(cache)) {
        if marks_at > cache_at {
            if let Err(e) = save(cache, &Catalog::new(known), &Marks::load(), &state) {
                diag_log!("[hubcap] No se pudieron aplicar las marcas +18 al catálogo: {}", e);
            }
        }
    }
}

/// Brings the Hubcap catalog up to date and returns the file holding it, in
/// the same shape and the same hand-off Ryuu's catalog uses.
///
/// When Hubcap cannot be reached, the saved copy is handed back rather than
/// an error — a catalog from yesterday beats an empty Library.
pub async fn sync_catalog(
    key: &str,
    force: bool,
    progress: &(dyn Fn(CatalogProgress) + Send + Sync),
) -> Result<PathBuf, String> {
    let cache = cache_path();
    let known = read_games(&cache);
    let state = read_state(&cache);

    let (start, refresh) = match plan(state.as_ref(), !known.is_empty(), force, unix_now()) {
        Plan::UseCache => {
            apply_new_marks(&cache, known, state.unwrap_or_default());
            return Ok(cache);
        }
        Plan::Continue { offset } => (offset, false),
        Plan::Refresh => (0, true),
    };

    if refresh {
        diag_log!("[hubcap] Buscando cambios en el catálogo guardado ({} juegos).", known.len());
    } else {
        diag_log!("[hubcap] Descargando catálogo desde el juego {} ({} ya guardados).", start, known.len());
    }

    let marks = Marks::load();
    let previous_check = state.as_ref().map(|s| s.checked_at).unwrap_or(0);
    let mut catalog = Catalog::new(known);
    let checkpoint = |c: &Catalog, next_offset: usize| {
        let s = CatalogState { complete: false, next_offset, checked_at: previous_check };
        if let Err(e) = save(&cache, c, &marks, &s) {
            diag_log!("[hubcap] No se pudo guardar el avance del catálogo: {}", e);
        }
    };

    let stale_or = |reason: String| -> Result<PathBuf, String> {
        if read_games(&cache).is_empty() {
            // Logged here too. This is the failure a first switch to Hubcap
            // hits — no older copy to fall back on.
            diag_log!("[hubcap] No se pudo cargar el catálogo y no hay copia guardada: {}", reason);
            Err(reason)
        } else {
            diag_log!("[hubcap] {} — se usa el catálogo guardado.", reason);
            Ok(cache.clone())
        }
    };

    let walked = match walk_library(key, &mut catalog, start, refresh, &checkpoint, progress).await {
        Ok(w) => w,
        Err(e) => return stale_or(e),
    };
    if catalog.len() == 0 {
        return stale_or("Hubcap devolvió un catálogo vacío.".to_string());
    }

    let state = match (walked.complete, refresh) {
        (true, _) => CatalogState { complete: true, next_offset: 0, checked_at: unix_now() },
        // A refresh cut short leaves a complete list complete; it is simply
        // asked again next time.
        (false, true) => CatalogState { complete: true, next_offset: 0, checked_at: previous_check },
        (false, false) => CatalogState {
            complete: false,
            next_offset: walked.next_offset,
            checked_at: previous_check,
        },
    };
    save(&cache, &catalog, &marks, &state)
        .map_err(|e| format!("No se pudo guardar el catálogo de Hubcap: {}", e))?;
    diag_log!(
        "[hubcap] Catálogo guardado: {} juegos ({}).",
        catalog.len(),
        if state.complete { "completo".to_string() } else { format!("sigue desde el juego {}", state.next_offset) }
    );
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(hex_len: usize) -> String {
        format!("smm_{}", "a1".repeat(hex_len / 2))
    }

    #[test]
    fn only_the_real_key_shape_is_accepted() {
        assert!(is_valid_key_format(&key(96)));
        // Pasted with surrounding whitespace, which is how keys usually arrive.
        assert!(is_valid_key_format(&format!("  {}\n", key(96))));

        assert!(!is_valid_key_format(""));
        assert!(!is_valid_key_format(&key(94)), "una clave cortada no vale");
        assert!(!is_valid_key_format(&key(98)), "una clave con sobrante no vale");
        assert!(!is_valid_key_format(&key(96).replace("smm_", "sk_")), "prefijo equivocado");
        assert!(
            !is_valid_key_format(&key(96).to_uppercase().replace("SMM_", "smm_")),
            "Hubcap usa hex en minúsculas"
        );
    }

    #[test]
    fn each_failure_tells_the_user_what_to_do() {
        let bad_key = explain_status(401);
        let limit = explain_status(429);
        let missing = explain_status(404);
        assert!(bad_key.contains("clave"));
        assert!(limit.contains("descargas"));
        assert!(missing.contains("no tiene"));
        assert_ne!(bad_key, limit);
        assert_ne!(limit, missing);
        assert_eq!(explain_status(403), bad_key, "403 también es un problema de clave");

        // On the free routes a 429 is rate limiting, not the daily quota.
        assert_ne!(explain_listing_status(429), limit);
        assert!(!explain_listing_status(404).contains("no tiene"));
    }

    #[test]
    fn bad_input_never_reaches_the_network() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for bad_id in ["", "12 34", "../../etc", "4001890&x=1"] {
            let err = rt.block_on(download_game(&key(96), bad_id)).unwrap_err();
            assert!(err.contains("AppID"), "{bad_id:?}: {err}");
        }
        assert!(rt.block_on(download_game("smm_corta", "431960")).unwrap_err().contains("formato"));
        assert!(rt.block_on(usage("no-es-una-clave")).unwrap_err().contains("formato"));
        assert!(rt.block_on(walk_library("no-es-una-clave", &mut Catalog::new(Vec::new()), 0, false, &|_, _| {}, &|_| {})).unwrap_err().contains("formato"));
    }

    /// Remaining is worked out the way ArcAdder's dashboard does it, and never
    /// shown as a negative number when usage runs past the limit.
    #[test]
    fn usage_reports_what_is_left_today() {
        let u = usage_from_json(&json!({
            "daily_usage": 7, "daily_limit": 25, "can_make_requests": true,
            "api_key_expires_at": "2026-10-01T12:00:00", "username": "sheaker"
        }));
        assert_eq!((u.daily_usage, u.daily_limit, u.remaining), (7, 25, 18));
        assert!(u.can_make_requests);
        assert_eq!(u.username.as_deref(), Some("sheaker"));

        let over = usage_from_json(&json!({ "daily_usage": 30, "daily_limit": 25 }));
        assert_eq!(over.remaining, 0);
        assert!(!over.can_make_requests, "sin campo explícito, se deduce de lo que queda");

        // A role with a bigger allowance, sent as strings.
        let role = usage_from_json(&json!({ "daily_usage": "10", "daily_limit": "100" }));
        assert_eq!(role.remaining, 90);
    }

    /// The expiry arrives without a zone and is UTC.
    #[test]
    fn an_expired_key_is_recognised() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 9, 15).unwrap().and_hms_opt(12, 0, 0).unwrap();
        let at = |exp: &str| usage_from_json_at(&json!({ "daily_limit": 25, "api_key_expires_at": exp }), now);

        let past = at("2026-09-14T08:00:00");
        assert!(past.expired);
        assert_eq!(past.days_left, None);

        let soon = at("2026-09-17T13:00:00.123456");
        assert!(!soon.expired);
        assert_eq!(soon.days_left, Some(2));

        let today = at("2026-09-15 18:00:00");
        assert!(!today.expired);
        assert_eq!(today.days_left, Some(0), "vence dentro del día");

        assert!(at("2026-09-15T11:00:00Z").expired, "con zona también");
        assert!(!at("mañana").expired, "una fecha ilegible no marca la clave como vencida");

        let none = usage_from_json_at(&json!({ "daily_limit": 25 }), now);
        assert!(!none.expired);
        assert_eq!(none.days_left, None);
    }

    /// The three list shapes ArcAdder accepts, and only games make it through.
    #[test]
    fn a_library_page_yields_only_games() {
        let entries = json!([
            { "game_id": "1245620", "game_name": "ELDEN RING", "updated": "2026-09-01" },
            { "app_id": 431960, "name": "Wallpaper Engine" },
            { "game_id": "2778580", "game_name": "Shadow of the Erdtree", "is_dlc": true },
            { "game_id": "1896300", "game_name": "OST", "type": "Music" },
            { "game_id": "999", "game_name": "Pack", "dlc_for": "1245620" },
            { "game_id": "no-es-numero", "game_name": "Roto" }
        ]);

        for body in [json!({ "games": entries }), json!({ "results": entries }), entries.clone()] {
            let games = parse_library_page(&body);
            let ids: Vec<&str> = games.iter().map(|g| g.id.as_str()).collect();
            assert_eq!(ids, vec!["1245620", "431960"], "{body}");
            assert_eq!(games[0].name, "ELDEN RING");
            assert_eq!(games[0].updated_at.as_deref(), Some("2026-09-01"));
            assert_eq!(games[1].name, "Wallpaper Engine", "acepta app_id numérico y name");
            assert!(games[0].size_bytes.is_none(), "file_size es el zip, no el juego");
        }

        assert!(parse_library_page(&json!({ "error": "nope" })).is_empty());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn game(id: &str, name: &str, updated: &str) -> GameMetadata {
        parse_library_page(&serde_json::json!([{ "game_id": id, "game_name": name, "updated": updated }])).remove(0)
    }

    fn ids(c: &Catalog) -> Vec<String> {
        c.snapshot(&Marks::default()).into_iter().map(|g| g.id).collect()
    }

    #[test]
    fn a_saved_catalog_is_not_downloaded_again() {
        let now = 1_000_000;
        let done = CatalogState { complete: true, next_offset: 0, checked_at: now - 60 };
        assert_eq!(plan(Some(&done), true, false, now), Plan::UseCache);
        // A day old: look for changes, never start over.
        let old = CatalogState { checked_at: now - FRESH_SECS, ..done.clone() };
        assert_eq!(plan(Some(&old), true, false, now), Plan::Refresh);
        // Switching catalogs forces a sync, and that is only a refresh.
        assert_eq!(plan(Some(&done), true, true, now), Plan::Refresh);
        // Closed halfway: continue from there, forced or not.
        let half = CatalogState { complete: false, next_offset: 14_000, checked_at: 0 };
        assert_eq!(plan(Some(&half), true, false, now), Plan::Continue { offset: 14_000 });
        assert_eq!(plan(Some(&half), true, true, now), Plan::Continue { offset: 14_000 });
        // Nothing usable on disk.
        assert_eq!(plan(Some(&half), false, false, now), Plan::Continue { offset: 0 });
        assert_eq!(plan(None, false, false, now), Plan::Continue { offset: 0 });
        // Saved by the version before progress was tracked: possibly cut short.
        assert_eq!(plan(None, true, false, now), Plan::Continue { offset: 0 });
    }

    #[test]
    fn merging_never_lists_a_game_twice() {
        let mut c = Catalog::new(vec![game("1", "A", "d1"), game("2", "B", "d1"), game("1", "A otra vez", "d1")]);
        assert_eq!(c.len(), 2, "una caché con repetidos se limpia al cargarla");
        assert_eq!(c.merge(vec![game("2", "B", "d1"), game("3", "C", "d1")], false), 1);
        assert_eq!(c.merge(vec![game("2", "B", "d2")], false), 1, "actualizado cuenta como cambio");
        assert_eq!(c.merge(vec![game("1", "A", "d1")], false), 0);
        assert_eq!(ids(&c), ["1", "2", "3"]);
    }

    #[test]
    fn a_refresh_puts_new_games_first() {
        let mut c = Catalog::new(vec![game("1", "A", "d1")]);
        assert_eq!(c.merge(vec![game("9", "Nuevo", "d9"), game("1", "A", "d1")], true), 1);
        assert_eq!(c.merge(vec![game("9", "Nuevo", "d9")], true), 0, "ya estaba al frente");
        assert_eq!(ids(&c), ["9", "1"]);
    }

    #[test]
    fn saved_games_carry_ryuus_adult_mark_and_genres() {
        let dir = std::env::temp_dir().join("ragnarok_hubcap_cache_test");
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join(CATALOG_CACHE);
        let mut marks = Marks::default();
        marks.adult.insert("2".into());
        marks.genres.insert("1".into(), vec!["Action".into()]);
        let c = Catalog::new(vec![game("1", "A", "d1"), game("2", "B", "d1")]);
        let state = CatalogState { complete: false, next_offset: 400, checked_at: 7 };
        save(&cache, &c, &marks, &state).unwrap();

        let back = read_games(&cache);
        assert_eq!(back[0].genres.as_deref(), Some(&["Action".to_string()][..]));
        assert_eq!(back[0].nsfw, None);
        assert_eq!(back[1].nsfw, Some(true), "+18 según Ryuu");
        assert_eq!(read_state(&cache), Some(state));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hubcaps_own_adult_flag_is_read_when_present() {
        let games = parse_library_page(&serde_json::json!([
            { "game_id": "1", "game_name": "A", "nsfw": true },
            { "game_id": "2", "game_name": "B" }
        ]));
        assert_eq!(games[0].nsfw, Some(true));
        assert_eq!(games[1].nsfw, None);
    }
}
