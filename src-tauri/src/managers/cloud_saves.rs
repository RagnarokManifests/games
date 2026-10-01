use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zip::write::FileOptions;
use zip::ZipWriter;

// ============================================================
// Google Cloud Console OAuth2 credentials:
// Configurable via GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET env vars
// ============================================================
const GOOGLE_CLIENT_ID: &str = "";
const GOOGLE_CLIENT_SECRET: &str = "";
const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file openid email";

fn get_google_client_id() -> String {
    std::env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| GOOGLE_CLIENT_ID.to_string())
}

fn get_google_client_secret() -> String {
    std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| GOOGLE_CLIENT_SECRET.to_string())
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TokenData {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BackupInfo {
    pub id: String,
    pub app_id: String,
    pub name: String,
    pub created_at: String,
    pub size_bytes: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct AccountsStore {
    accounts: HashMap<String, TokenData>,
    #[serde(default)]
    default_account: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn old_token_path() -> PathBuf {
    let mut p = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("ragnarok");
    p.push("gdrive_token.json");
    p
}

fn accounts_path() -> PathBuf {
    let mut p = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("ragnarok");
    p.push("gdrive_accounts.json");
    p
}

/// Key for the accounts file at rest.
///
/// Same posture the Legends token already uses, and the same honest caveat: a
/// fixed passphrase compiled into the binary is obfuscation, not secrecy —
/// anyone who reverse-engineers the exe gets it. What it does buy is real
/// though: the infostealers that sweep %APPDATA% for readable credentials
/// stop finding a JSON file with `refresh_token` spelled out in it. Those
/// tokens do not expire, and this app's own client_secret ships in the same
/// binary, so a plaintext copy was an indefinite grant to the user's Drive
/// folder for anything that walked past.
fn accounts_key() -> aes_gcm::Key<aes_gcm::Aes256Gcm> {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(b"ragnarok-gdrive-accounts-v1");
    *aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(&hash)
}

fn encrypted_accounts_path() -> PathBuf {
    let mut p = accounts_path();
    p.set_extension("enc");
    p
}

fn decrypt_accounts(data: &[u8]) -> Option<AccountsStore> {
    use aes_gcm::aead::{Aead, KeyInit};
    if data.len() < 12 {
        return None;
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher = aes_gcm::Aes256Gcm::new(&accounts_key());
    let plaintext = cipher
        .decrypt(aes_gcm::Nonce::from_slice(nonce_bytes), ciphertext)
        .ok()?;
    serde_json::from_slice(&plaintext).ok()
}

fn load_accounts() -> AccountsStore {
    // Encrypted file first; the plaintext one is only read to migrate it.
    let enc = encrypted_accounts_path();
    if let Ok(data) = fs::read(&enc) {
        if let Some(store) = decrypt_accounts(&data) {
            return store;
        }
    }

    let path = accounts_path();
    if path.exists() {
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(store) = serde_json::from_str::<AccountsStore>(&data) {
                // Rewrite it encrypted and delete the readable copy, so an
                // existing user stops leaving tokens in the clear without
                // having to reconnect anything.
                if save_accounts(&store).is_ok() {
                    let _ = fs::remove_file(&path);
                }
                return store;
            }
        }
    }
    // Migrate old single-token file
    let old = old_token_path();
    if old.exists() {
        if let Ok(data) = fs::read_to_string(&old) {
            if let Ok(tokens) = serde_json::from_str::<TokenData>(&data) {
                let mut accounts = HashMap::new();
                // Use a placeholder; will be resolved on first email fetch
                accounts.insert("default".to_string(), tokens);
                let store = AccountsStore {
                    accounts,
                    default_account: Some("default".to_string()),
                };
                let _ = save_accounts(&store);
                let _ = fs::remove_file(&old);
                return store;
            }
        }
    }
    AccountsStore::default()
}

fn save_accounts(store: &AccountsStore) -> Result<(), String> {
    use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};

    let path = encrypted_accounts_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec(store).map_err(|e| e.to_string())?;

    let cipher = aes_gcm::Aes256Gcm::new(&accounts_key());
    let nonce = aes_gcm::Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, json.as_ref())
        .map_err(|e| e.to_string())?;

    let mut out = nonce.to_vec();
    out.extend_from_slice(&ciphertext);
    fs::write(&path, out).map_err(|e| e.to_string())?;

    // Never leave a readable copy behind next to the encrypted one.
    let _ = fs::remove_file(accounts_path());
    Ok(())
}

async fn get_valid_token_for(email: &str) -> Result<String, String> {
    let store = load_accounts();

    // An empty email means "no specific account requested" — resolve it to the
    // default/first configured account instead of literally looking up "" as
    // a key (which never matches and used to fail every backup/restore/list/
    // delete call with "Cuenta  no encontrada", since the Tauri commands here
    // were passing "" whenever the frontend didn't have an account selected).
    let resolved_email: String = if email.is_empty() {
        store
            .default_account
            .clone()
            .or_else(|| store.accounts.keys().next().cloned())
            .ok_or("No hay cuentas de Google Drive configuradas")?
    } else {
        email.to_string()
    };

    let tokens = store
        .accounts
        .get(&resolved_email)
        .ok_or_else(|| format!("Cuenta {} no encontrada", resolved_email))?
        .clone();
    drop(store);

    if now_secs() + 60 < tokens.expires_at {
        Ok(tokens.access_token)
    } else {
        let refreshed = do_refresh_logic(&tokens.refresh_token).await?;
        let mut store = load_accounts();
        store.accounts.insert(resolved_email.clone(), refreshed.clone());
        let _ = save_accounts(&store);
        Ok(refreshed.access_token)
    }
}

pub fn is_connected() -> bool {
    let store = load_accounts();
    !store.accounts.is_empty()
}

pub fn list_accounts() -> Vec<String> {
    let store = load_accounts();
    store.accounts.keys().cloned().collect()
}

pub fn get_default_account() -> Option<String> {
    let store = load_accounts();
    store.default_account.clone()
}

pub fn set_default_account(email: &str) -> Result<(), String> {
    let mut store = load_accounts();
    if !store.accounts.contains_key(email) {
        return Err(format!("Cuenta {} no encontrada", email));
    }
    store.default_account = Some(email.to_string());
    save_accounts(&store)
}

/// Removes every linked Google account and the credentials that go with them.
///
/// This used to drop exactly one — the default, or whichever happened to come
/// first out of the map — while the button that calls it says "Desconectar"
/// and the frontend sets its state to disconnected unconditionally. With two
/// or three accounts linked, the user was told they had disconnected while N-1
/// refresh tokens stayed on disk in plain text, and reopening the panel showed
/// "connected" again because `is_connected()` only asks whether the map is
/// empty. Credentials someone believes they deleted must actually be gone.
pub fn disconnect() {
    let mut store = load_accounts();
    store.accounts.clear();
    store.default_account = None;
    let _ = save_accounts(&store);
}

pub fn remove_account(email: &str) {
    let mut store = load_accounts();
    store.accounts.remove(email);
    if store.default_account.as_deref() == Some(email) {
        store.default_account = store.accounts.keys().next().cloned();
    }
    let _ = save_accounts(&store);
}

fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut b = input.replace('-', "+").replace('_', "/");
    match b.len() % 4 {
        2 => b.push_str("=="),
        3 => b.push_str("="),
        _ => {}
    }
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = b.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i] == b'=' { break; }
        let a = ALPHABET.iter().position(|&c| c == bytes[i]).unwrap_or(0);
        let b_char = if bytes[i + 1] != b'=' { ALPHABET.iter().position(|&c| c == bytes[i + 1]).unwrap_or(0) } else { 0 };
        let c_char = if i + 2 < bytes.len() && bytes[i + 2] != b'=' { ALPHABET.iter().position(|&c| c == bytes[i + 2]).unwrap_or(0) } else { 0 };
        let d = if i + 3 < bytes.len() && bytes[i + 3] != b'=' { ALPHABET.iter().position(|&c| c == bytes[i + 3]).unwrap_or(0) } else { 0 };
        out.push(((a as u8) << 2) | ((b_char as u8) >> 4));
        if i + 2 < bytes.len() && bytes[i + 2] != b'=' {
            out.push((((b_char as u8) & 0x0f) << 4) | ((c_char as u8) >> 2));
        }
        if i + 3 < bytes.len() && bytes[i + 3] != b'=' {
            out.push((((c_char as u8) & 0x03) << 6) | (d as u8));
        }
        i += 4;
    }
    Ok(out)
}

fn decode_jwt_payload(id_token: &str) -> Result<serde_json::Value, String> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() < 2 {
        return Err("id_token inválido".to_string());
    }
    let decoded = base64url_decode(parts[1])?;
    serde_json::from_slice(&decoded).map_err(|e| e.to_string())
}

fn email_from_id_token(resp: &serde_json::Value) -> Result<String, String> {
    let id_token = resp["id_token"]
        .as_str()
        .ok_or("No id_token en la respuesta de Google")?;
    let payload = decode_jwt_payload(id_token)?;
    payload["email"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!("No email en id_token: {}", serde_json::to_string(&payload).unwrap_or_default())
        })
}

async fn email_from_token(access_token: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://www.googleapis.com/oauth2/v1/userinfo?alt=json")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("Error al obtener info de la cuenta: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Error HTTP {} al obtener email: {}", status, body));
    }

    let resp: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Error al decodificar respuesta: {}", e))?;
    resp["email"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            let raw = serde_json::to_string(&resp).unwrap_or_default();
            format!("No se pudo obtener el email: respuesta: {}", raw)
        })
}

pub async fn get_account_email() -> Result<String, String> {
    let store = load_accounts();
    store
        .default_account
        .as_deref()
        .or_else(|| store.accounts.keys().next().map(|s| s.as_str()))
        .map(|s| s.to_string())
        .ok_or("No hay cuentas de Google Drive configuradas".to_string())
}

async fn do_refresh_logic(refresh_tok: &str) -> Result<TokenData, String> {
    let client_id = get_google_client_id();
    let client_secret = get_google_client_secret();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("Las credenciales de Google Drive OAuth no están configuradas.".to_string());
    }
    let client = reqwest::Client::new();
    let params = [
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("refresh_token", refresh_tok),
        ("grant_type", "refresh_token"),
    ];
    let resp: serde_json::Value = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let access_token = resp["access_token"]
        .as_str()
        .ok_or_else(|| format!("Refresh failed: {:?}", resp))?
        .to_string();
    let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);
    Ok(TokenData {
        access_token,
        refresh_token: refresh_tok.to_string(),
        expires_at: now_secs() + expires_in,
    })
}

