//! Steam's own best-seller and most-played charts, used to put the games people
//! actually buy and play in front of a catalog of tens of thousands of titles.
//!
//! Both catalogs list everything they can get a ticket for, which is mostly
//! shovelware by count. These lists come from Valve, free and without a key:
//!
//!   store.steampowered.com/search/results?filter=topsellers|mostplayed&infinite=1
//!       the store's own search — what its Top Sellers and Most Played pages
//!       show — 100 per page, games only (`category1=998`)
//!   api.steampowered.com/ISteamChartsService/GetMostPlayedGames
//!   api.steampowered.com/IStoreTopSellersService/GetWeeklyTopSellers
//!       the charts API, capped at 100; only used when the search fails
//!
//! Saved for six hours. The charts move daily, and ten store requests on every
//! launch would be asking to be rate limited.

use crate::diag_log;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

const LIST_SIZE: usize = 500;
const PAGE: usize = 100;
const FRESH_SECS: u64 = 6 * 3600;
const CACHE: &str = "steam_rankings.json";

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Rankings {
    /// AppIDs, best-selling first.
    pub top_sellers: Vec<String>,
    /// AppIDs, most played first.
    pub most_played: Vec<String>,
    /// Unix seconds of the last successful download.
    pub updated_at: u64,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("com.ragnarok.launcher").join("catalog"))
        .unwrap_or_else(std::env::temp_dir)
        .join(CACHE)
}

fn read_cache() -> Option<Rankings> {
    serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()
}

fn write_cache(rankings: &Rankings) {
    let path = cache_path();
    let Ok(json) = serde_json::to_string(rankings) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Keeps the first occurrence of each id, in order.
fn push_unique(into: &mut Vec<String>, seen: &mut HashSet<String>, ids: impl IntoIterator<Item = String>) {
    for id in ids {
        if seen.insert(id.clone()) {
            into.push(id);
        }
    }
}

/// AppIDs out of one page of the store's search, in listed order.
///
/// A bundle row carries several ids separated by commas; it is not a game and
/// the pattern skips it.
fn appids_from_search_page(body: &Value) -> Vec<String> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"data-ds-appid="(\d+)""#).expect("patrón literal"));
    let html = body.get("results_html").and_then(|h| h.as_str()).unwrap_or("");
    re.captures_iter(html).map(|c| c[1].to_string()).collect()
}

/// AppIDs out of a charts API response, in rank order.
fn appids_from_charts(body: &Value) -> Vec<String> {
    body.pointer("/response/ranks")
        .and_then(|r| r.as_array())
        .map(|ranks| {
            ranks
                .iter()
                .filter_map(|r| r.get("appid").and_then(|a| a.as_u64()))
                .map(|a| a.to_string())
                .collect()
        })
        .unwrap_or_default()
}

async fn from_store_search(client: &reqwest::Client, filter: &str) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for start in (0..LIST_SIZE).step_by(PAGE) {
        let query: [(&str, String); 7] = [
            ("query", String::new()),
            ("start", start.to_string()),
            ("count", PAGE.to_string()),
            ("filter", filter.to_string()),
            ("category1", "998".to_string()),
            ("infinite", "1".to_string()),
            ("l", "english".to_string()),
        ];
        let page = async {
            let res = client
                .get("https://store.steampowered.com/search/results/")
                .query(&query)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("la tienda respondió {}", res.status()));
            }
            let body: Value = res.json().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(appids_from_search_page(&body))
        }
        .await;

        match page {
            Ok(page) if page.is_empty() => break,
            Ok(page) => push_unique(&mut ids, &mut seen, page),
            // Whatever arrived before a failure is still the top of the list.
            Err(e) if ids.is_empty() => return Err(e),
            Err(_) => break,
        }
    }
    ids.truncate(LIST_SIZE);
    if ids.is_empty() {
        Err("la tienda no devolvió juegos".to_string())
    } else {
        Ok(ids)
    }
}

