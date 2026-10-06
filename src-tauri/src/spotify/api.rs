use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const API_BASE: &str = "https://api.spotify.com/v1";
/// Get Playlist Items returns at most 50 entries per page.
pub const READ_PAGE_SIZE: usize = 50;
/// Replace and Add Items accept at most 100 URIs per call.
pub const WRITE_CHUNK_SIZE: usize = 100;

/// Maximum response body size accepted from Spotify (4 MB).
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    LoggedOut,
    Forbidden(String),
    NotFound,
    RateLimited,
    Status(u16, String),
    Network(String),
    Invalid(String),
    Other(String),
}

const NOT_EDITABLE: &str = "Only the playlist's owner or its collaborators can shuffle it.";
const NOT_REGISTERED: &str = "This Spotify account isn't allowed to use the Spotify app yet. \
     Ask the app's owner to add it under User Management in the Spotify Developer Dashboard.";

impl ApiError {
    /// Spotify's development mode refuses accounts that weren't added under User Management
    /// ("…the user may not be registered").
    pub fn is_unregistered_user(&self) -> bool {
        matches!(self, Self::Forbidden(message) if message.to_ascii_lowercase().contains("registered"))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoggedOut => write!(f, "Log in to Spotify again."),
            Self::Forbidden(_) if self.is_unregistered_user() => write!(f, "{NOT_REGISTERED}"),
            Self::Forbidden(message) => {
                write!(f, "{NOT_EDITABLE}")?;
                // Scope and other problems also come back as 403; keep Spotify's reason.
                if !message.is_empty() && !message.eq_ignore_ascii_case("forbidden") {
                    write!(f, " (Spotify: {message})")?;
                }
                Ok(())
            }
            Self::NotFound => write!(f, "Spotify couldn't find that playlist."),
            Self::RateLimited => write!(
                f,
                "Spotify is limiting requests right now. Try again in a few minutes."
            ),
            Self::Status(code, message) => write!(f, "Spotify error {code}: {message}"),
            Self::Network(e) => write!(f, "Couldn't reach Spotify: {e}"),
            Self::Invalid(e) => write!(f, "Unexpected response from Spotify: {e}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl From<ApiError> for String {
    fn from(e: ApiError) -> Self {
        e.to_string()
    }
}

/// Supplies bearer tokens; `force_refresh` asks for a new one after a 401.
#[async_trait]
pub trait TokenSource: Send + Sync {
    async fn token(&self, force_refresh: bool) -> Result<String, ApiError>;
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    /// First backoff after a 5xx or network error; doubles on each retry.
    pub base_delay: Duration,
    /// Upper bound on a single `Retry-After` wait.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            base_delay: Duration::from_millis(500),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInfo {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub collaborative: bool,
    pub snapshot_id: String,
    pub total: usize,
    /// Cover image sized for the playlist card, if the playlist has one.
    pub image_url: Option<String>,
}

impl PlaylistInfo {
    pub fn editable_by(&self, user_id: &str) -> bool {
        self.owner_id == user_id || self.collaborative
    }
}

/// One playlist entry; `uri` is `None` when Spotify no longer has the track.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub uri: Option<String>,
    pub is_local: bool,
}

#[derive(Deserialize)]
struct Total {
    #[serde(default)]
    total: usize,
}

#[derive(Deserialize)]
struct Owner {
    id: String,
}

// Spotify renamed `tracks` to `items` in February 2026; both are read so either shape works.
#[derive(Deserialize)]
struct RawPlaylist {
    id: String,
    name: String,
    owner: Owner,
    #[serde(default)]
    collaborative: bool,
    #[serde(default)]
    snapshot_id: String,
    items: Option<Total>,
    tracks: Option<Total>,
    #[serde(default)]
    images: Option<Vec<Image>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Image {
    pub url: String,
    pub width: Option<u32>,
}

/// Smallest image that still looks sharp in the card (64pt at 2x), else the largest.
/// Spotify lists images largest first; user-uploaded covers often have no width.
pub fn pick_image(images: &[Image]) -> Option<String> {
    const MIN_WIDTH: u32 = 128;
    images
        .iter()
        .filter(|i| i.width.is_some_and(|w| w >= MIN_WIDTH))
        .min_by_key(|i| i.width)
        .or_else(|| images.first())
        .map(|i| i.url.clone())
}

impl From<RawPlaylist> for PlaylistInfo {
    fn from(raw: RawPlaylist) -> Self {
        Self {
            id: raw.id,
            name: raw.name,
            owner_id: raw.owner.id,
            collaborative: raw.collaborative,
            snapshot_id: raw.snapshot_id,
            total: raw.items.or(raw.tracks).map(|t| t.total).unwrap_or(0),
            image_url: raw.images.as_deref().and_then(pick_image),
        }
    }
}

#[derive(Deserialize)]
struct UriRef {
    uri: Option<String>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    is_local: bool,
    item: Option<UriRef>,
    track: Option<UriRef>,
}

#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
    next: Option<String>,
    #[serde(default)]
    total: usize,
}

#[derive(Deserialize)]
struct Snapshot {
    snapshot_id: String,
}

/// Rejects anything that isn't a plain Spotify ID before it lands in a URL path.
pub fn check_id(id: &str) -> Result<(), ApiError> {
    if crate::backup::is_valid_id(id) {
        Ok(())
    } else {
        Err(ApiError::Other(format!("Invalid playlist ID: {id}")))
    }
}

/// Reads the full response body, enforcing the size cap regardless of whether
/// Content-Length is present.
async fn read_body_limited(resp: reqwest::Response) -> Result<Vec<u8>, ApiError> {
    if let Some(len) = resp.content_length() {
        if len > MAX_RESPONSE_BYTES {
            return Err(ApiError::Invalid(format!(
                "response too large ({len} bytes)"
            )));
        }
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(ApiError::Invalid(format!(
            "response too large ({} bytes)",
            body.len()
        )));
    }
    Ok(body.to_vec())
}

/// Pulls Spotify's `{"error": {"message": …}}` text out of an error body.
fn error_message(status: StatusCode, body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("error_description"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("unknown error")
                .to_string()
        })
}