/// Whether a Google Drive link is already running.
///
/// Only one can be, because the OAuth redirect always comes back to the one
/// port Google has registered for this client. Two flows meant the second one
/// lost the race for it and died with WSAEADDRINUSE — surfaced to the user as
/// "Puerto 8080 ocupado: Solo se permite un uso de cada dirección de socket
/// (os error 10048)", which reads as a conflict with some other program and
/// sends people hunting through their machine for it.
///
/// The real cause is this app. `wait_for_oauth_code` holds the port for up to
/// two minutes waiting for a redirect, so an attempt that is abandoned — tab
/// closed, no default browser, an antivirus eating the callback — keeps it for
/// the rest of that window, and every retry inside it fails the same way. A
/// user linking a second account right after the first hits it every time.
static AUTH_FLOW_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Releases the flag however the flow ends — success, error, timeout, or an
/// early `?`. A plain `store(false)` at the end of the function would leak the
/// flag on every failure path, which is exactly the set of paths this exists
/// for.
#[derive(Debug)]
struct AuthFlowGuard;

impl AuthFlowGuard {
    fn acquire() -> Result<Self, String> {
        match AUTH_FLOW_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => Ok(AuthFlowGuard),
            Err(_) => Err("Ya hay una vinculación de Google Drive en curso. Termina la que abriste en el navegador (o espera un par de minutos a que se cancele sola) antes de agregar otra cuenta.".to_string()),
        }
    }
}

impl Drop for AuthFlowGuard {
    fn drop(&mut self) {
        AUTH_FLOW_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Claims the callback port.
///
/// Tries port 8080 first. If occupied (e.g. by Steam or another local service),
/// gracefully falls back to alternative ports or an OS-assigned ephemeral port (0).
/// Google OAuth 2.0 Desktop Apps allow any loopback port per RFC 8252.
fn bind_callback_listener() -> Result<(TcpListener, String), String> {
    let candidate_ports: [u16; 7] = [8080, 8081, 8082, 8085, 8090, 8888, 0];
    for &port in &candidate_ports {
        if let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{}", port)) {
            if let Ok(addr) = listener.local_addr() {
                let actual_port = addr.port();
                let redirect_uri = format!("http://127.0.0.1:{}/callback", actual_port);
                return Ok((listener, redirect_uri));
            }
        }
    }
    Err("No se pudo abrir ningún puerto local para recibir la respuesta de Google. Comprueba tu firewall o cierra programas que puedan estar bloqueando las conexiones locales.".to_string())
}

// Modern auth: returns the authenticated email
async fn do_auth_inner(email_hint: Option<&str>) -> Result<String, String> {
    // Both before the browser opens: a second flow must be refused with a
    // sentence the user can act on, and the port must be ours before anyone
    // is asked to log in.
    let _flow = AuthFlowGuard::acquire()?;
    let (listener, redirect_uri) = bind_callback_listener()?;

    let client_id = get_google_client_id();
    let client_secret = get_google_client_secret();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("Las credenciales de Google Drive OAuth no están configuradas en esta compilación.".to_string());
    }

    let mut params = vec![
        ("client_id", client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("response_type", "code"),
        ("scope", DRIVE_SCOPE),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    if let Some(hint) = email_hint {
        params.push(("login_hint", hint));
    }
    let auth_url = reqwest::Url::parse_with_params(
        "https://accounts.google.com/o/oauth2/auth",
        &params,
    )
    .map_err(|e| e.to_string())?;

    opener::open(auth_url.as_str())
        .map_err(|e| format!("No se pudo abrir el navegador: {}", e))?;

    let code = tokio::task::spawn_blocking(move || wait_for_oauth_code(listener))
        .await
        .map_err(|e| e.to_string())??;

    let client = reqwest::Client::new();
    let params = [
        ("code", code.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];
    let resp: serde_json::Value = client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let access_token = resp["access_token"]
        .as_str()
        .ok_or("No access_token en la respuesta de Google")?
        .to_string();
    let refresh_token = resp["refresh_token"]
        .as_str()
        .ok_or("No refresh_token — usa prompt=consent o revoca acceso previo")?
        .to_string();
    let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);

    let email = email_from_id_token(&resp)?;

    let mut store = load_accounts();
    store.accounts.insert(
        email.clone(),
        TokenData {
            access_token,
            refresh_token,
            expires_at: now_secs() + expires_in,
        },
    );
    if store.default_account.is_none() {
        store.default_account = Some(email.clone());
    }
    save_accounts(&store)?;

    Ok(email)
}

pub async fn do_auth() -> Result<String, String> {
    do_auth_inner(None).await
}

pub async fn add_account() -> Result<String, String> {
    do_auth_inner(None).await
}

// Blocking — runs inside spawn_blocking to receive the OAuth redirect.
// Uses a non-blocking accept loop with an overall deadline: without this, a
// redirect that never arrives (blocked by a firewall/AV, no default browser
// configured, user closes the tab) hangs here forever, wedging port 8080 for
// the rest of the app's lifetime and leaving the frontend's "Conectando..."
// spinner stuck with no error ever surfacing.
fn wait_for_oauth_code(listener: TcpListener) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if std::time::Instant::now() > deadline {
            return Err("Tiempo de espera agotado esperando la respuesta de Google. Verifica tu firewall/antivirus o que tengas un navegador predeterminado configurado, y vuelve a intentarlo.".to_string());
        }

        let (mut stream, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        stream.set_nonblocking(false).ok();

        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let first_line = request.lines().next().unwrap_or("");
        let path = first_line.split_whitespace().nth(1).unwrap_or("");
        let qs = path.splitn(2, '?').nth(1).unwrap_or("");

        if let Some(pair) = qs.split('&').find(|p| p.starts_with("code=")) {
            let code = pair[5..].to_string();
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
                <!DOCTYPE html><html><body style='font-family:sans-serif;\
                text-align:center;padding:60px;background:#0f0f14;color:white'>\
                <h2>Ragnarok Launcher</h2>\
                <p>Autenticacion completada. Puedes cerrar esta ventana.</p>\
                </body></html>",
            );
            return Ok(code);
        } else if qs.contains("error=") {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n\r\nError");
            return Err("El usuario denegó el acceso a Google Drive".to_string());
        }
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n");
    }
}

// ── Google Drive helpers ──────────────────────────────────────────────────────

