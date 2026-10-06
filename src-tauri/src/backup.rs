use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Backups kept per playlist; older ones are deleted after each write.
pub const KEEP: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub playlist_id: String,
    pub playlist_name: String,
    pub snapshot_id: String,
    /// Epoch milliseconds; also the file name, so it doubles as the backup ID.
    pub created_at: u64,
    pub uris: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub id: String,
    pub playlist_name: String,
    pub created_at: u64,
    pub track_count: usize,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Playlist and backup IDs arrive over IPC; only plain alphanumerics may become path segments.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn playlist_dir(root: &Path, playlist_id: &str) -> Result<PathBuf, String> {
    if !is_valid_id(playlist_id) {
        return Err(format!("Invalid playlist ID: {playlist_id}"));
    }
    Ok(root.join(playlist_id))
}

/// Backup IDs in `dir`, newest first.
fn ids_newest_first(dir: &Path) -> Result<Vec<u64>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut ids: Vec<u64> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".json")?.parse().ok()
        })
        .collect();
    ids.sort_unstable_by(|a, b| b.cmp(a));
    Ok(ids)
}

/// Saves `backup` under `<root>/<playlist_id>/<created_at>.json` and prunes old ones.
/// Returns the backup ID.
pub fn write(root: &Path, backup: &Backup) -> Result<String, String> {
    let dir = playlist_dir(root, &backup.playlist_id)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create backup folder: {e}"))?;

    // Two backups in the same millisecond would collide; bump until the name is free.
    let mut backup = backup.clone();
    while dir.join(format!("{}.json", backup.created_at)).exists() {
        backup.created_at += 1;
    }
    let json = serde_json::to_vec_pretty(&backup).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", backup.created_at)), json)
        .map_err(|e| format!("Couldn't write backup: {e}"))?;

    for old in ids_newest_first(&dir)?.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(dir.join(format!("{old}.json")));
    }
    Ok(backup.created_at.to_string())
}

pub fn list(root: &Path, playlist_id: &str) -> Result<Vec<BackupSummary>, String> {
    let dir = playlist_dir(root, playlist_id)?;
    let summaries = ids_newest_first(&dir)?
        .into_iter()
        .filter_map(|id| load(root, playlist_id, &id.to_string()).ok())
        .map(|b| BackupSummary {
            id: b.created_at.to_string(),
            playlist_name: b.playlist_name,
            created_at: b.created_at,
            track_count: b.uris.len(),
        })
        .collect();
    Ok(summaries)
}

pub fn load(root: &Path, playlist_id: &str, backup_id: &str) -> Result<Backup, String> {
    let dir = playlist_dir(root, playlist_id)?;
    if !is_valid_id(backup_id) {
        return Err(format!("Invalid backup ID: {backup_id}"));
    }
    let bytes = std::fs::read(dir.join(format!("{backup_id}.json")))
        .map_err(|e| format!("Couldn't read backup: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Backup file is damaged: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYLIST: &str = "37i9dQZF1DXcBWIGoYBM5M";

    fn sample(created_at: u64) -> Backup {
        Backup {
            playlist_id: PLAYLIST.to_string(),
            playlist_name: "My Mix".to_string(),
            snapshot_id: "snap".to_string(),
            created_at,
            uris: vec!["spotify:track:a".to_string(), "spotify:track:b".to_string()],
        }
    }

    #[test]
    fn write_list_and_load_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let id = write(root.path(), &sample(1_000)).unwrap();
        assert_eq!(id, "1000");

        let listed = list(root.path(), PLAYLIST).unwrap();
        assert_eq!(
            listed,
            vec![BackupSummary {
                id: "1000".to_string(),
                playlist_name: "My Mix".to_string(),
                created_at: 1_000,
                track_count: 2,
            }]
        );
        assert_eq!(load(root.path(), PLAYLIST, &id).unwrap(), sample(1_000));
    }

    #[test]
    fn list_is_newest_first_and_empty_for_unknown_playlist() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &sample(1)).unwrap();
        write(root.path(), &sample(3)).unwrap();
        write(root.path(), &sample(2)).unwrap();
        let ids: Vec<String> = list(root.path(), PLAYLIST)
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(ids, ["3", "2", "1"]);
        assert!(list(root.path(), "unknown").unwrap().is_empty());
    }

    #[test]
    fn same_millisecond_backups_do_not_overwrite_each_other() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(write(root.path(), &sample(5)).unwrap(), "5");
        assert_eq!(write(root.path(), &sample(5)).unwrap(), "6");
        assert_eq!(list(root.path(), PLAYLIST).unwrap().len(), 2);
    }

    #[test]
    fn pruning_keeps_the_newest_ten() {
        let root = tempfile::tempdir().unwrap();
        for t in 1..=15 {
            write(root.path(), &sample(t)).unwrap();
        }
        let listed = list(root.path(), PLAYLIST).unwrap();
        assert_eq!(listed.len(), KEEP);
        assert_eq!(listed.first().unwrap().created_at, 15);
        assert_eq!(listed.last().unwrap().created_at, 6);
    }

    #[test]
    fn traversal_style_ids_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        assert!(list(root.path(), "../etc").is_err());
        assert!(load(root.path(), PLAYLIST, "../../secret").is_err());
        assert!(load(root.path(), "..", "1").is_err());
        let mut bad = sample(1);
        bad.playlist_id = "a/b".to_string();
        assert!(write(root.path(), &bad).is_err());
    }

    #[test]
    fn id_validation() {
        assert!(is_valid_id(PLAYLIST));
        assert!(is_valid_id("1700000000000"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("a.b"));
        assert!(!is_valid_id(&"a".repeat(65)));
    }
}
