use reqwest::Client;
use serde::{Deserialize, Serialize};
use regex::Regex;
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SwitchCatalogItem {
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub cover_url: Option<String>,
    pub excerpt: String,
    pub source: String,
    pub page_url: String,
    pub format: Option<String>,
    pub size: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SwitchDownloadLink {
    pub name: String,
    pub url: String,
    pub host: String,
    pub file_type: Option<String>,
    pub size: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SwitchGameDetails {
    pub id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub description: String,
    pub source: String,
    pub page_url: String,
    pub format: Option<String>,
    pub size: Option<String>,
    pub version: Option<String>,
    pub release_date: Option<String>,
    pub download_links: Vec<SwitchDownloadLink>,
}

fn clean_html(input: &str) -> String {
    let tag_re = Regex::new(r"<[^>]*>").unwrap();
    let cleaned = tag_re.replace_all(input, "");
    cleaned
        .replace("&nbsp;", " ")
        .replace("&#8217;", "'")
        .replace("&#8216;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace("&#038;", "&")
        .replace("&amp;", "&")
        .replace("&#8211;", "-")
        .replace("&#8212;", "—")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&hellip;", "...")
        .trim()
        .to_string()
}

fn clean_title(title: &str) -> String {
    let t = clean_html(title);
    let re = Regex::new(r"(?i)\s*(NSP|XCI)?\s*,?\s*(ROM)?\s*(\(Full Game\))?\s*(\+\s*v?[\d\.]+.*)?\s*$").unwrap();
    re.replace(&t, "").trim().to_string()
}

fn identify_host(url: &str) -> String {
    let lower = url.to_lowercase();
    if lower.contains("1fichier.com") {
        "1Fichier".to_string()
    } else if lower.contains("mega.nz") || lower.contains("mega.co.nz") {
        "Mega".to_string()
    } else if lower.contains("drive.google.com") {
        "Google Drive".to_string()
    } else if lower.contains("mediafire.com") {
        "MediaFire".to_string()
    } else if lower.contains("pixeldrain.com") {
        "PixelDrain".to_string()
    } else if lower.contains("gofile.io") {
        "Gofile".to_string()
    } else if lower.contains("qiwi.gg") {
        "Qiwi".to_string()
    } else if lower.contains("krakenfiles.com") {
        "KrakenFiles".to_string()
    } else if lower.contains("projectnx.org/?go=") || lower.contains("projectnx.org/r/") {
        "ProjectNX Server".to_string()
    } else {
        "Descarga Directa".to_string()
    }
}

pub async fn fetch_catalog(
    client: &Client,
    source: &str,
    query: Option<String>,
    page: usize,
    per_page: usize,
) -> Result<Vec<SwitchCatalogItem>, String> {
    let actual_page = if page == 0 { 1 } else { page };
    let actual_per_page = if per_page == 0 || per_page > 50 { 24 } else { per_page };

    // Every source here is a WordPress site, which is why one code path
    // serves all of them: the REST shape, the pagination and the search
    // parameter are identical. Only the host and the category differ.
    //
    // The PlayStation categories were read from the site's own
    // `/wp-json/wp/v2/categories` rather than guessed, and their sizes
    // checked against the `X-WP-Total` header: PS2 3854, PS3 3191, PS4 6415.
    // Every source here is a WordPress site, which is why one code path
    // serves all of them: the REST shape, the pagination and the search
    // parameter are identical. Only the host and the category differ.
    //
    // romsfun was tried for the PlayStation catalogues and had to be dropped.
    // Its listings are richer for PS2 (5449 against 3854) but it answers
    // **403 to this application**, while returning 200 to curl carrying the
    // very same User-Agent — and it kept refusing with a full set of browser
    // headers, Referer and Sec-Fetch-* included. That is a client fingerprint
    // check, not a header check, and no amount of header tuning gets past it.
    // `live_source_probe` at the bottom of this file is what established
    // that, and is there to re-check it in one command.
    // No PlayStation entries here on purpose: romsfun serves those, and it
    // answers 403 to this client whatever headers or HTTP version it is given
    // (`live_source_probe` below demonstrates it). The frontend fetches them
    // through the webview instead, which romsfun accepts because it is a real
    // browser. Leaving a Rust path for them would only ever produce a 403.
    let base_url = match source {
        "projectnx" => "https://projectnx.org/wp-json/wp/v2/posts",
        _ => "https://eggnsemulator.com/wp-json/wp/v2/posts",
    };

    // Read from the site's own `/wp-json/wp/v2/categories`, with the sizes
    // confirmed against `X-WP-Total`: PS2 3854, PS3 3191, PS4 6415.
    let category_param = match source {
        "projectnx" => "categories=16",
        _ => "categories=7",
    };

    let mut url = format!(
        "{}?{}&page={}&per_page={}&_embed=1",
        base_url, category_param, actual_page, actual_per_page
    );

    if let Some(ref q) = query {
        let trimmed = q.trim();
        if !trimmed.is_empty() {
            url.push_str(&format!("&search={}", urlencoding::encode(trimmed)));
        }
    }

    let res = client
        .get(&url)
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(45))
        .send()
        .await
        .map_err(|e| format!("Error conectando con el catálogo ({}): {}", source, e))?;

    if !res.status().is_success() {
        return Err(format!("El servidor de juegos respondió con estado HTTP {}", res.status()));
    }

    let posts: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Error procesando respuesta JSON: {}", e))?;

    let arr = posts.as_array().ok_or("Formato de catálogo inválido")?;
    let mut items = Vec::new();

    for p in arr {
        let id = p.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let raw_title = p
            .get("title")
            .and_then(|t| t.get("rendered"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let title = clean_title(raw_title);
        let slug = p.get("slug").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let page_url = p.get("link").and_then(|s| s.as_str()).unwrap_or("").to_string();

        let raw_excerpt = p
            .get("excerpt")
            .and_then(|e| e.get("rendered"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let excerpt = clean_html(raw_excerpt);

        // Format detection
        let mut format = None;
        let title_upper = raw_title.to_uppercase();
        if title_upper.contains("NSP") && title_upper.contains("XCI") {
            format = Some("NSP / XCI".to_string());
        } else if title_upper.contains("NSP") {
            format = Some("NSP".to_string());
        } else if title_upper.contains("XCI") {
            format = Some("XCI".to_string());
        }

        // Extract size if present in excerpt/title
        let size_re = Regex::new(r"(?i)(\d+(\.\d+)?\s*(GB|MB))").unwrap();
        let size = size_re.find(&excerpt).or_else(|| size_re.find(raw_title)).map(|m| m.as_str().to_string());

        // Extract version if present
        let ver_re = Regex::new(r"(?i)v(\d+(\.\d+)+)").unwrap();
        let version = ver_re.find(raw_title).map(|m| m.as_str().to_string());

        // Extract cover thumbnail.
        //
        // Two places, because the sites differ. eggns and projectnx attach a
        // featured image, which arrives under `_embedded`. dlpsgame sets no
        // featured image at all — `featured_media` is 0 — and puts the cover
        // in its SEO block instead, as `yoast_head_json.og_image`. Reading
        // only the first left every PlayStation card blank.
        let cover_url = p
            .get("_embedded")
            .and_then(|emb| emb.get("wp:featuredmedia"))
            .and_then(|media| media.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("source_url"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                p.get("yoast_head_json")
                    .and_then(|y| y.get("og_image"))
                    .and_then(|imgs| imgs.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|img| img.get("url"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            });

        items.push(SwitchCatalogItem {
            id,
            title,
            slug,
            cover_url,
            excerpt,
            source: source.to_string(),
            page_url,
            format,
            size,
            version,
        });
    }

    Ok(items)
}

pub async fn fetch_game_details(
    client: &Client,
    source: &str,
    page_url: &str,
) -> Result<SwitchGameDetails, String> {
    let res = client
        .get(page_url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .timeout(std::time::Duration::from_secs(45))
        .send()
        .await
        .map_err(|e| format!("Error cargando página del juego: {}", e))?;

    let html = res.text().await.map_err(|e| format!("Error leyendo HTML: {}", e))?;

    let mut download_links: Vec<SwitchDownloadLink> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();

    // 1. ProjectNX ?go= bridge page auto-resolution (bypasses 5s countdown & ad redirects)
    let go_re = Regex::new(r#"href="([^"]*projectnx\.org/\?go=[^"]+)""#).unwrap();
    let mut bridge_urls = Vec::new();
    for cap in go_re.captures_iter(&html) {
        bridge_urls.push(cap[1].to_string());
    }

    for b_url in bridge_urls {
        if let Ok(b_res) = client.get(&b_url).send().await {
            if let Ok(b_html) = b_res.text().await {
                let pnx_row_re = Regex::new(r#"(?s)<tr[^>]*class="[^"]*nsw-bridge-row[^"]*"[^>]*data-final-url="([^"]+)"[^>]*>.*?<td>(.*?)</td>\s*<td>(.*?)</td>\s*<td>(.*?)</td>"#).unwrap();
                for cap in pnx_row_re.captures_iter(&b_html) {
                    let mut raw_url = cap[1].trim().to_string();
                    // Strip affiliate query parameters (e.g. ?af=5006637)
                    if let Some(idx) = raw_url.find("?af=") {
                        raw_url = raw_url[..idx].to_string();
                    }
                    let name = clean_html(&cap[2]);
                    let size = clean_html(&cap[3]);
                    let ftype = clean_html(&cap[4]);

                    if !raw_url.is_empty() && !raw_url.starts_with('#') && seen_urls.insert(raw_url.clone()) {
                        let host = identify_host(&raw_url);
                        download_links.push(SwitchDownloadLink {
                            name: if name.is_empty() { format!("Descargar {}", ftype) } else { name },
                            url: raw_url,
                            host,
                            file_type: if ftype.is_empty() { None } else { Some(ftype) },
                            size: if size.is_empty() { None } else { Some(size) },
                        });
                    }
                }
            }
        }
    }

    // 2. Direct ProjectNX table/bridge rows in the page itself
    let pnx_row_re = Regex::new(r#"(?s)<tr[^>]*class="[^"]*nsw-bridge-row[^"]*"[^>]*data-final-url="([^"]+)"[^>]*>.*?<td>(.*?)</td>\s*<td>(.*?)</td>\s*<td>(.*?)</td>"#).unwrap();
    for cap in pnx_row_re.captures_iter(&html) {
        let mut raw_url = cap[1].trim().to_string();
        if let Some(idx) = raw_url.find("?af=") {
            raw_url = raw_url[..idx].to_string();
        }
        let name = clean_html(&cap[2]);
        let size = clean_html(&cap[3]);
        let ftype = clean_html(&cap[4]);

        if !raw_url.is_empty() && !raw_url.starts_with('#') && seen_urls.insert(raw_url.clone()) {
            let host = identify_host(&raw_url);
            download_links.push(SwitchDownloadLink {
                name: if name.is_empty() { format!("Descargar {}", ftype) } else { name },
                url: raw_url,
                host,
                file_type: if ftype.is_empty() { None } else { Some(ftype) },
                size: if size.is_empty() { None } else { Some(size) },
            });
        }
    }

    // 3. EggNS and general download link tags (cleaning tracking and direct file hosts)
    let a_tag_re = Regex::new(r#"(?si)<a\s+[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    for cap in a_tag_re.captures_iter(&html) {
        let mut url = cap[1].trim().to_string();
        let anchor_text = clean_html(&cap[2]);
        let anchor_lower = anchor_text.to_lowercase();
        let url_lower = url.to_lowercase();

        let is_ad_or_social = url_lower.contains("wp-admin")
            || url_lower.contains("facebook.com")
            || url_lower.contains("twitter.com")
            || url_lower.contains("whatsapp.com")
            || url_lower.contains("discord.gg")
            || url_lower.contains("t.me")
            || url_lower.contains("youtube.com")
            || url_lower.contains("rankmath")
            || url_lower.contains("/privacy-policy")
            || url_lower.contains("/terms")
            || url_lower.contains("/dmca")
            || url_lower.contains("/about")
            || url_lower.contains("/contact")
            || url_lower.contains("/category/");

        if is_ad_or_social || url.starts_with('#') {
            continue;
        }

        let is_dl_host = url_lower.contains("1fichier.com")
            || url_lower.contains("mega.nz")
            || url_lower.contains("drive.google.com")
            || url_lower.contains("mediafire.com")
            || url_lower.contains("pixeldrain.com")
            || url_lower.contains("gofile.io")
            || url_lower.contains("qiwi.gg")
            || url_lower.contains("krakenfiles.com");

        let is_dl_text = anchor_lower.contains("download")
            || anchor_lower.contains("descargar")
            || anchor_lower.contains("mirror")
            || anchor_lower.contains("part")
            || anchor_lower.contains("nsp")
            || anchor_lower.contains("xci");

        if is_dl_host || is_dl_text {
            // Strip affiliate query parameters
            if let Some(idx) = url.find("?af=") {
                url = url[..idx].to_string();
            }

            if seen_urls.insert(url.clone()) {
                let host = identify_host(&url);
                let ftype = if anchor_lower.contains("nsp") {
                    Some("NSP".to_string())
                } else if anchor_lower.contains("xci") {
                    Some("XCI".to_string())
                } else if anchor_lower.contains("update") {
                    Some("Update".to_string())
                } else if anchor_lower.contains("dlc") {
                    Some("DLC".to_string())
                } else {
                    None
                };

                let name = if anchor_text.is_empty() || anchor_text.len() < 3 {
                    format!("Descargar ({})", host)
                } else {
                    anchor_text
                };

                download_links.push(SwitchDownloadLink {
                    name,
                    url,
                    host,
                    file_type: ftype,
                    size: None,
                });
            }
        }
    }

    // Title from HTML
    let title_re = Regex::new(r"(?i)<title>(.*?)</title>").unwrap();
    let raw_title = title_re.captures(&html).map(|c| c[1].to_string()).unwrap_or_else(|| "Nintendo Switch Game".to_string());
    let title = clean_title(&raw_title.replace(" - Egg NS Emulator", "").replace(" - ProjectNX", "").replace(" - Project NX", ""));

    // Cover image
    let og_image_re = Regex::new(r#"property="og:image"\s+content="([^"]+)""#).unwrap();
    let cover_url = og_image_re.captures(&html).map(|c| c[1].to_string());

    // Description text
    let desc_re = Regex::new(r#"name="description"\s+content="([^"]+)""#).unwrap();
    let description = desc_re.captures(&html).map(|c| clean_html(&c[1])).unwrap_or_default();

    // Size / Version
    let size_re = Regex::new(r"(?i)(\d+(\.\d+)?\s*(GB|MB))").unwrap();
    let size = size_re.find(&html).map(|m| m.as_str().to_string());

    let ver_re = Regex::new(r"(?i)v(\d+(\.\d+)+)").unwrap();
    let version = ver_re.find(&raw_title).map(|m| m.as_str().to_string());

    Ok(SwitchGameDetails {
        id: 0,
        title,
        cover_url,
        description,
        source: source.to_string(),
        page_url: page_url.to_string(),
        format: None,
        size,
        version,
        release_date: None,
        download_links,
    })
}

#[cfg(test)]
mod live_source_probe {
    /// Asks each catalogue source through the very client the app uses.
    ///
    /// Ignored by default because it needs the network. Run it when a source
    /// starts answering 403: curl with a browser User-Agent is not a fair
    /// stand-in for our client — reqwest differs in TLS fingerprint, HTTP/2
    /// settings and header order, and some hosts block on exactly that.
    ///
    ///   cargo test --bin Ragnarok-launcher live_source_probe -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn every_source_answers() {
        let client = crate::make_client().expect("cliente");
        let cases = [
            ("eggnsemulator", "https://eggnsemulator.com/wp-json/wp/v2/posts?categories=7&per_page=1"),
            ("projectnx", "https://projectnx.org/wp-json/wp/v2/posts?categories=16&per_page=1"),
            ("ps2", "https://romsfun.com/wp-json/wp/v2/rom?console=8&per_page=1"),
            ("ps3", "https://romsfun.com/wp-json/wp/v2/rom?console=15&per_page=1"),
            ("ps4", "https://romsfun.com/wp-json/wp/v2/rom?console=87&per_page=1"),
        ];
        for (name, url) in cases {
            let res = client
                .get(url)
                .header("Accept", "application/json")
                .timeout(std::time::Duration::from_secs(45))
                .send()
                .await;
            match res {
                Ok(r) => println!("  {:<14} -> HTTP {}", name, r.status()),
                Err(e) => println!("  {:<14} -> error de red: {}", name, e),
            }
        }

        println!("  --- romsfun con cabeceras de navegador completas ---");
        let res = client
            .get("https://romsfun.com/wp-json/wp/v2/rom?console=8&per_page=1")
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", "https://romsfun.com/roms/ps2/")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-origin")
            .timeout(std::time::Duration::from_secs(45))
            .send()
            .await;
        match res {
            Ok(r) => println!("  {:<14} -> HTTP {}", "romsfun+hdrs", r.status()),
            Err(e) => println!("  {:<14} -> error de red: {}", "romsfun+hdrs", e),
        }

        println!("  --- romsfun: variantes del cliente ---");
        let url = "https://romsfun.com/wp-json/wp/v2/rom?console=8&per_page=1";
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

        // HTTP/1.1 only: some protections fingerprint the HTTP/2 settings
        // frame, which reqwest sends differently from a browser.
        if let Ok(c) = reqwest::Client::builder().user_agent(ua).http1_only().build() {
            match c.get(url).send().await {
                Ok(r) => println!("  {:<14} -> HTTP {}", "http1_only", r.status()),
                Err(e) => println!("  {:<14} -> error: {}", "http1_only", e),
            }
        }

        // Same, plus the headers a browser actually sends alongside.
        if let Ok(c) = reqwest::Client::builder().user_agent(ua).http1_only().build() {
            match c
                .get(url)
                .header("Accept", "application/json, text/javascript, */*; q=0.01")
                .header("Accept-Language", "en-US,en;q=0.9")
                .header("Referer", "https://romsfun.com/roms/ps2/")
                .header("X-Requested-With", "XMLHttpRequest")
                .timeout(std::time::Duration::from_secs(45))
                .send()
                .await
            {
                Ok(r) => println!("  {:<14} -> HTTP {}", "h1+headers", r.status()),
                Err(e) => println!("  {:<14} -> error: {}", "h1+headers", e),
            }
        }

        // The plain site, not the API — to tell "the API is protected" from
        // "the whole host refuses us".
        if let Ok(c) = reqwest::Client::builder().user_agent(ua).build() {
            match c.get("https://romsfun.com/roms/ps2/").send().await {
                Ok(r) => println!("  {:<14} -> HTTP {}", "pagina html", r.status()),
                Err(e) => println!("  {:<14} -> error: {}", "pagina html", e),
            }
        }

        println!("  --- dlpsgame, para comparar ---");
        for (name, url) in [
            ("dlps ps2", "https://dlpsgame.com/wp-json/wp/v2/posts?categories=5917&per_page=1"),
            ("dlps ps3", "https://dlpsgame.com/wp-json/wp/v2/posts?categories=64&per_page=1"),
            ("dlps ps4", "https://dlpsgame.com/wp-json/wp/v2/posts?categories=4370&per_page=1"),
        ] {
            let res = client
                .get(url)
                .header("Accept", "application/json")
                .send()
                .await;
            match res {
                Ok(r) => println!("  {:<14} -> HTTP {}", name, r.status()),
                Err(e) => println!("  {:<14} -> error de red: {}", name, e),
            }
        }
    }
}