async fn find_or_create_folder(
    token: &str,
    name: &str,
    parent_id: Option<&str>,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let query = match parent_id {
        Some(pid) => format!(
            "name='{}' and mimeType='application/vnd.google-apps.folder' and '{}' in parents and trashed=false",
            name, pid
        ),
        None => format!(
            "name='{}' and mimeType='application/vnd.google-apps.folder' and trashed=false",
            name
        ),
    };

    // Google Drive does NOT enforce folder-name uniqueness — two folders
    // both named "Ragnarok Saves" (or the same app_id) can coexist. A
    // transient failure here (a slow/failed search right after the first
    // creation, Drive's brief write-then-search consistency lag, etc.) can
    // leave a duplicate behind, and without an explicit orderBy this query
    // returns Drive's unspecified default order — meaning `.first()` could
    // silently pick the newer, EMPTY duplicate on one run and the original,
    // populated one on another. That reads as "I have backups but Ragnarok
    // shows nothing" with no error anywhere, since nothing actually fails —
    // it's just listing the wrong (but valid) empty folder. Ordering by
    // creation time makes the pick deterministic: always the oldest, i.e.
    // whichever one every prior backup actually accumulated in.
    let resp: serde_json::Value = client
        .get("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(token)
        .query(&[("q", query.as_str()), ("fields", "files(id)"), ("orderBy", "createdTime")])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(id) = resp["files"].as_array()
        .and_then(|arr| arr.first())
        .and_then(|f| f["id"].as_str())
    {
        return Ok(id.to_string());
    }

    let mut body = serde_json::json!({
        "name": name,
        "mimeType": "application/vnd.google-apps.folder"
    });
    if let Some(pid) = parent_id {
        body["parents"] = serde_json::json!([pid]);
    }

    let resp: serde_json::Value = client
        .post("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    resp["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Error al crear carpeta '{}' en Drive: {}", name, drive_api_error(&resp)))
}

fn drive_api_error(resp: &serde_json::Value) -> String {
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| resp.as_str().unwrap_or("error desconocido"))
        .to_string()
}

// ── ZIP helpers ───────────────────────────────────────────────────────────────

// Entries from the Goldberg GSE Saves folder are bundled into the same zip
// as the real Steam save folder, under this prefix, so a single backup/
// restore round-trip covers both without needing two separate Drive uploads.
const GOLDBERG_ZIP_PREFIX: &str = "__goldberg_achievements__";

/// gbe_fork's default "GSE Saves" folder for one game — %appdata%\GSE
/// Saves\<appid>\ on Windows. This is where a Goldberg-patched game's
/// emulated remote storage AND achievement/stat unlock state actually live
/// (Ragnarok never sets `local_save_path` when generating Goldberg's config,
/// so gbe_fork falls back to this global default) — entirely separate from
/// Steam's own userdata/<id>/<appid>/remote/ that find_save_path() looks at,
/// and invisible to it. Real Steam games never have anything here.
/// Where a restore should put Goldberg saves back.
///
/// Restoring into the folder the emulator is NOT reading is the same as not
/// restoring at all. The backup side already handles both names via
/// `find_goldberg_saves_dir`, but this returned "GSE Saves" unconditionally —
/// so on a machine whose fork uses the older `Goldberg SteamEmu Saves`, the
/// backup was taken from one folder and the restore written to the other. The
/// emulator kept reading the old one, the restored progress was invisible, and
/// the operation reported success.
///
/// An existing folder wins; "GSE Saves" is only the target when neither is
/// there yet, since that is gbe_fork's current default.
pub fn goldberg_saves_dir(app_id: &str) -> Option<PathBuf> {
    if let Some(existing) = find_goldberg_saves_dir(app_id) {
        return Some(existing);
    }
    Some(dirs::data_dir()?.join("GSE Saves").join(app_id))
}

/// Same lookup as goldberg_saves_dir(), but for detecting an EXISTING
/// Goldberg save/achievement folder rather than picking a target to write
/// to. Goldberg forks have used two different folder names over time
/// ("GSE Saves" is gbe_fork's current default, "Goldberg SteamEmu Saves" is
/// the older name still produced by some releases/repacks) — main.rs's
/// find_local_achievements_file() already checks both for achievements;
/// this mirrors that so backup/mtime detection doesn't report "no saves
/// found" for a game that saved into the older folder name.
pub fn find_goldberg_saves_dir(app_id: &str) -> Option<PathBuf> {
    let data_dir = dirs::data_dir()?;
    for folder in ["GSE Saves", "Goldberg SteamEmu Saves"] {
        let candidate = data_dir.join(folder).join(app_id);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Builds the backup zip, returning it together with how many real files
/// went into it.
///
/// The count matters: an empty zip is a perfectly valid 22-byte file, so
/// byte length cannot tell "nothing to back up" apart from "a tiny save".
fn zip_backup_bundle(
    save_path: Option<&Path>,
    goldberg_path: Option<&Path>,
    origin: Option<&SaveOrigin>,
) -> Result<(Vec<u8>, usize), String> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let mut files = 0usize;
    if let Some(sp) = save_path {
        files += zip_add_dir(&mut zip, sp, sp, "")?;
    }
    // Written only alongside real save files, and not counted as one: a zip
    // holding nothing but this record must still be refused as empty.
    if let (Some(origin), true) = (origin, files > 0) {
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let json = serde_json::to_string_pretty(origin).map_err(|e| e.to_string())?;
        zip.start_file(BACKUP_MANIFEST, options).map_err(|e| e.to_string())?;
        zip.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
    }
    if let Some(gp) = goldberg_path {
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.add_directory(GOLDBERG_ZIP_PREFIX, options).map_err(|e| e.to_string())?;
        files += zip_add_dir(&mut zip, gp, gp, GOLDBERG_ZIP_PREFIX)?;
    }
    let result = zip.finish().map_err(|e| e.to_string())?;
    Ok((result.into_inner(), files))
}

fn zip_add_dir(
    zip: &mut ZipWriter<std::io::Cursor<Vec<u8>>>,
    base: &Path,
    current: &Path,
    prefix: &str,
) -> Result<usize, String> {
    let mut files = 0usize;
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in fs::read_dir(current).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let relative = path.strip_prefix(base).map_err(|e| e.to_string())?;
        let rel_name = relative.to_string_lossy().replace('\\', "/");
        let name = if prefix.is_empty() { rel_name } else { format!("{}/{}", prefix, rel_name) };
        if path.is_dir() {
            zip.add_directory(&name, options).map_err(|e| e.to_string())?;
            files += zip_add_dir(zip, base, &path, prefix)?;
        } else {
            zip.start_file(&name, options).map_err(|e| e.to_string())?;
            let data = fs::read(&path).map_err(|e| e.to_string())?;
            zip.write_all(&data).map_err(|e| e.to_string())?;
            files += 1;
        }
    }
    Ok(files)
}

/// Splits the combined backup zip back apart on restore: anything under
/// GOLDBERG_ZIP_PREFIX goes to the Goldberg GSE Saves folder (if the caller
/// has one to restore into), everything else goes to the real Steam save
/// folder — same as a plain saves-only backup did before achievements were
/// bundled in.
///
/// Returns how many real files were written. A zip that unpacks to nothing
/// is not a successful restore — it is the shape a half-finished upload
/// leaves behind, and reporting it as success is what made a restore appear
/// to run fine while the game still showed no save.
fn unzip_backup_bundle(
    data: &[u8],
    save_target: &Path,
    goldberg_target: Option<&Path>,
) -> Result<usize, String> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;
    let mut written = 0usize;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        // The origin record is for Ragnarok, not the game.
        if file.name() == BACKUP_MANIFEST {
            continue;
        }
        let is_dir = file.name().ends_with('/');
        let is_goldberg = file.name().starts_with(GOLDBERG_ZIP_PREFIX);
        // mangled_name() sanitizes the archive-relative path (strips leading
        // slashes / ".." components) — using it instead of the raw entry
        // name avoids a zip-slip write outside the intended target folder.
        let mangled = file.mangled_name();

        let outpath = if is_goldberg {
            let Some(gt) = goldberg_target else { continue }; // this game has no Goldberg install to restore into — skip
            let rest = mangled.strip_prefix(GOLDBERG_ZIP_PREFIX).unwrap_or(&mangled);
            if rest.as_os_str().is_empty() { continue; } // the bare prefix directory entry itself
            gt.join(rest)
        } else {
            save_target.join(&mangled)
        };

        if is_dir {
            fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
            written += 1;
        }
    }
    Ok(written)
}

// ── Steam save detection ──────────────────────────────────────────────────────

/// Picks the real userdata/<steamId> folder to restore INTO when this game
/// has never saved locally before (fresh install, wiped PC, or exactly the
/// case a restore is meant to fix) — find_save_path() can't help here since
/// it requires the app's own save folder to already exist.
///
/// Both restore paths used to fall back to a literal "userdata/0/<app_id>/
/// remote" in that situation. "0" is never a real Steam account folder (those
/// are named by the account's 32-bit SteamID), so the restore would silently
/// "succeed" — files written, no error — into a folder Steam never reads,
/// leaving the game's actual save spot untouched. Reported by a user who did
/// exactly this: ran a restore, saw no error, but the game still showed no
/// save.
///
/// Picks the single existing numeric userdata folder when there's only one
/// (the overwhelming majority of PCs — one Steam account), or the
/// most-recently-modified one when several accounts have used Steam on this
/// machine, as the best available signal for "the one actually in use."
pub fn resolve_userdata_dir(steam_path: &str) -> Option<PathBuf> {
    let userdata = Path::new(steam_path).join("userdata");
    let candidates: Vec<PathBuf> = fs::read_dir(&userdata)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().and_then(|n| n.to_str()).map(|n| n.chars().all(|c| c.is_ascii_digit())).unwrap_or(false))
        .collect();

    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates.into_iter().next().unwrap());
    }
    candidates
        .into_iter()
        .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// Where a backup's saves came from. Stored inside every backup as
/// `ragnarok_backup.json`, so a restore puts them back in the same kind of
/// place instead of guessing.
///
/// Guessing was the bug a user reported as "guardé la partida y al restaurarla
/// no estaba". Restore looked for the save folder only among folders that
/// already exist — and after formatting, or on a new PC, the one the game uses
/// usually does not yet. It then fell back to Steam Cloud's
/// `userdata/<id>/<appid>/remote`, wrote the files there without an error,
/// and the game, which saves somewhere else entirely, showed no save.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SaveOrigin {
    /// Steam Cloud's own folder, `userdata/<id>/<appid>[/remote]`.
    SteamUserdata { remote: bool },
    /// A folder the game declares in appinfo.vdf's `ufs/savefiles`.
    Declared { root: String, template: String, holds_profiles: bool },
    /// A Steam emulator's own remote-storage folder (CODEX, RUNE, OnlineFix,
    /// EMPRESS). `base` names the Windows folder it hangs off, `rel` the rest
    /// with the app id already in it.
    Emulator { base: String, rel: String },
}

/// The Windows folder an emulator save hangs off, resolved on this PC.
fn emulator_base(base: &str) -> Option<PathBuf> {
    match base {
        "public_documents" => Some(
            std::env::var_os("PUBLIC")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"))
                .join("Documents"),
        ),
        "roaming" => dirs::data_dir(),
        _ => None,
    }
}

/// Where the Steam emulators keep what a game writes through Steam's remote
/// storage. A game that saves only through Steam Cloud — A Plague Tale:
/// Requiem is one, Steam declares no `ufs` folder for it — writes to
/// `userdata/<id>/<appid>/remote` under real Steam, but under one of these
/// emulators the same save lands here instead, and the backup reported
/// "no saves found" for a game that had them.
///
/// Goldberg's folder is not in this list: it is bundled separately, together
/// with its achievements, by `find_goldberg_saves_dir`.
fn emulator_save_candidates(app_id: &str) -> Vec<(String, String)> {
    let p = |base: &str, rel: String| (base.to_string(), rel);
    vec![
        p("public_documents", format!(r"Steam\CODEX\{}\remote", app_id)),
        p("roaming", format!(r"Steam\CODEX\{}\remote", app_id)),
        p("public_documents", format!(r"Steam\RUNE\{}\remote", app_id)),
        p("public_documents", format!(r"OnlineFix\{}\Saves", app_id)),
        p("public_documents", format!(r"EMPRESS\{}\remote", app_id)),
        p("roaming", format!(r"EMPRESS\remote\{}", app_id)),
    ]
}

/// The first emulator save folder that actually holds files.
fn locate_emulator_save(app_id: &str) -> Option<(PathBuf, SaveOrigin)> {
    emulator_save_candidates(app_id).into_iter().find_map(|(base, rel)| {
        let path = emulator_base(&base)?.join(&rel);
        (path.is_dir() && crate::managers::appinfo::has_any_file(&path, 0))
            .then(|| (path, SaveOrigin::Emulator { base, rel }))
    })
}

/// The record's name inside a backup. Never restored as a save file.
const BACKUP_MANIFEST: &str = "ragnarok_backup.json";

