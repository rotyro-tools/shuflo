use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::SysRng;
use rand::TryRng;
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use super::api::{http_client, ApiError, SpotifyClient, TokenSource, User};
use crate::state::{AccessToken, AppState};

pub const ACCOUNTS_BASE: &str = "https://accounts.spotify.com";
pub const SCOPES: &str =
    "playlist-read-private playlist-read-collaborative playlist-modify-public playlist-modify-private";
/// Both must be registered as `http://127.0.0.1:<port>/callback` in the Spotify app.
pub const CALLBACK_PORTS: [u16; 2] = [8898, 8899];

const KEYRING_SERVICE: &str = "com.rotyrotools.shuflo";
const KEYRING_USER: &str = "spotify-refresh-token";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Refresh the access token when less than this much lifetime is left.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

const LOGIN_DONE_HTML: &str = "<html><head><title>Shuflo</title></head><body \
    style=\"font-family:system-ui,sans-serif;text-align:center;padding-top:4rem\">\
    <h2>Shuflo is connected</h2><p>You can close this tab and go back to Shuflo.</p></body></html>";

#[derive(Debug)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// `n` bytes from the OS random source, base64url-encoded.
pub fn random_token(n: usize) -> Result<String, String> {
    let mut buf = vec![0u8; n];
    SysRng
        .try_fill_bytes(&mut buf)
        .map_err(|e| format!("OS random source failed: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// S256 code challenge for a PKCE verifier (RFC 7636 §4.2).
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn new_pkce() -> Result<Pkce, String> {
    // 64 bytes encode to 86 characters, inside RFC 7636's 43–128 range.
    let verifier = random_token(64)?;
    let challenge = challenge_for(&verifier);
    Ok(Pkce {
        verifier,
        challenge,
    })
}

pub fn authorize_url(
    accounts_base: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> Result<Url, String> {
    Url::parse_with_params(
        &format!("{accounts_base}/authorize"),
        [
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge),
            ("state", state),
            ("scope", SCOPES),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Extracts the authorization code from the loopback callback URL.
/// The listener is reachable by any local process, so `state` must match.
pub fn parse_callback(callback: &str, expected_state: &str) -> Result<String, String> {
    let url = Url::parse(callback).map_err(|_| "Spotify sent back an invalid login response.")?;
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    if param("state").as_deref() != Some(expected_state) {
        return Err("The login response didn't match this login. Try again.".to_string());
    }
    if let Some(error) = param("error") {
        return Err(if error == "access_denied" {
            "Spotify login was cancelled.".to_string()
        } else {
            format!("Spotify login failed: {error}")
        });
    }
    param("code")
        .filter(|c| !c.is_empty())
        .ok_or_else(|| "Spotify didn't send a login code. Try again.".to_string())
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub expires_in: u64,
    pub refresh_token: Option<String>,
}

async fn token_request(
    http: &reqwest::Client,
    accounts_base: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, ApiError> {
    let resp = http
        .post(format!("{accounts_base}/api/token"))
        .form(form)
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    if status.is_success() {
        return serde_json::from_slice(&bytes).map_err(|e| ApiError::Invalid(e.to_string()));
    }
    // A revoked or expired refresh token comes back as 400 invalid_grant.
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    if body.get("error").and_then(|e| e.as_str()) == Some("invalid_grant") {
        return Err(ApiError::LoggedOut);
    }
    let message = body
        .get("error_description")
        .and_then(|e| e.as_str())
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("unknown error"))
        .to_string();
    Err(ApiError::Status(status.as_u16(), message))
}

pub async fn exchange_code(
    http: &reqwest::Client,
    accounts_base: &str,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<TokenResponse, ApiError> {
    token_request(
        http,
        accounts_base,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ],
    )
    .await
}

pub async fn refresh(
    http: &reqwest::Client,
    accounts_base: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse, ApiError> {
    token_request(
        http,
        accounts_base,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )
    .await
}

// ── Refresh token storage (OS keychain, with an in-memory copy) ──────────────

fn keychain_entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
}

/// The stored refresh token, reading the keychain only on first use.
fn stored_refresh_token(state: &AppState) -> Option<String> {
    if let Some(token) = state.refresh_token.lock().ok()?.clone() {
        return Some(token);
    }
    match keychain_entry().and_then(|e| e.get_password()) {
        Ok(token) => {
            if let Ok(mut slot) = state.refresh_token.lock() {
                *slot = Some(token.clone());
            }
            Some(token)
        }
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            eprintln!("[shuflo] keychain read failed: {e}");
            state.keychain_ok.store(false, Ordering::SeqCst);
            None
        }
    }
}

fn store_refresh_token(state: &AppState, token: &str) {
    if let Ok(mut slot) = state.refresh_token.lock() {
        *slot = Some(token.to_string());
    }
    match keychain_entry().and_then(|e| e.set_password(token)) {
        Ok(()) => state.keychain_ok.store(true, Ordering::SeqCst),
        Err(e) => {
            // Keep the session for this run; Settings warns that it won't survive a restart.
            eprintln!("[shuflo] keychain write failed: {e}");
            state.keychain_ok.store(false, Ordering::SeqCst);
        }
    }
}

pub fn has_session(app: &AppHandle) -> bool {
    stored_refresh_token(&app.state::<AppState>()).is_some()
}

pub async fn logout(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut slot) = state.refresh_token.lock() {
        *slot = None;
    }
    if let Ok(mut user) = state.user.lock() {
        *user = None;
    }
    *state.token.lock().await = None;
    match keychain_entry().and_then(|e| e.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => eprintln!("[shuflo] keychain delete failed: {e}"),
    }
}

/// A valid access token, refreshed when it is close to expiry or when `force` is set.
pub async fn access_token(app: &AppHandle, force: bool) -> Result<String, ApiError> {
    let state = app.state::<AppState>();
    let mut cached = state.token.lock().await;
    if !force {
        if let Some(token) = cached.as_ref() {
            if token.expires_at > Instant::now() + REFRESH_MARGIN {
                return Ok(token.value.clone());
            }
        }
    }

    let refresh_token = stored_refresh_token(&state).ok_or(ApiError::LoggedOut)?;
    let client_id = crate::settings::client_id(app).map_err(ApiError::Other)?;
    let tokens = match refresh(&http_client(), ACCOUNTS_BASE, &client_id, &refresh_token).await {
        Ok(tokens) => tokens,
        Err(ApiError::LoggedOut) => {
            drop(cached);
            logout(app).await;
            return Err(ApiError::LoggedOut);
        }
        Err(e) => return Err(e),
    };
    // Spotify may rotate the refresh token; keep the newest one.
    if let Some(rotated) = tokens.refresh_token.as_deref() {
        if rotated != refresh_token {
            store_refresh_token(&state, rotated);
        }
    }
    *cached = Some(AccessToken {
        value: tokens.access_token.clone(),
        expires_at: Instant::now() + Duration::from_secs(tokens.expires_in),
    });
    Ok(tokens.access_token)
}

/// Token source backed by the app's stored session.
pub struct AppTokens(pub AppHandle);

#[async_trait]
impl TokenSource for AppTokens {
    async fn token(&self, force_refresh: bool) -> Result<String, ApiError> {
        access_token(&self.0, force_refresh).await
    }
}

pub fn client(app: &AppHandle) -> SpotifyClient {
    SpotifyClient::new(std::sync::Arc::new(AppTokens(app.clone())))
}

/// The logged-in Spotify user, cached after the first lookup.
pub async fn current_user(app: &AppHandle) -> Result<User, ApiError> {
    let state = app.state::<AppState>();
    if let Some(user) = state.user.lock().ok().and_then(|u| u.clone()) {
        return Ok(user);
    }
    let user = client(app).me().await?;
    if let Ok(mut slot) = state.user.lock() {
        *slot = Some(user.clone());
    }
    Ok(user)
}

/// Runs the Authorization Code + PKCE flow through the default browser and a
/// one-shot loopback listener, then stores the session.
pub async fn login(app: &AppHandle) -> Result<User, String> {
    let client_id = crate::settings::client_id(app)?;
    let pkce = new_pkce()?;
    let state_token = random_token(16)?;

    // A second click restarts the flow instead of leaving a stale listener behind.
    let previous = app
        .state::<AppState>()
        .login_port
        .lock()
        .ok()
        .and_then(|mut p| p.take());
    if let Some(port) = previous {
        let _ = tauri_plugin_oauth::cancel(port);
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut tx = Some(tx);
    let port = tauri_plugin_oauth::start_with_config(
        tauri_plugin_oauth::OauthConfig {
            ports: Some(CALLBACK_PORTS.to_vec()),
            response: Some(LOGIN_DONE_HTML.into()),
            redirect_uri: None,
        },
        move |url| {
            if let Some(tx) = tx.take() {
                let _ = tx.send(url);
            }
        },
    )
    .map_err(|e| format!("Couldn't start the login listener on port 8898 or 8899: {e}"))?;
    if let Ok(mut slot) = app.state::<AppState>().login_port.lock() {
        *slot = Some(port);
    }

    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let result = async {
        let url = authorize_url(
            ACCOUNTS_BASE,
            &client_id,
            &redirect_uri,
            &pkce.challenge,
            &state_token,
        )?;
        app.opener()
            .open_url(url.as_str(), None::<&str>)
            .map_err(|e| format!("Couldn't open the browser: {e}"))?;
        match tokio::time::timeout(LOGIN_TIMEOUT, rx).await {
            Ok(Ok(callback)) => Ok(callback),
            Ok(Err(_)) => Err("Login was interrupted. Try again.".to_string()),
            Err(_) => Err("Login timed out. Try again.".to_string()),
        }
    }
    .await;

    // The listener stops by itself after a callback; cancel it on every other path.
    let still_ours = app
        .state::<AppState>()
        .login_port
        .lock()
        .ok()
        .and_then(|mut p| if *p == Some(port) { p.take() } else { None });
    let callback = match result {
        Ok(callback) => callback,
        Err(e) => {
            if still_ours.is_some() {
                let _ = tauri_plugin_oauth::cancel(port);
            }
            return Err(e);
        }
    };

    let code = parse_callback(&callback, &state_token)?;
    let tokens = exchange_code(
        &http_client(),
        ACCOUNTS_BASE,
        &client_id,
        &code,
        &redirect_uri,
        &pkce.verifier,
    )
    .await?;
    let refresh_token = tokens
        .refresh_token
        .ok_or("Spotify didn't return a refresh token.")?;

    let state = app.state::<AppState>();
    store_refresh_token(&state, &refresh_token);
    if let Ok(mut user) = state.user.lock() {
        *user = None;
    }
    *state.token.lock().await = Some(AccessToken {
        value: tokens.access_token,
        expires_at: Instant::now() + Duration::from_secs(tokens.expires_in),
    });

    match current_user(app).await {
        Ok(user) => Ok(user),
        // e.g. an account not added under User Management: don't keep a session it can't use.
        Err(e @ ApiError::Forbidden(_)) => {
            logout(app).await;
            Err(e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[test]
    fn challenge_matches_rfc_7636_appendix_b() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn new_pkce_is_random_and_well_formed() {
        let a = new_pkce().unwrap();
        let b = new_pkce().unwrap();
        assert_ne!(a.verifier, b.verifier);
        assert_eq!(a.verifier.len(), 86);
        assert!(a
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(a.challenge, challenge_for(&a.verifier));
    }

    #[test]
    fn authorize_url_carries_pkce_state_and_scopes() {
        let url = authorize_url(
            ACCOUNTS_BASE,
            "client",
            "http://127.0.0.1:8898/callback",
            "chal",
            "st",
        )
        .unwrap();
        assert_eq!(url.host_str(), Some("accounts.spotify.com"));
        assert_eq!(url.path(), "/authorize");
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "client");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:8898/callback");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], "chal");
        assert_eq!(q["state"], "st");
        assert_eq!(q["scope"], SCOPES);
    }

    #[test]
    fn parse_callback_returns_the_code_for_a_matching_state() {
        let url = "http://127.0.0.1:8898/callback?code=abc&state=xyz";
        assert_eq!(parse_callback(url, "xyz").unwrap(), "abc");
    }

    #[test]
    fn parse_callback_rejects_bad_responses() {
        let mismatched = "http://127.0.0.1:8898/callback?code=abc&state=other";
        assert!(parse_callback(mismatched, "xyz")
            .unwrap_err()
            .contains("didn't match"));
        let denied = "http://127.0.0.1:8898/callback?error=access_denied&state=xyz";
        assert_eq!(
            parse_callback(denied, "xyz").unwrap_err(),
            "Spotify login was cancelled."
        );
        let no_code = "http://127.0.0.1:8898/callback?state=xyz";
        assert!(parse_callback(no_code, "xyz").is_err());
        assert!(parse_callback("not a url", "xyz").is_err());
    }

    #[tokio::test]
    async fn exchange_code_posts_the_pkce_form() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("code_verifier=ver"))
            .and(body_string_contains("client_id=client"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at", "token_type": "Bearer", "expires_in": 3600,
                "refresh_token": "rt", "scope": SCOPES
            })))
            .mount(&server)
            .await;
        let tokens = exchange_code(
            &http_client(),
            &server.uri(),
            "client",
            "code",
            "http://127.0.0.1:8898/callback",
            "ver",
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.expires_in, 3600);
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt"));
    }

    #[tokio::test]
    async fn revoked_refresh_token_means_logged_out() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant", "error_description": "Refresh token revoked"
            })))
            .mount(&server)
            .await;
        let err = refresh(&http_client(), &server.uri(), "client", "rt")
            .await
            .unwrap_err();
        assert_eq!(err, ApiError::LoggedOut);
    }
}