fn retry_after(resp: &reqwest::Response) -> Duration {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(1))
}

pub struct SpotifyClient {
    http: reqwest::Client,
    api_base: String,
    retry: RetryPolicy,
    tokens: Arc<dyn TokenSource>,
}

impl SpotifyClient {
    pub fn new(tokens: Arc<dyn TokenSource>) -> Self {
        Self::with_base(API_BASE, RetryPolicy::default(), tokens)
    }

    pub fn with_base(api_base: &str, retry: RetryPolicy, tokens: Arc<dyn TokenSource>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            http,
            api_base: api_base.trim_end_matches('/').to_string(),
            retry,
            tokens,
        }
    }

    fn backoff(&self, attempt: u32) -> Duration {
        self.retry.base_delay * 2u32.saturating_pow(attempt.saturating_sub(1))
    }

    fn url(&self, path: &str) -> Result<Url, ApiError> {
        Url::parse(&format!("{}{}", self.api_base, path))
            .map_err(|e| ApiError::Invalid(e.to_string()))
    }

    /// Sends one API call with retries: waits out 429s, backs off on 5xx and network
    /// errors, and refreshes the token once on a 401.
    async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        // `next` links come from Spotify; never send the token anywhere else.
        if !url.starts_with(&self.api_base) {
            return Err(ApiError::Invalid(format!("unexpected URL {url}")));
        }

        let mut attempt = 0;
        let mut force_refresh = false;
        let mut refreshed = false;
        loop {
            attempt += 1;
            let token = self.tokens.token(force_refresh).await?;
            force_refresh = false;

            let mut req = self.http.request(method.clone(), url).bearer_auth(&token);
            if let Some(body) = body {
                req = req.json(body);
            }

            let resp = match req.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    if attempt >= self.retry.max_attempts {
                        return Err(ApiError::Network(e.to_string()));
                    }
                    tokio::time::sleep(self.backoff(attempt)).await;
                    continue;
                }
            };

            let status = resp.status();
            if status.is_success() {
                let bytes = read_body_limited(resp).await?;
                if bytes.is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_slice(&bytes)
                    .map_err(|e| ApiError::Invalid(e.to_string()));
            }

            if status == StatusCode::UNAUTHORIZED {
                if refreshed {
                    return Err(ApiError::LoggedOut);
                }
                refreshed = true;
                force_refresh = true;
                continue;
            }

            if status == StatusCode::TOO_MANY_REQUESTS {
                if attempt >= self.retry.max_attempts {
                    return Err(ApiError::RateLimited);
                }
                tokio::time::sleep(retry_after(&resp).min(self.retry.max_retry_after)).await;
                continue;
            }

            let bytes = read_body_limited(resp).await.unwrap_or_default();
            let message = error_message(status, &bytes);
            if status.is_server_error() {
                if attempt >= self.retry.max_attempts {
                    return Err(ApiError::Status(status.as_u16(), message));
                }
                tokio::time::sleep(self.backoff(attempt)).await;
                continue;
            }
            return Err(match status {
                StatusCode::FORBIDDEN => ApiError::Forbidden(message),
                StatusCode::NOT_FOUND => ApiError::NotFound,
                _ => ApiError::Status(status.as_u16(), message),
            });
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, ApiError> {
        let value = self.request(Method::GET, url, None).await?;
        serde_json::from_value(value).map_err(|e| ApiError::Invalid(e.to_string()))
    }

    pub async fn me(&self) -> Result<User, ApiError> {
        self.get(self.url("/me")?.as_str()).await
    }

    /// Playlists the user owns or collaborates on — the only ones Shuflo can reorder.
    pub async fn my_playlists(&self, user_id: &str) -> Result<Vec<PlaylistInfo>, ApiError> {
        let mut url = self.url("/me/playlists")?;
        url.query_pairs_mut()
            .append_pair("limit", "50")
            .append_pair("offset", "0");
        let mut next = Some(url.to_string());
        let mut playlists = Vec::new();
        while let Some(url) = next {
            let page: Page<Option<RawPlaylist>> = self.get(&url).await?;
            playlists.extend(
                page.items
                    .into_iter()
                    .flatten()
                    .map(PlaylistInfo::from)
                    .filter(|p| p.editable_by(user_id)),
            );
            next = page.next;
        }
        Ok(playlists)
    }

    pub async fn playlist(&self, id: &str) -> Result<PlaylistInfo, ApiError> {
        check_id(id)?;
        let mut url = self.url(&format!("/playlists/{id}"))?;
        url.query_pairs_mut().append_pair(
            "fields",
            "id,name,owner(id),collaborative,snapshot_id,items(total),tracks(total),images(url,width)",
        );
        let raw: RawPlaylist = self.get(url.as_str()).await?;
        Ok(raw.into())
    }

    /// Reads every entry of the playlist in order, calling `on_page(read, total)` per page.
    pub async fn playlist_entries(
        &self,
        id: &str,
        on_page: impl Fn(usize, usize) + Send + Sync,
    ) -> Result<Vec<Entry>, ApiError> {
        check_id(id)?;
        let mut url = self.url(&format!("/playlists/{id}/items"))?;
        url.query_pairs_mut()
            .append_pair("limit", &READ_PAGE_SIZE.to_string())
            .append_pair("offset", "0")
            .append_pair("additional_types", "track,episode")
            .append_pair("fields", "items(is_local,item(uri),track(uri)),next,total");
        let mut next = Some(url.to_string());
        let mut entries = Vec::new();
        while let Some(url) = next {
            let page: Page<RawEntry> = self.get(&url).await?;
            entries.extend(page.items.into_iter().map(|raw| Entry {
                uri: raw.item.or(raw.track).and_then(|r| r.uri),
                is_local: raw.is_local,
            }));
            on_page(entries.len(), page.total.max(entries.len()));
            next = page.next;
        }
        Ok(entries)
    }

    /// Replaces the whole playlist with `uris` (at most 100).
    pub async fn replace_items(&self, id: &str, uris: &[String]) -> Result<String, ApiError> {
        self.write_items(Method::PUT, id, uris).await
    }

    /// Appends `uris` (at most 100) to the end of the playlist.
    pub async fn add_items(&self, id: &str, uris: &[String]) -> Result<String, ApiError> {
        self.write_items(Method::POST, id, uris).await
    }

    async fn write_items(
        &self,
        method: Method,
        id: &str,
        uris: &[String],
    ) -> Result<String, ApiError> {
        check_id(id)?;
        if uris.len() > WRITE_CHUNK_SIZE {
            return Err(ApiError::Other(format!(
                "at most {WRITE_CHUNK_SIZE} items per write"
            )));
        }
        let url = self.url(&format!("/playlists/{id}/items"))?;
        let value = self
            .request(method, url.as_str(), Some(&json!({ "uris": uris })))
            .await?;
        let snapshot: Snapshot =
            serde_json::from_value(value).map_err(|e| ApiError::Invalid(e.to_string()))?;
        Ok(snapshot.snapshot_id)
    }
}