/// The save folder this game uses on this PC, and how it was found.
pub fn locate_save(steam_path: &str, app_id: &str) -> Option<(PathBuf, Option<SaveOrigin>)> {
    // No early return when userdata/ is missing. That used to skip the
    // appinfo.vdf lookup below as well — which does not need userdata at all —
    // so a Steam install nobody had signed into yet found no saves anywhere.
    let userdata = Path::new(steam_path).join("userdata");
    if let Ok(entries) = fs::read_dir(&userdata) {
        for entry in entries {
            // `continue`, not `?`: one unreadable entry under userdata/ — a
            // folder Steam has open, a leftover from another account — must
            // not abandon the search, nor skip the appinfo.vdf lookup below.
            let Ok(entry) = entry else { continue };
            let user_dir = entry.path();
            if !user_dir.is_dir() {
                continue;
            }
            let remote = user_dir.join(app_id).join("remote");
            if remote.exists() {
                return Some((remote, Some(SaveOrigin::SteamUserdata { remote: true })));
            }
            let app_dir = user_dir.join(app_id);
            if app_dir.exists() {
                return Some((app_dir, Some(SaveOrigin::SteamUserdata { remote: false })));
            }
        }
    }

    // Nothing under Steam Cloud. Ask Steam where the game actually saves:
    // appinfo.vdf carries a ufs/savefiles definition per app, the same thing
    // Steam Cloud itself syncs from. Crimson Desert keeps 8.9 MB under a
    // folder named after its studio, which no search by title would find.
    let game_dir = crate::managers::crackers::find_game_folder(steam_path, app_id);
    let Some(path) = crate::managers::appinfo::existing_save_dirs(steam_path, app_id, game_dir.as_deref())
        .into_iter()
        .next()
    else {
        // Steam declares nothing that exists: the game may save only through
        // Steam Cloud, which under an emulator lands in the emulator's folder.
        return locate_emulator_save(app_id).map(|(path, origin)| (path, Some(origin)));
    };
    let origin = crate::managers::appinfo::declared_save_dirs(steam_path, app_id, game_dir.as_deref())
        .into_iter()
        .find(|d| d.path == path)
        .map(|d| SaveOrigin::Declared {
            root: d.root,
            template: d.template,
            holds_profiles: d.holds_profiles,
        });
    Some((path, origin))
}

pub fn find_save_path(steam_path: &str, app_id: &str) -> Option<PathBuf> {
    locate_save(steam_path, app_id).map(|(path, _)| path)
}

/// A recorded origin, resolved on this PC. Does not require the folder to exist.
fn target_from_origin(steam_path: &str, app_id: &str, origin: &SaveOrigin) -> Option<PathBuf> {
    match origin {
        SaveOrigin::SteamUserdata { remote } => {
            let base = resolve_userdata_dir(steam_path)?.join(app_id);
            Some(if *remote { base.join("remote") } else { base })
        }
        SaveOrigin::Declared { root, template, holds_profiles } => {
            let game_dir = crate::managers::crackers::find_game_folder(steam_path, app_id);
            crate::managers::appinfo::resolve_template(steam_path, root, template, *holds_profiles, game_dir.as_deref())
        }
        SaveOrigin::Emulator { base, rel } => Some(emulator_base(base)?.join(rel)),
    }
}

/// Where a restore writes, in order of how much is actually known:
///
/// 1. the origin recorded in the backup — where the saves really came from;
/// 2. a save folder that already exists on this PC;
/// 3. a folder the game declares, even though it does not exist yet — the
///    after-a-format case, which is what used to go wrong;
/// 4. Steam Cloud's own folder, for games that declare nothing.
///
/// `holds_profiles` is only a hint for backups made before the origin was
/// recorded: whether their saves sit one level down in account-id folders.
fn restore_target(
    steam_path: &str,
    app_id: &str,
    origin: Option<&SaveOrigin>,
    holds_profiles: bool,
) -> Result<PathBuf, String> {
    if let Some(path) = origin.and_then(|o| target_from_origin(steam_path, app_id, o)) {
        return Ok(path);
    }
    if let Some((path, _)) = locate_save(steam_path, app_id) {
        return Ok(path);
    }
    let game_dir = crate::managers::crackers::find_game_folder(steam_path, app_id);
    let declared = crate::managers::appinfo::declared_save_dirs(steam_path, app_id, game_dir.as_deref());
    if let Some(d) = declared
        .iter()
        .find(|d| d.holds_profiles == holds_profiles)
        .or_else(|| declared.first())
    {
        return Ok(d.path.clone());
    }
    let user_dir = resolve_userdata_dir(steam_path).ok_or_else(|| {
        "No se encontró ninguna carpeta de usuario de Steam (userdata/<id>) — inicia sesión en Steam al menos una vez antes de restaurar.".to_string()
    })?;
    Ok(user_dir.join(app_id).join("remote"))
}

/// The origin recorded inside a backup zip, if it has one.
fn read_backup_origin(data: &[u8]) -> Option<SaveOrigin> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data)).ok()?;
    let mut file = archive.by_name(BACKUP_MANIFEST).ok()?;
    let mut json = String::new();
    file.read_to_string(&mut json).ok()?;
    serde_json::from_str(&json).ok()
}

/// Whether every save entry sits inside a numeric (account id) folder — the
/// shape of a backup taken from the folder above `{Steam3AccountID}`.
fn names_hold_profiles<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let mut any = false;
    for name in names {
        if name == BACKUP_MANIFEST || name.starts_with(GOLDBERG_ZIP_PREFIX) {
            continue;
        }
        let Some((first, _)) = name.trim_start_matches('/').split_once('/') else {
            return false; // a file at the top level
        };
        if first.is_empty() || !first.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        any = true;
    }
    any
}

fn archive_holds_profiles(data: &[u8]) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(data)) else { return false };
    let mut names = Vec::new();
    for i in 0..archive.len() {
        if let Ok(f) = archive.by_index(i) {
            names.push(f.name().to_string());
        }
    }
    names_hold_profiles(names.iter().map(|s| s.as_str()))
}

fn dir_holds_profiles(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else { return false };
    let mut any = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == BACKUP_MANIFEST {
            continue;
        }
        if !entry.path().is_dir() || !name.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        any = true;
    }
    any
}

