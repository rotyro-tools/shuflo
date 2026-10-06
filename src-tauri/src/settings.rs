use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub playlist_id: Option<String>,
    #[serde(default)]
    pub launch_at_login: bool,
    /// Client ID of the user's Spotify app, entered during setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

pub const STORE_FILE: &str = "shuflo.json";
const SETTINGS_KEY: &str = "settings";
// Kept apart from `settings` so saving settings never overwrites it.
const LAST_SHUFFLED_KEY: &str = "lastShuffled";

pub fn load(app: &AppHandle) -> Settings {
    app.store(STORE_FILE)
        .ok()
        .and_then(|store| {
            store
                .get(SETTINGS_KEY)
                .and_then(|v| serde_json::from_value(v).ok())
        })
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(dir);
    }
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(
        SETTINGS_KEY,
        serde_json::to_value(settings).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())
}

fn load_last_shuffled(app: &AppHandle) -> HashMap<String, u64> {
    app.store(STORE_FILE)
        .ok()
        .and_then(|store| {
            store
                .get(LAST_SHUFFLED_KEY)
                .and_then(|v| serde_json::from_value(v).ok())
        })
        .unwrap_or_default()
}

/// Epoch milliseconds of the last successful shuffle of `playlist_id`.
pub fn last_shuffled(app: &AppHandle, playlist_id: &str) -> Option<u64> {
    load_last_shuffled(app).get(playlist_id).copied()
}

pub fn set_last_shuffled(app: &AppHandle, playlist_id: &str, at_ms: u64) -> Result<(), String> {
    let mut map = load_last_shuffled(app);
    map.insert(playlist_id.to_string(), at_ms);
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(
        LAST_SHUFFLED_KEY,
        serde_json::to_value(map).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())
}

/// Spotify Client IDs are 32 hex characters.
pub fn is_valid_client_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn client_id(app: &AppHandle) -> Result<String, String> {
    load(app)
        .client_id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "No Spotify Client ID is set yet.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_have_no_playlist() {
        let s = Settings::default();
        assert!(s.playlist_id.is_none());
        assert!(!s.launch_at_login);
        assert!(s.client_id.is_none());
    }

    #[test]
    fn settings_serde_round_trip() {
        let original = Settings {
            playlist_id: Some("37i9dQZF1DXcBWIGoYBM5M".to_string()),
            launch_at_login: true,
            client_id: Some("abc123".to_string()),
        };
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(json["playlistId"], "37i9dQZF1DXcBWIGoYBM5M");
        let restored: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(restored.playlist_id, original.playlist_id);
        assert_eq!(restored.launch_at_login, original.launch_at_login);
        assert_eq!(restored.client_id, original.client_id);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let restored: Settings = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(restored.playlist_id.is_none());
        assert!(!restored.launch_at_login);
    }

    #[test]
    fn client_id_validation() {
        assert!(is_valid_client_id("0123456789abcdef0123456789ABCDEF"));
        assert!(!is_valid_client_id(""));
        assert!(!is_valid_client_id("0123456789abcdef0123456789abcde"));
        assert!(!is_valid_client_id("0123456789abcdef0123456789abcdeg"));
    }
}