#[cfg(test)]
pub mod test_support {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Hands out a fixed token and counts forced refreshes.
    #[derive(Default)]
    pub struct FakeTokens {
        pub refreshes: AtomicUsize,
    }

    #[async_trait]
    impl TokenSource for FakeTokens {
        async fn token(&self, force_refresh: bool) -> Result<String, ApiError> {
            if force_refresh {
                self.refreshes.fetch_add(1, Ordering::SeqCst);
            }
            Ok("test-token".to_string())
        }
    }

    pub fn fast_retry() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 6,
            base_delay: Duration::from_millis(1),
            max_retry_after: Duration::from_millis(10),
        }
    }

    pub fn client(server_uri: &str, tokens: Arc<FakeTokens>) -> SpotifyClient {
        SpotifyClient::with_base(&format!("{server_uri}/v1"), fast_retry(), tokens)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::test_support::{client, FakeTokens};
    use super::*;

    const PLAYLIST: &str = "37i9dQZF1DXcBWIGoYBM5M";

    #[tokio::test]
    async fn request_survives_429_5xx_and_one_401() {
        let server = MockServer::start().await;
        let me = json!({ "id": "owner", "display_name": "Owner" });
        Mock::given(method("GET"))
            .and(path("/v1/me"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/me"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(2)
            .with_priority(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/me"))
            .respond_with(ResponseTemplate::new(401))
            .up_to_n_times(1)
            .with_priority(3)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&me))
            .with_priority(4)
            .mount(&server)
            .await;

        let tokens = Arc::new(FakeTokens::default());
        let user = client(&server.uri(), tokens.clone()).me().await.unwrap();
        assert_eq!(user.id, "owner");
        assert_eq!(user.display_name.as_deref(), Some("Owner"));
        assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn second_401_means_logged_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/me"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = client(&server.uri(), Arc::default())
            .me()
            .await
            .unwrap_err();
        assert_eq!(err, ApiError::LoggedOut);
    }

    #[tokio::test]
    async fn persistent_5xx_gives_up_after_max_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/me"))
            .respond_with(
                ResponseTemplate::new(503)
                    .set_body_json(json!({ "error": { "status": 503, "message": "Busy" } })),
            )
            .mount(&server)
            .await;
        let err = client(&server.uri(), Arc::default())
            .me()
            .await
            .unwrap_err();
        assert_eq!(err, ApiError::Status(503, "Busy".to_string()));
        assert_eq!(server.received_requests().await.unwrap().len(), 6);
    }

    #[tokio::test]
    async fn forbidden_write_maps_to_the_owner_message() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(format!("/v1/playlists/{PLAYLIST}/items")))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({ "error": { "status": 403, "message": "Forbidden" } })),
            )
            .mount(&server)
            .await;
        let err = client(&server.uri(), Arc::default())
            .replace_items(PLAYLIST, &["spotify:track:a".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
        assert_eq!(err.to_string(), NOT_EDITABLE);
        let specific = ApiError::Forbidden("Insufficient client scope".to_string());
        assert_eq!(
            specific.to_string(),
            format!("{NOT_EDITABLE} (Spotify: Insufficient client scope)")
        );
    }

    #[test]
    fn unregistered_accounts_get_the_user_management_hint() {
        let err = ApiError::Forbidden(
            "Check settings on developer.spotify.com/dashboard, the user may not be registered."
                .to_string(),
        );
        assert!(err.is_unregistered_user());
        assert_eq!(err.to_string(), NOT_REGISTERED);
        assert!(!ApiError::Forbidden("Forbidden".to_string()).is_unregistered_user());
        assert!(!ApiError::NotFound.is_unregistered_user());
    }

    #[tokio::test]
    async fn my_playlists_follows_next_and_keeps_editable_ones() {
        let server = MockServer::start().await;
        let base = format!("{}/v1", server.uri());
        let playlist = |id: &str, owner: &str, collaborative: bool| {
            json!({ "id": id, "name": id, "owner": { "id": owner },
                    "collaborative": collaborative, "snapshot_id": "s", "items": { "total": 3 } })
        };
        Mock::given(method("GET"))
            .and(path("/v1/me/playlists"))
            .and(query_param("offset", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [playlist("mine", "owner", false), playlist("theirs", "someone", false)],
                "next": format!("{base}/me/playlists?offset=50&limit=50"),
                "total": 3
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/me/playlists"))
            .and(query_param("offset", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [playlist("shared", "someone", true)],
                "next": null,
                "total": 3
            })))
            .mount(&server)
            .await;

        let names: Vec<String> = client(&server.uri(), Arc::default())
            .my_playlists("owner")
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["mine", "shared"]);
    }

    #[tokio::test]
    async fn playlist_reads_old_and_new_total_fields() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/v1/playlists/{PLAYLIST}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": PLAYLIST, "name": "My Mix", "owner": { "id": "owner" },
                "collaborative": false, "snapshot_id": "snap", "tracks": { "total": 1000 },
                "images": [{ "url": "https://i.scdn.co/image/big", "width": 640, "height": 640 },
                           { "url": "https://i.scdn.co/image/mid", "width": 300, "height": 300 }]
            })))
            .mount(&server)
            .await;
        let info = client(&server.uri(), Arc::default())
            .playlist(PLAYLIST)
            .await
            .unwrap();
        assert_eq!(info.total, 1000);
        assert_eq!(info.snapshot_id, "snap");
        assert_eq!(
            info.image_url.as_deref(),
            Some("https://i.scdn.co/image/mid")
        );
        assert!(info.editable_by("owner"));
        assert!(!info.editable_by("someone"));
    }

    #[tokio::test]
    async fn next_links_to_other_hosts_are_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/me/playlists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [], "next": "https://evil.example/steal", "total": 0
            })))
            .mount(&server)
            .await;
        let err = client(&server.uri(), Arc::default())
            .my_playlists("owner")
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Invalid(_)));
    }

    #[tokio::test]
    async fn invalid_ids_never_reach_the_network() {
        let server = MockServer::start().await;
        let c = client(&server.uri(), Arc::default());
        assert!(c.playlist("../me").await.is_err());
        assert!(c.add_items("a/b", &[]).await.is_err());
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn pick_image_prefers_the_smallest_sharp_one() {
        let img = |url: &str, width: Option<u32>| Image {
            url: url.to_string(),
            width,
        };
        let mosaic = [
            img("640", Some(640)),
            img("300", Some(300)),
            img("60", Some(60)),
        ];
        assert_eq!(pick_image(&mosaic).as_deref(), Some("300"));
        assert_eq!(
            pick_image(&[img("uploaded", None)]).as_deref(),
            Some("uploaded")
        );
        assert_eq!(pick_image(&[img("60", Some(60))]).as_deref(), Some("60"));
        assert_eq!(pick_image(&[]), None);
    }

    #[test]
    fn error_message_prefers_spotify_text() {
        let body = br#"{"error":{"status":400,"message":"Bad uri"}}"#;
        assert_eq!(error_message(StatusCode::BAD_REQUEST, body), "Bad uri");
        assert_eq!(
            error_message(StatusCode::BAD_REQUEST, b"oops"),
            "Bad Request"
        );
    }
}