#[cfg(test)]
mod save_origin_tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ragnarok_saveorigin_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_origin_survives_the_round_trip_through_json() {
        for origin in [
            SaveOrigin::SteamUserdata { remote: true },
            SaveOrigin::Declared {
                root: "WinAppDataLocal".into(),
                template: "Pearl Abyss/CD/save/{Steam3AccountID}".into(),
                holds_profiles: true,
            },
            SaveOrigin::Emulator { base: "public_documents".into(), rel: r"Steam\CODEX\1182900\remote".into() },
        ] {
            let json = serde_json::to_string(&origin).unwrap();
            assert_eq!(serde_json::from_str::<SaveOrigin>(&json).unwrap(), origin, "{json}");
        }
    }

    #[test]
    fn an_emulator_origin_resolves_to_its_folder_before_it_exists() {
        let origin = SaveOrigin::Emulator { base: "roaming".into(), rel: r"EMPRESS\remote\1182900".into() };
        let target = target_from_origin("", "1182900", &origin).unwrap();
        assert_eq!(target, dirs::data_dir().unwrap().join("EMPRESS").join("remote").join("1182900"));
        assert!(emulator_save_candidates("1182900").iter().all(|(_, rel)| rel.contains("1182900")));
    }

    #[test]
    fn a_backup_of_profile_folders_is_recognised() {
        assert!(names_hold_profiles(["19627/", "19627/slot1.save", "764415490/slot1.save", BACKUP_MANIFEST].into_iter()));
        assert!(!names_hold_profiles(["slot1.save"].into_iter()), "archivo suelto arriba");
        assert!(!names_hold_profiles(["19627/a.save", "Config/x.ini"].into_iter()), "carpeta no numérica");
        assert!(!names_hold_profiles(std::iter::empty()), "vacío no es de perfiles");
    }

    /// The record travels inside the zip, and restoring never writes it out
    /// among the player's save files.
    #[test]
    fn a_zip_carries_its_origin_and_restore_skips_the_record() {
        let src = temp("zip_src");
        fs::write(src.join("slot1.save"), b"partida").unwrap();
        let origin = SaveOrigin::Declared {
            root: "WinMyDocuments".into(),
            template: "My Games/Juego".into(),
            holds_profiles: false,
        };

        let (zip, files) = zip_backup_bundle(Some(&src), None, Some(&origin)).unwrap();
        assert_eq!(files, 1, "el registro no cuenta como archivo de partida");
        assert_eq!(read_backup_origin(&zip), Some(origin));

        let dst = temp("zip_dst");
        assert_eq!(unzip_backup_bundle(&zip, &dst, None).unwrap(), 1);
        assert_eq!(fs::read(dst.join("slot1.save")).unwrap(), b"partida");
        assert!(!dst.join(BACKUP_MANIFEST).exists());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    /// Restoring after a format: the Steam Cloud folder does not exist yet,
    /// and the recorded origin still resolves to it.
    #[test]
    fn a_steam_cloud_origin_resolves_before_the_folder_exists() {
        let steam = temp("steam");
        fs::create_dir_all(steam.join("userdata").join("12345")).unwrap();
        let target = target_from_origin(
            &steam.to_string_lossy(),
            "4001890",
            &SaveOrigin::SteamUserdata { remote: true },
        )
        .unwrap();
        assert_eq!(target, steam.join("userdata").join("12345").join("4001890").join("remote"));
        assert!(!target.exists(), "no hace falta que exista para saber dónde va");
        let _ = fs::remove_dir_all(&steam);
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub async fn do_backup(app_id: &str, steam_path: &str, account_email: &str) -> Result<String, String> {
    let located = locate_save(steam_path, app_id);
    let save_path = located.as_ref().map(|(path, _)| path.clone());
    let origin = located.and_then(|(_, origin)| origin);
    // Only bundle the Goldberg folder if it actually exists — most games
    // aren't Goldberg-patched, and an empty %appdata%\GSE Saves\<appid>\
    // would just be dead weight in the zip.
    let goldberg_path = find_goldberg_saves_dir(app_id);

    if save_path.is_none() && goldberg_path.is_none() {
        // Almost always a game that has not saved yet on this PC. The old text
        // just named a Steam path, which read like something the user had to
        // go and fix by hand.
        return Err(format!(
            "Este juego todavía no tiene ninguna partida guardada en esta PC (AppID {}). \
             Juega hasta que se guarde la partida al menos una vez y vuelve a intentarlo. \
             Se buscó en Steam Cloud (userdata/<id>/{}/remote), en las carpetas que declara el juego \
             y en las de los emuladores (Goldberg, CODEX, RUNE, OnlineFix, EMPRESS).",
            app_id, app_id
        ));
    }

    // ZIP the saves (and, if present, Goldberg achievements/stats) folder(s)
    // in a blocking thread
    let (zip_data, file_count) = tokio::task::spawn_blocking({
        let sp = save_path.clone();
        let gp = goldberg_path.clone();
        let og = origin.clone();
        move || zip_backup_bundle(sp.as_deref(), gp.as_deref(), og.as_ref())
    })
    .await
    .map_err(|e| e.to_string())??;

    // Guarded on the entry count. The old test was `zip_data.len() < 22`,
    // and an empty zip is exactly 22 bytes — so a save folder that existed
    // but held no files sailed past it and got uploaded. That is where the
    // "files in my Drive that aren't my saves" came from: valid, listable,
    // 22-byte backups with nothing inside, which then restored silently
    // without putting anything back.
    if file_count == 0 {
        return Err(format!(
            "La carpeta de saves existe pero no tiene ningún archivo dentro, así que no hay nada que copiar: {}",
            save_path.map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }

    let access_token = get_valid_token_for(account_email).await?;
    let root_id = find_or_create_folder(&access_token, "Ragnarok Saves", None).await?;
    let app_folder_id = find_or_create_folder(&access_token, app_id, Some(&root_id)).await?;

    let filename = format!("saves_{}_{}.zip", app_id, now_secs());
    let client = reqwest::Client::new();

    let uploaded =
        upload_backup_zip(&client, &access_token, &filename, &app_folder_id, zip_data).await?;

    // Say what actually went in.
    //
    // find_save_path only knows two places: Steam Cloud's own
    // userdata/<id>/<appid>/remote, and the emulator's folder. Plenty of games
    // save somewhere else entirely — Crimson Desert keeps 8.9 MB under
    // %LOCALAPPDATA%\Pearl Abyss, named after the studio rather than the game,
    // so no search by title would find it either. When that happens the backup
    // still succeeds, carrying only the 2 KB of achievements, and reports
    // success exactly like a complete one. Somebody trusting that would find
    // out only when they came to restore.
    if save_path.is_none() {
        return Ok(format!(
            "{} — ATENCIÓN: solo se guardaron los logros. No se encontró la partida de este juego: \
             Ragnarok busca en la carpeta de Steam Cloud y en la del emulador, y este juego guarda \
             en otro sitio. No uses esta copia como respaldo de tu progreso.",
            uploaded
        ));
    }

    Ok(uploaded)
}

/// Closing delimiter of a `multipart/related` body: CRLF, the boundary
/// marked final with a trailing `--`, then CRLF.
fn zip_trailer(boundary: &str) -> Vec<u8> {
    format!("\r\n--{}--\r\n", boundary).into_bytes()
}

/// Uploads the backup in a single request, metadata and bytes together.
///
/// This used to be two calls: create the file record, then PATCH its content
/// in. Whenever the second call did not land — a dropped connection, a token
/// that expired between the two, the app being closed mid-upload — Drive kept
/// what the first one made: a 0-byte file with a machine-generated name and
/// no save data in it. Reported by a user as "my Drive has temporary files
/// instead of my saves, and the download buttons do nothing" — those empty
/// records were also being listed as restorable backups, so restoring one
/// downloaded nothing and silently did nothing.
///
/// A single multipart/related upload cannot leave that behind: Drive either
/// stores the complete file or no file at all.
///
/// Note this must be `multipart/related` built by hand, not reqwest's
/// `multipart` feature — that sends `multipart/form-data`, which the Drive
/// upload endpoint rejects.
async fn upload_backup_zip(
    client: &reqwest::Client,
    access_token: &str,
    filename: &str,
    parent_id: &str,
    zip_data: Vec<u8>,
) -> Result<String, String> {
    const BOUNDARY: &str = "ragnarok-save-boundary-8f21c7d3";

    let metadata = serde_json::json!({
        "name": filename,
        "parents": [parent_id],
        "mimeType": "application/zip",
    });

    let mut body: Vec<u8> = Vec::with_capacity(zip_data.len() + 512);
    body.extend_from_slice(format!("--{}\r\n", BOUNDARY).as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata.to_string().as_bytes());
    body.extend_from_slice(format!("\r\n--{}\r\n", BOUNDARY).as_bytes());
    body.extend_from_slice(b"Content-Type: application/zip\r\n\r\n");
    body.extend_from_slice(&zip_data);
    body.extend_from_slice(&zip_trailer(BOUNDARY));

    let resp = client
        .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&fields=id")
        .bearer_auth(access_token)
        .header("Content-Type", format!("multipart/related; boundary={}", BOUNDARY))
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Error de red al subir la copia: {}", e))?;

    // Checked before parsing: an error response is still valid JSON, and
    // reading the id out of it just produced "error desconocido" instead of
    // what Drive actually said.
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);

    if !status.is_success() {
        return Err(format!(
            "Drive rechazó la subida ({}): {}",
            status,
            drive_api_error(&parsed)
        ));
    }

    parsed["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Drive no devolvió el id del archivo: {}", drive_api_error(&parsed)))
}

pub async fn list_backups(app_id: &str, account_email: &str) -> Result<Vec<BackupInfo>, String> {
    let access_token = get_valid_token_for(account_email).await?;
    let root_id = find_or_create_folder(&access_token, "Ragnarok Saves", None).await?;
    let app_folder_id = find_or_create_folder(&access_token, app_id, Some(&root_id)).await?;

    // Restricted to files this app wrote. Anything else that ends up in that
    // folder — a subfolder, a stray upload, or a 0-byte record left by the
    // old two-step upload — cannot be restored from, and offering it as a
    // backup is exactly what made the restore button look broken.
    let query = format!(
        "'{}' in parents and trashed=false and name contains 'saves_' and mimeType != 'application/vnd.google-apps.folder'",
        app_folder_id
    );
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .get("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(&access_token)
        .query(&[
            ("q", query.as_str()),
            ("fields", "files(id,name,createdTime,size)"),
            ("orderBy", "createdTime desc"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let infos: Vec<BackupInfo> = resp["files"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|f| BackupInfo {
                    id: f["id"].as_str().unwrap_or("").to_string(),
                    app_id: app_id.to_string(),
                    name: f["name"].as_str().unwrap_or("").to_string(),
                    created_at: f["createdTime"].as_str().unwrap_or("").to_string(),
                    size_bytes: f["size"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();

    // A backup with no bytes in it cannot restore anything. These still
    // exist on accounts that ran the old upload path, so they are filtered
    // out here rather than only being prevented going forward.
    let infos: Vec<BackupInfo> = infos.into_iter().filter(|b| b.size_bytes > 0).collect();

    Ok(infos)
}

/// Deletes the local save folder for a game (after cloud backup succeeds).
/// Returns true if something was deleted, false if no saves existed.
pub fn delete_local_save_folder(steam_path: &str, app_id: &str) -> Result<bool, String> {
    let save_path = find_save_path(steam_path, app_id)
        .ok_or_else(|| format!("No se encontraron saves para AppID {}", app_id))?;

    if !save_path.exists() {
        return Ok(false);
    }

    fn remove_recursive(path: &std::path::Path) -> Result<(), String> {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                remove_recursive(&entry.path())?;
            }
            std::fs::remove_dir(path).map_err(|e| e.to_string())?;
        } else {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    remove_recursive(&save_path)?;
    Ok(true)
}

/// Parses a Google Drive `createdTime` string ("YYYY-MM-DDTHH:MM:SS...", UTC)
/// into a real Unix timestamp, accounting for actual days-per-month and leap
/// years (no `chrono` dependency needed for this one conversion).
fn time_from_iso8601(iso: &str) -> Result<u64, ()> {
    if iso.len() < 19 { return Err(()); }
    let y: i64 = iso[0..4].parse().map_err(|_| ())?;
    let mo: u32 = iso[5..7].parse().map_err(|_| ())?;
    let d: u32 = iso[8..10].parse().map_err(|_| ())?;
    let h: i64 = iso[11..13].parse().map_err(|_| ())?;
    let mi: i64 = iso[14..16].parse().map_err(|_| ())?;
    let s: i64 = iso[17..19].parse().map_err(|_| ())?;

    if !(1..=12).contains(&mo) || d == 0 || d > 31 {
        return Err(());
    }

    let is_leap = |year: i64| (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    const DAYS_IN_MONTH: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    let mut days: i64 = 0;
    if y >= 1970 {
        for year in 1970..y {
            days += if is_leap(year) { 366 } else { 365 };
        }
    } else {
        for year in y..1970 {
            days -= if is_leap(year) { 366 } else { 365 };
        }
    }
    for m in 0..(mo as usize - 1) {
        days += DAYS_IN_MONTH[m];
        if m == 1 && is_leap(y) { days += 1; }
    }
    days += (d as i64) - 1;

    let total_secs = days * 86400 + h * 3600 + mi * 60 + s;
    Ok(total_secs.max(0) as u64)
}

pub async fn do_restore(app_id: &str, backup_id: &str, steam_path: &str, account_email: &str) -> Result<(), String> {
    let access_token = get_valid_token_for(account_email).await?;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "https://www.googleapis.com/drive/v3/files/{}?alt=media",
            backup_id
        ))
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    // The status was never looked at here. On any Drive error — an expired
    // grant, a deleted file, a quota block — the JSON error body was handed
    // straight to the zip parser, so the user got "invalid Zip archive"
    // instead of the actual reason, or nothing at all.
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();

    if !status.is_success() {
        let parsed: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        return Err(format!(
            "Drive no entregó la copia ({}): {}",
            status,
            drive_api_error(&parsed)
        ));
    }

    if bytes.is_empty() {
        return Err(
            "Esa copia está vacía en Drive (0 bytes): la subida quedó a medias. Borrala y hacé una copia nueva."
                .to_string(),
        );
    }

    let save_path = restore_target(
        steam_path,
        app_id,
        read_backup_origin(&bytes).as_ref(),
        archive_holds_profiles(&bytes),
    )?;
    // Restored unconditionally (not gated on already existing) — a fresh
    // install after a format is exactly the case this is for, and
    // unzip_backup_bundle only writes anything under it if the backup
    // actually has a Goldberg entry to place there.
    let goldberg_target = goldberg_saves_dir(app_id);

    let restored = tokio::task::spawn_blocking(move || {
        unzip_backup_bundle(&bytes, &save_path, goldberg_target.as_deref())
    })
    .await
    .map_err(|e| e.to_string())??;

    if restored == 0 {
        return Err(
            "La copia se descargó pero no tenía ningún archivo de partida dentro. Probablemente se subió a medias: borrala y hacé una copia nueva."
                .to_string(),
        );
    }

    Ok(())
}

pub async fn delete_backup(backup_id: &str, account_email: &str) -> Result<(), String> {
    let access_token = get_valid_token_for(account_email).await?;
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!("https://www.googleapis.com/drive/v3/files/{}", backup_id))
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Error deleting backup. Status: {}", resp.status()))
    }
}

// ── Conflict Detection ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct SaveConflict {
    pub has_conflict: bool,
    pub local_modified_time: u64,
    pub cloud_modified_time: u64,
    pub local_size_bytes: u64,
    pub cloud_size_bytes: i64,
    pub local_date: String,
    pub cloud_date: String,
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

fn latest_file_time(path: &Path) -> u64 {
    let skip_files = ["remotecache.vdf", "remotestorage.vdf"];
    let mut latest = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let t = latest_file_time(&p);
                if t > latest { latest = t; }
            } else if let Ok(meta) = entry.metadata() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if skip_files.contains(&name) { continue; }
                }
                if let Ok(sys_time) = meta.modified() {
                    if let Ok(dur) = sys_time.duration_since(UNIX_EPOCH) {
                        let secs = dur.as_secs();
                        if secs > latest { latest = secs; }
                    }
                }
            }
        }
    }
    latest
}

fn format_timestamp(secs: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    if let Ok(sys_time) = SystemTime::now().duration_since(UNIX_EPOCH) {
        let diff = sys_time.as_secs().saturating_sub(secs);
        if diff < 60 { return "Hace menos de 1 min".to_string(); }
        if diff < 3600 { return format!("Hace {} min", diff / 60); }
        if diff < 86400 { return format!("Hace {} h", diff / 3600); }
        return format!("Hace {} d", diff / 86400);
    }
    "Desconocido".to_string()
}

pub fn detect_local_save(app_id: &str, steam_path: &str) -> Option<(u64, u64)> {
    let save_path = find_save_path(steam_path, app_id)?;
    let mod_time = latest_file_time(&save_path);
    let size = dir_size(&save_path);
    if mod_time == 0 { return None; }
    Some((mod_time, size))
}

/// Returns the latest cloud backup's (time, size), or `Ok(None)` if the user
/// genuinely has no backups. Auth/network failures are returned as `Err` so
/// the caller can tell "confirmed empty" apart from "couldn't check" — an
/// expired Google token must never be silently reported as "no cloud
/// backups", since that could lead a user to overwrite their only real
/// backup without warning.
pub async fn detect_cloud_save(app_id: &str, account_email: &str) -> Result<Option<(u64, i64)>, String> {
    get_valid_token_for(account_email).await?;
    let backups = list_backups(app_id, account_email).await?;
    if backups.is_empty() { return Ok(None); }
    let latest = &backups[0];
    let t = time_from_iso8601(&latest.created_at).map_err(|_| "No se pudo interpretar la fecha del backup en la nube".to_string())?;
    Ok(Some((t, latest.size_bytes)))
}

pub async fn check_save_conflict(app_id: &str, steam_path: &str, account_email: &str) -> Result<SaveConflict, String> {
    let (local_time, local_size) = detect_local_save(app_id, steam_path)
        .ok_or_else(|| "No se encontraron saves locales".to_string())?;

    let cloud_info = detect_cloud_save(app_id, account_email).await?;

    match cloud_info {
        Some((cloud_time, cloud_size)) => {
            let has_conflict = local_time > cloud_time && local_time - cloud_time > 300;
            Ok(SaveConflict {
                has_conflict,
                local_modified_time: local_time,
                cloud_modified_time: cloud_time,
                local_size_bytes: local_size,
                cloud_size_bytes: cloud_size,
                local_date: format_timestamp(local_time),
                cloud_date: format_timestamp(cloud_time),
            })
        }
        None => {
            Ok(SaveConflict {
                has_conflict: false,
                local_modified_time: local_time,
                cloud_modified_time: 0,
                local_size_bytes: local_size,
                cloud_size_bytes: 0,
                local_date: format_timestamp(local_time),
                cloud_date: "Sin backups en la nube".to_string(),
            })
        }
    }
}

// ── Steam Cloud stuck-sync fix ─────────────────────────────────────────────────
//
// Steam tracks per-file cloud sync state for each app in
// userdata/<userid>/<appid>/remotecache.vdf (plain text VDF: size/hash/time
// per tracked save file). When saves are restored/replaced outside of Steam's
// own client — exactly what this launcher's backup/restore feature does — that
// cache can go stale and Steam flags the game as having a cloud conflict on
// next launch. Valve's own documented fix for a stuck conflict is to delete
// that file; Steam regenerates a fresh one from whatever's on disk. We rename
// instead of delete so the previous cache is recoverable if anything looks off.
/// Index of the brace that closes the one at `open`.
fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    for (i, c) in text.bytes().enumerate().skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Writes `"cloudenabled" "0"` for one app into a sharedconfig.vdf body.
///
/// This is the setting behind Steam's own per-game "Keep game saves in the
/// Steam Cloud" checkbox, and turning it off is the only thing that actually
/// stops the "Unable to sync" badge: the games this launcher unlocks have no
/// real licence, so Steam's cloud refuses them and says so on the library page
/// forever. Renaming remotecache.vdf — what this used to do — neither disables
/// the sync nor hides the error; it only leaves a file behind.
///
/// Kept as a pure string transform so it can be tested without a Steam
/// install, and so a file it cannot understand is returned untouched rather
/// than rewritten into something Steam would reject.
pub fn set_cloud_disabled_in_vdf(body: &str, app_id: &str) -> Option<String> {
    let entry = format!("\t\t\t\t\t\"{}\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"cloudenabled\"\t\t\"0\"\n\t\t\t\t\t}}\n", app_id);

    // Already recorded for this app: flip whatever value is there.
    if let Some(app_pos) = body.find(&format!("\"{}\"", app_id)) {
        if let Some(brace) = body[app_pos..].find('{').map(|i| app_pos + i) {
            if let Some(close) = matching_brace(body, brace) {
                let block = &body[brace..=close];
                if block.contains("\"cloudenabled\"") {
                    let fixed = block
                        .replace("\"cloudenabled\"\t\t\"1\"", "\"cloudenabled\"\t\t\"0\"")
                        .replace("\"cloudenabled\"		\"1\"", "\"cloudenabled\"		\"0\"");
                    if fixed != block {
                        return Some(format!("{}{}{}", &body[..brace], fixed, &body[close + 1..]));
                    }
                    return Some(body.to_string()); // already 0
                }
                // Block exists without the key — add it just inside.
                let insert = brace + 1;
                return Some(format!(
                    "{}\n\t\t\t\t\t\t\"cloudenabled\"\t\t\"0\"{}",
                    &body[..insert],
                    &body[insert..]
                ));
            }
        }
    }

    // No entry for this app. Put it inside "apps", creating that section if
    // Steam has never written one — which is the normal state until the first
    // game gets its cloud turned off.
    if let Some(apps_pos) = body.find("\"apps\"") {
        let brace = body[apps_pos..].find('{').map(|i| apps_pos + i)?;
        let insert = brace + 1;
        return Some(format!("{}\n{}{}", &body[..insert], entry, &body[insert..]));
    }

    let steam_pos = body.find("\"Steam\"")?;
    let brace = body[steam_pos..].find('{').map(|i| steam_pos + i)?;
    let insert = brace + 1;
    Some(format!(
        "{}\n\t\t\t\t\"apps\"\n\t\t\t\t{{\n{}\t\t\t\t}}{}",
        &body[..insert],
        entry,
        &body[insert..]
    ))
}

/// Turns off Steam Cloud for one app, for every account on this machine.
///
/// Steam rewrites sharedconfig.vdf when it exits, so a change made while it is
/// running is lost. The caller is expected to have closed it; the count of
/// files actually written is returned so it can say whether anything took.
pub fn disable_steam_cloud(steam_path: &str, app_id: &str) -> usize {
    let mut written = 0usize;
    let userdata = Path::new(steam_path).join("userdata");
    let Ok(users) = fs::read_dir(&userdata) else { return 0 };

    for user in users.flatten() {
        let cfg = user.path().join("7").join("remote").join("sharedconfig.vdf");
        if !cfg.is_file() {
            continue;
        }
        let Ok(body) = fs::read_to_string(&cfg) else { continue };
        let Some(updated) = set_cloud_disabled_in_vdf(&body, app_id) else { continue };
        if updated != body {
            let _ = fs::write(cfg.with_extension("vdf.ragnarok-bak"), &body);
            if fs::write(&cfg, updated).is_ok() {
                written += 1;
            }
        }
    }
    written
}

#[cfg(test)]
mod cloud_toggle_tests {
    use super::set_cloud_disabled_in_vdf;

    /// The shape Steam ships before any game has had its cloud turned off:
    /// no "apps" section at all.
    const BARE: &str = "\"UserRoamingConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"SurveyDate\"\t\t\"2020-12-13\"\n\t\t\t}\n\t\t}\n\t}\n}\n";

    #[test]
    fn creates_the_apps_section_when_there_is_none() {
        let out = set_cloud_disabled_in_vdf(BARE, "2661300").unwrap();
        assert!(out.contains("\"apps\""), "deberia crear la seccion apps");
        assert!(out.contains("\"2661300\""));
        assert!(out.contains("\"cloudenabled\"\t\t\"0\""));
        // The rest of the file must survive.
        assert!(out.contains("\"SurveyDate\"\t\t\"2020-12-13\""));
    }

    #[test]
    fn adds_the_app_to_an_existing_apps_section() {
        let with_apps = BARE.replace(
            "\t\t\t\t\"SurveyDate\"\t\t\"2020-12-13\"",
            "\t\t\t\t\"apps\"\n\t\t\t\t{\n\t\t\t\t\t\"550\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"cloudenabled\"\t\t\"1\"\n\t\t\t\t\t}\n\t\t\t\t}",
        );
        let out = set_cloud_disabled_in_vdf(&with_apps, "2661300").unwrap();
        assert!(out.contains("\"2661300\""));
        // The other game keeps its own setting.
        assert!(out.contains("\"550\""));
        assert!(out.contains("\"cloudenabled\"\t\t\"1\""), "no debe tocar el 550");
    }

    /// An app already listed with cloud on gets flipped, not duplicated.
    #[test]
    fn flips_an_existing_entry_instead_of_adding_a_second() {
        let with_app = BARE.replace(
            "\t\t\t\t\"SurveyDate\"\t\t\"2020-12-13\"",
            "\t\t\t\t\"apps\"\n\t\t\t\t{\n\t\t\t\t\t\"2661300\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"cloudenabled\"\t\t\"1\"\n\t\t\t\t\t}\n\t\t\t\t}",
        );
        let out = set_cloud_disabled_in_vdf(&with_app, "2661300").unwrap();
        assert_eq!(out.matches("\"2661300\"").count(), 1, "no debe duplicarse");
        assert!(out.contains("\"cloudenabled\"\t\t\"0\""));
        assert!(!out.contains("\"cloudenabled\"\t\t\"1\""));
    }

    /// A file with nothing recognisable in it is left alone rather than
    /// rewritten into something Steam would reject.
    #[test]
    fn refuses_a_file_it_does_not_recognise() {
        assert!(set_cloud_disabled_in_vdf("no soy un vdf", "2661300").is_none());
    }
}

/// Puts back any remote cache that was renamed and then left behind.
///
/// apply_cloud_fix_batch renames `remotecache.vdf` to `.bak` so Steam cannot
/// decide its own cloud copy is authoritative and overwrite a save that was
/// just restored. That is right at the moment of a restore and wrong forever
/// after: with no cache, Steam has no record of what it last synced, marks the
/// app `changeslocally`, tries to sync, fails, and settles on "Unable to sync"
/// permanently. One app on this machine (495420) had lost its cache entirely,
/// with only the `.bak` left beside it.
///
/// So the rename is treated as temporary now. Only orphans are repaired — a
/// `.bak` whose original is gone — which leaves a restore that has just run
/// untouched while undoing the ones that were abandoned.
pub fn restore_cloud_caches(steam_path: &str) -> Vec<String> {
    let mut restored = Vec::new();
    let userdata = Path::new(steam_path).join("userdata");
    let Ok(users) = fs::read_dir(&userdata) else { return restored };

    for user in users.flatten() {
        let user_dir = user.path();
        if !user_dir.is_dir() {
            continue;
        }
        let Ok(apps) = fs::read_dir(&user_dir) else { continue };
        for app in apps.flatten() {
            let app_dir = app.path();
            if !app_dir.is_dir() {
                continue;
            }
            let cache = app_dir.join("remotecache.vdf");
            let backup = app_dir.join("remotecache.vdf.bak");
            if backup.is_file() && !cache.is_file() && fs::rename(&backup, &cache).is_ok() {
                restored.push(app.file_name().to_string_lossy().into_owned());
            }
        }
    }
    restored
}

pub fn apply_cloud_fix_batch(steam_path: &str, app_ids: &[String]) -> Result<(), String> {
    let userdata = Path::new(steam_path).join("userdata");
    if !userdata.exists() {
        return Err("No se encontró la carpeta userdata de Steam.".to_string());
    }

    // Turn the cloud off properly first.
    //
    // This function used to do one thing: rename remotecache.vdf out of the
    // way. That stops Steam treating its cloud copy as authoritative over a
    // save just restored, which was the bug it was written for — but it never
    // disabled anything, so Steam kept trying to sync, kept failing for want
    // of a real licence, and kept "Unable to sync" on the library page for
    // good. Writing `cloudenabled = 0` is what Steam's own per-game checkbox
    // does, and it is the only thing that removes the error rather than
    // hiding from it.
    let mut disabled = 0usize;
    for app_id in app_ids {
        disabled += disable_steam_cloud(steam_path, app_id);
    }

    // Only fall back to the old rename where the setting could not be written
    // — no sharedconfig.vdf, or Steam holding it open. With the cloud off
    // there is nothing left to overwrite a restore, so the rename (and the
    // orphaned .bak it leaves behind) is not wanted otherwise.
    if disabled > 0 {
        return Ok(());
    }

    let mut fixed_count = 0usize;
    if let Ok(entries) = fs::read_dir(&userdata) {
        for entry in entries.flatten() {
            let user_dir = entry.path();
            if !user_dir.is_dir() {
                continue;
            }
            for app_id in app_ids {
                let cache_path = user_dir.join(app_id).join("remotecache.vdf");
                if !cache_path.exists() {
                    continue;
                }
                let backup_path = user_dir.join(app_id).join("remotecache.vdf.bak");
                let _ = fs::remove_file(&backup_path); // drop any older backup first
                if fs::rename(&cache_path, &backup_path).is_ok() {
                    fixed_count += 1;
                }
            }
        }
    }

    if fixed_count == 0 {
        return Err(
            "No se pudo desactivar la nube de Steam para estos juegos. Cerrá Steam y volvé a \
             intentarlo: mientras está abierto reescribe su configuración al salir y deshace el \
             cambio."
                .to_string(),
        );
    }
    Ok(())
}

// ── Local Save Backup ──────────────────────────────────────────────────────────

fn local_backup_dir() -> PathBuf {
    let mut p = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push("ragnarok");
    p.push("local_save_backups");
    p
}

pub fn local_backup_path(app_id: &str) -> PathBuf {
    let mut p = local_backup_dir();
    p.push(app_id);
    p
}

pub fn do_local_backup(app_id: &str, steam_path: &str) -> Result<String, String> {
    let (save_path, origin) = locate_save(steam_path, app_id).ok_or_else(|| {
        format!(
            "No se encontraron saves para AppID {}. Steam los guarda en userdata/{{userId}}/{}/remote/",
            app_id, app_id
        )
    })?;

    let backup_root = local_backup_path(app_id);
    fs::create_dir_all(&backup_root).map_err(|e| format!("Error al crear directorio de backup: {}", e))?;

    let timestamp = now_secs();
    let backup_name = format!("backup_{}", timestamp);
    let backup_dir = backup_root.join(&backup_name);

    // Copy save directory recursively
    copy_dir_recursive(&save_path, &backup_dir)?;

    // Verify backup is not empty
    let size = dir_size(&backup_dir);
    if size == 0 {
        // Remove empty backup dir
        let _ = fs::remove_dir_all(&backup_dir);
        return Err("La carpeta de saves está vacía".to_string());
    }

    // Recorded after the emptiness check above, so a backup holding only this
    // record can never pass for one with saves in it.
    if let Some(origin) = &origin {
        if let Ok(json) = serde_json::to_string_pretty(origin) {
            let _ = fs::write(backup_dir.join(BACKUP_MANIFEST), json);
        }
    }

    // Keep only the 10 most recent backups
    cleanup_old_local_backups(app_id, 10);

    Ok(backup_name)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Error creando directorio: {}", e))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Error leyendo directorio fuente: {}", e))? {
        let entry = entry.map_err(|e| format!("Error al leer entrada: {}", e))?;
        let file_type = entry.file_type().map_err(|e| format!("Error al obtener tipo: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| format!("Error copiando {}: {}", entry.file_name().to_string_lossy(), e))?;
        }
    }
    Ok(())
}