async fn from_charts(client: &reqwest::Client, filter: &str) -> Result<Vec<String>, String> {
    let req = if filter == "topsellers" {
        client
            .get("https://api.steampowered.com/IStoreTopSellersService/GetWeeklyTopSellers/v1/")
            .query(&[(
                "input_json",
                r#"{"country_code":"US","context":{"language":"english","country_code":"US"},"page_start":0,"page_count":100}"#,
            )])
    } else {
        client.get("https://api.steampowered.com/ISteamChartsService/GetMostPlayedGames/v1/")
    };
    let body: Value = req
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    push_unique(&mut ids, &mut HashSet::new(), appids_from_charts(&body));
    if ids.is_empty() {
        Err("la API de Steam no devolvió juegos".to_string())
    } else {
        Ok(ids)
    }
}

async fn one_list(client: &reqwest::Client, filter: &str) -> Result<Vec<String>, String> {
    match from_store_search(client, filter).await {
        Ok(ids) => Ok(ids),
        Err(e) => {
            diag_log!("[rankings] La tienda no dio {} ({}); se usa la lista de 100 de la API.", filter, e);
            from_charts(client, filter).await
        }
    }
}

/// Steam's best sellers and most played, from the saved copy while it is fresh.
///
/// A list that cannot be downloaded keeps its previous copy, and nothing is
/// saved until both arrive — so a failed refresh is retried next time instead
/// of being treated as fresh for six hours.
pub async fn get(force: bool) -> Result<Rankings, String> {
    let cached = read_cache();
    if let Some(c) = &cached {
        let fresh = unix_now().saturating_sub(c.updated_at) < FRESH_SECS;
        if !force && fresh && !c.top_sellers.is_empty() && !c.most_played.is_empty() {
            return Ok(c.clone());
        }
    }

    let client = reqwest::Client::builder()
        .user_agent("Ragnarok-Launcher")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let (sellers, played) = tokio::join!(one_list(&client, "topsellers"), one_list(&client, "mostplayed"));

    let old = cached.unwrap_or_default();
    match (sellers, played) {
        (Ok(top_sellers), Ok(most_played)) => {
            let fresh = Rankings { top_sellers, most_played, updated_at: unix_now() };
            write_cache(&fresh);
            Ok(fresh)
        }
        (sellers, played) => {
            let merged = Rankings {
                top_sellers: sellers.unwrap_or(old.top_sellers),
                most_played: played.unwrap_or(old.most_played),
                updated_at: old.updated_at,
            };
            if merged.top_sellers.is_empty() && merged.most_played.is_empty() {
                Err("No se pudieron cargar los rankings de Steam.".to_string())
            } else {
                Ok(merged)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn store_search_ids_come_out_in_order_without_bundles() {
        let body = json!({
            "success": 1,
            "results_html": "<a href=\"x\" data-ds-appid=\"1867240\" data-ds-itemkey=\"App_1867240\"></a>\n<a data-ds-appid=\"730\"></a><a data-ds-appid=\"10,20,30\" data-ds-bundleid=\"5\"></a><a data-ds-appid=\"2402680\"></a>"
        });
        assert_eq!(appids_from_search_page(&body), ["1867240", "730", "2402680"]);
        assert!(appids_from_search_page(&json!({ "success": 1 })).is_empty());
    }

    #[test]
    fn chart_ids_come_out_in_rank_order() {
        let body = json!({ "response": { "ranks": [
            { "rank": 1, "appid": 730, "peak_in_game": 1194411 },
            { "rank": 2, "appid": 570 },
            { "rank": 3, "item": { "name": "sin appid arriba" } }
        ]}});
        assert_eq!(appids_from_charts(&body), ["730", "570"]);
        assert!(appids_from_charts(&json!({ "response": {} })).is_empty());
    }

    #[test]
    fn a_repeated_id_keeps_its_first_place() {
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        push_unique(&mut ids, &mut seen, ["1".to_string(), "2".to_string()]);
        push_unique(&mut ids, &mut seen, ["2".to_string(), "3".to_string()]);
        assert_eq!(ids, ["1", "2", "3"]);
    }
}