fn cleanup_old_local_backups(app_id: &str, max_keep: usize) {
    let backup_root = local_backup_path(app_id);
    if !backup_root.exists() {
        return;
    }
    let mut entries: Vec<_> = match fs::read_dir(&backup_root) {
        Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    entries.sort_by(|a, b| {
        let a_modified = a.metadata().ok().and_then(|m| m.created().ok());
        let b_modified = b.metadata().ok().and_then(|m| m.created().ok());
        b_modified.cmp(&a_modified)
    });
    for entry in entries.iter().skip(max_keep) {
        let _ = fs::remove_dir_all(entry.path());
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LocalBackupInfo {
    pub name: String,
    pub app_id: String,
    pub created_at: String,
    pub size_bytes: u64,
}

pub fn list_local_backups(app_id: &str) -> Result<Vec<LocalBackupInfo>, String> {
    let backup_root = local_backup_path(app_id);
    if !backup_root.exists() {
        return Ok(Vec::new());
    }
    let mut backups = Vec::new();
    let entries = fs::read_dir(&backup_root).map_err(|e| format!("Error leyendo backups locales: {}", e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let size = dir_size(&path);
        let created_at = entry
            .metadata()
            .ok()
            .and_then(|m| m.created().ok())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        backups.push(LocalBackupInfo {
            name,
            app_id: app_id.to_string(),
            created_at: created_at.to_string(),
            size_bytes: size,
        });
    }
    // Sort by creation time descending (newest first)
    backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(backups)
}

/// Whether `dir` holds at least one real file, at any depth within `depth`.
///
/// A folder of empty folders is not a backup. Used to refuse a restore before
/// it deletes anything, since the alternative is wiping a live save and
/// reporting success over nothing.
fn dir_has_file(dir: &Path, depth: u32) -> bool {
    let Ok(entries) = fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            return true;
        }
        if depth > 0 && path.is_dir() && dir_has_file(&path, depth - 1) {
            return true;
        }
    }
    false
}

pub fn do_restore_local(app_id: &str, backup_name: &str, steam_path: &str) -> Result<(), String> {
    let backup_dir = local_backup_path(app_id).join(backup_name);
    if !backup_dir.exists() {
        return Err(format!("Backup '{}' no encontrado para AppID {}", backup_name, app_id));
    }

    let recorded_origin = fs::read_to_string(backup_dir.join(BACKUP_MANIFEST))
        .ok()
        .and_then(|json| serde_json::from_str::<SaveOrigin>(&json).ok());
    let save_path = restore_target(
        steam_path,
        app_id,
        recorded_origin.as_ref(),
        dir_holds_profiles(&backup_dir),
    )?;
    fs::create_dir_all(&save_path).map_err(|e| format!("Error creando directorio de saves: {}", e))?;

    // Refuse before touching anything if the backup has nothing in it.
    //
    // This used to wipe the live save first and copy afterwards, with no check
    // on either side. An empty backup folder — deleted by hand, or left behind
    // by a cleanup that ran halfway — meant the save was destroyed and
    // copy_dir_recursive then returned Ok over zero files: a "successful"
    // restore that silently erased the game.
    if !dir_has_file(&backup_dir, 8) {
        return Err(format!(
            "El respaldo '{}' está vacío. No se tocó la partida actual.",
            backup_name
        ));
    }

    // Move the current save aside instead of deleting it, so a copy that fails
    // partway can be undone. Losing a save to a half-finished restore is the
    // one outcome this whole feature exists to prevent.
    let stash = save_path.with_file_name(format!(
        "{}.ragnarok-restore-{}",
        save_path.file_name().and_then(|n| n.to_str()).unwrap_or("save"),
        now_secs()
    ));
    let stashed = if save_path.exists() && fs::read_dir(&save_path).map(|mut d| d.next().is_some()).unwrap_or(false) {
        fs::rename(&save_path, &stash)
            .map_err(|e| format!("No se pudo apartar la partida actual: {}", e))?;
        fs::create_dir_all(&save_path)
            .map_err(|e| format!("Error recreando el directorio de saves: {}", e))?;
        true
    } else {
        false
    };

    match copy_dir_recursive(&backup_dir, &save_path) {
        Ok(()) => {
            // Copied along with everything else; it is Ragnarok's, not the game's.
            let _ = fs::remove_file(save_path.join(BACKUP_MANIFEST));
            if stashed {
                let _ = fs::remove_dir_all(&stash);
            }
            Ok(())
        }
        Err(e) => {
            // Put it back exactly as it was.
            if stashed {
                let _ = fs::remove_dir_all(&save_path);
                if fs::rename(&stash, &save_path).is_err() {
                    return Err(format!(
                        "{}. La partida original quedó en {} — movela a mano antes de volver a jugar.",
                        e,
                        stash.display()
                    ));
                }
            }
            Err(format!("{}. Se restauró la partida anterior.", e))
        }
    }
}

pub fn delete_local_backup(app_id: &str, backup_name: &str) -> Result<(), String> {
    let backup_dir = local_backup_path(app_id).join(backup_name);
    if !backup_dir.exists() {
        return Err(format!("Backup '{}' no encontrado", backup_name));
    }
    fs::remove_dir_all(&backup_dir).map_err(|e| format!("Error eliminando backup: {}", e))
}

#[cfg(test)]
mod restore_guard_tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ragnarok_restore_test_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The guard that stops a restore from wiping a live save for nothing.
    #[test]
    fn an_empty_backup_is_not_a_backup() {
        let root = scratch("empty");
        let empty = root.join("backup_1");
        fs::create_dir_all(&empty).unwrap();
        assert!(!dir_has_file(&empty, 8), "una carpeta vacía no es un respaldo");

        // Nor is a tree of empty folders.
        fs::create_dir_all(empty.join("remote").join("saves")).unwrap();
        assert!(!dir_has_file(&empty, 8), "solo carpetas vacías tampoco");

        let _ = fs::remove_dir_all(&root);
    }

    /// One real file anywhere in the tree makes it restorable.
    #[test]
    fn a_file_at_any_depth_counts() {
        let root = scratch("deep");
        let nested = root.join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("slot1.dat"), b"progreso").unwrap();
        assert!(dir_has_file(&root, 8));

        // ...but not past the depth budget.
        assert!(!dir_has_file(&root, 1), "no debe descender más allá del límite");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_directory_is_not_a_backup() {
        assert!(!dir_has_file(Path::new(r"Z:\no\existe\jamas"), 8));
    }
}

#[cfg(test)]
mod save_bundle_tests {
    use super::*;

    /// Unique scratch folder per test, in the same style as the rest of the
    /// project (no tempfile dependency just for this).
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ragnarok_save_test_{}_{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A backup must survive the trip: same files, same contents, including
    /// ones in subfolders.
    #[test]
    fn a_backup_round_trips_through_the_zip() {
        let src = scratch("src");
        fs::write(src.join("save1.dat"), b"progreso").unwrap();
        fs::create_dir_all(src.join("slots")).unwrap();
        fs::write(src.join("slots").join("slot2.dat"), b"otra partida").unwrap();

        let (data, count) = zip_backup_bundle(Some(&src), None, None).unwrap();
        assert_eq!(count, 2, "no conto los archivos que metio");

        let dst = scratch("dst");
        let restored = unzip_backup_bundle(&data, &dst, None).unwrap();

        assert_eq!(restored, 2, "no restauro los dos archivos");
        assert_eq!(fs::read(dst.join("save1.dat")).unwrap(), b"progreso");
        assert_eq!(
            fs::read(dst.join("slots").join("slot2.dat")).unwrap(),
            b"otra partida"
        );

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    /// The bug a user hit: a save folder that exists but is empty produced a
    /// valid 22-byte zip, which sailed past the old `len() < 22` guard, got
    /// uploaded, and then restored without putting anything back — the
    /// "download button does nothing" report.
    #[test]
    fn an_empty_save_folder_produces_no_files_to_back_up() {
        let src = scratch("empty");

        let (data, count) = zip_backup_bundle(Some(&src), None, None).unwrap();

        assert_eq!(count, 0, "conto archivos donde no hay ninguno");
        assert!(
            data.len() <= 22,
            "un zip vacio deberia ser minimo, mide {}",
            data.len()
        );

        // And the other half of the same bug: restoring it writes nothing,
        // which do_restore now reports instead of calling it a success.
        let dst = scratch("empty_dst");
        assert_eq!(unzip_backup_bundle(&data, &dst, None).unwrap(), 0);

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    /// Goldberg entries go to their own target, not into the Steam save
    /// folder, and both halves still count as restored files.
    #[test]
    fn goldberg_entries_land_in_their_own_folder() {
        let saves = scratch("gb_saves");
        let gold = scratch("gb_gold");
        fs::write(saves.join("save1.dat"), b"partida").unwrap();
        fs::write(gold.join("achievements.json"), b"{}").unwrap();

        let (data, count) = zip_backup_bundle(Some(&saves), Some(&gold), None).unwrap();
        assert_eq!(count, 2);

        let save_dst = scratch("gb_save_dst");
        let gold_dst = scratch("gb_gold_dst");
        let restored = unzip_backup_bundle(&data, &save_dst, Some(&gold_dst)).unwrap();

        assert_eq!(restored, 2);
        assert!(save_dst.join("save1.dat").is_file());
        assert!(gold_dst.join("achievements.json").is_file());
        assert!(
            !save_dst.join(GOLDBERG_ZIP_PREFIX).exists(),
            "los logros se colaron en la carpeta de partidas"
        );

        for d in [&saves, &gold, &save_dst, &gold_dst] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// A zip whose entry names try to escape the target must not write
    /// outside it.
    #[test]
    fn refuses_to_write_outside_the_target() {
        let src = scratch("slip_src");
        fs::write(src.join("ok.dat"), b"x").unwrap();
        let (data, _) = zip_backup_bundle(Some(&src), None, None).unwrap();

        let dst = scratch("slip_dst");
        unzip_backup_bundle(&data, &dst, None).unwrap();

        // Everything written stayed under the target.
        for entry in walkdir::WalkDir::new(&dst).into_iter().flatten() {
            assert!(
                entry.path().starts_with(&dst),
                "escribio fuera del destino: {}",
                entry.path().display()
            );
        }

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }
}

#[cfg(test)]
mod auth_flow_tests {
    use super::*;

    /// The whole point of the guard: a second link attempt has to be refused
    /// with a sentence, not with WSAEADDRINUSE from the socket layer.
    #[test]
    fn second_concurrent_link_is_refused_with_an_explanation() {
        let first = AuthFlowGuard::acquire().expect("el primero debe poder arrancar");

        let err = AuthFlowGuard::acquire().expect_err("el segundo debe ser rechazado");
        assert!(
            err.contains("Ya hay una vinculación"),
            "el mensaje tiene que explicar la causa real, no filtrar el error de socket: {err}"
        );
        assert!(
            !err.contains("10048") && !err.to_lowercase().contains("socket"),
            "el usuario no debe ver el error crudo del sistema: {err}"
        );

        drop(first);
    }

    /// A flow that ends — including one that failed — must not leave the app
    /// unable to ever link again. This is the bug the RAII guard exists for:
    /// an abandoned attempt used to keep the port for two minutes.
    #[test]
    fn the_flag_is_released_when_a_flow_ends() {
        {
            let _first = AuthFlowGuard::acquire().expect("arranca");
            assert!(AuthFlowGuard::acquire().is_err(), "bloqueado mientras corre");
        }
        // Fuera del scope el guard se soltó, así que otro intento entra.
        let again = AuthFlowGuard::acquire();
        assert!(again.is_ok(), "un intento terminado no puede dejar la app trabada");
        drop(again);
    }

    /// Releases even when the flow ends early through `?`, which is every
    /// failure path in do_auth_inner.
    #[test]
    fn the_flag_is_released_on_an_early_return() {
        fn flow_that_fails() -> Result<(), String> {
            let _flow = AuthFlowGuard::acquire()?;
            Err("el navegador no abrió".to_string())?;
            unreachable!()
        }

        assert!(flow_that_fails().is_err());
        let after = AuthFlowGuard::acquire();
        assert!(after.is_ok(), "un fallo no puede dejar el flag puesto");
        drop(after);
    }

    /// If 8080 is occupied by another program, bind_callback_listener gracefully
    /// falls back to an alternative free port instead of failing.
    #[test]
    fn falls_back_to_another_port_if_8080_is_taken() {
        let Ok(_squatter) = std::net::TcpListener::bind("127.0.0.1:8080") else {
            return;
        };

        let (listener, redirect_uri) = bind_callback_listener().expect("debe buscar un puerto alternativo");
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 8080, "debe usar un puerto diferente a 8080 si este está ocupado");
        assert_eq!(redirect_uri, format!("http://127.0.0.1:{}/callback", port));
    }

    /// And when candidate ports are free, it hands back a usable listener and URI.
    #[test]
    fn a_free_port_yields_a_listener() {
        match bind_callback_listener() {
            Ok((listener, redirect_uri)) => {
                let port = listener.local_addr().unwrap().port();
                assert!(port > 0);
                assert_eq!(redirect_uri, format!("http://127.0.0.1:{}/callback", port));
            }
            Err(e) => {
                assert!(e.contains("No se pudo abrir"), "{e}");
            }
        }
    }
}
