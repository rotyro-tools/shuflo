//! Fast mode: rewrite the whole playlist in chunks of 100 (about 30 requests for
//! 1,000 tracks). Every track's "date added" becomes the time of the shuffle.

use std::path::Path;
use std::time::Duration;

use rand::rngs::{StdRng, SysRng};
use rand::SeedableRng;
use serde::Serialize;

use super::core::{fisher_yates, write_plan, WriteOp};
use crate::backup::{self, Backup};
use crate::spotify::api::{ApiError, PlaylistInfo, SpotifyClient, WRITE_CHUNK_SIZE};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Reading,
    Writing,
    Restoring,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

pub type ProgressFn<'a> = &'a (dyn Fn(Progress) + Send + Sync);

#[derive(Debug, Clone)]
pub struct Outcome {
    pub info: PlaylistInfo,
}

struct Current {
    info: PlaylistInfo,
    uris: Vec<String>,
}

/// Reads the playlist and checks it can be rewritten without losing anything.
async fn read_current(
    client: &SpotifyClient,
    id: &str,
    user_id: &str,
    progress: ProgressFn<'_>,
) -> Result<Current, String> {
    let info = client.playlist(id).await?;
    if !info.editable_by(user_id) {
        return Err(ApiError::Forbidden(String::new()).into());
    }
    let entries = client
        .playlist_entries(id, |done, total| {
            progress(Progress {
                phase: Phase::Reading,
                done,
                total,
            })
        })
        .await?;

    // Local files and tracks Spotify no longer has can't be added back, so a rewrite would drop them.
    let unusable = entries
        .iter()
        .filter(|e| e.is_local || e.uri.is_none())
        .count();
    if unusable > 0 {
        let what = if unusable == 1 {
            "1 local or unavailable track".to_string()
        } else {
            format!("{unusable} local or unavailable tracks")
        };
        return Err(format!(
            "This playlist has {what}. Shuffling would remove them, so Shuflo stopped. \
             Remove them in Spotify, then try again."
        ));
    }
    let uris: Vec<String> = entries.into_iter().filter_map(|e| e.uri).collect();
    if uris.is_empty() {
        return Err("The playlist is empty.".to_string());
    }
    Ok(Current { info, uris })
}

/// Writes `uris` as the playlist's full contents. On failure, also reports whether
/// anything was written (a failed first replace leaves the playlist untouched).
async fn write_all(
    client: &SpotifyClient,
    id: &str,
    uris: &[String],
    phase: Phase,
    progress: ProgressFn<'_>,
) -> Result<(), (ApiError, bool)> {
    let plan = write_plan(uris.len(), WRITE_CHUNK_SIZE).map_err(|e| (ApiError::Other(e), false))?;
    let total = uris.len();
    progress(Progress {
        phase,
        done: 0,
        total,
    });
    let mut wrote_any = false;
    for op in plan {
        let range = match op {
            WriteOp::Replace(r) => {
                client
                    .replace_items(id, &uris[r.clone()])
                    .await
                    .map_err(|e| (e, wrote_any))?;
                r
            }
            WriteOp::Append(r) => {
                client
                    .add_items(id, &uris[r.clone()])
                    .await
                    .map_err(|e| (e, wrote_any))?;
                r
            }
        };
        wrote_any = true;
        progress(Progress {
            phase,
            done: range.end,
            total,
        });
    }
    Ok(())
}

/// Replaces the playlist's order with `target`, putting `current` back if a write fails.
pub async fn apply_order(
    client: &SpotifyClient,
    id: &str,
    current: &[String],
    target: &[String],
    progress: ProgressFn<'_>,
) -> Result<(), String> {
    let (error, wrote_any) = match write_all(client, id, target, Phase::Writing, progress).await {
        Ok(()) => return Ok(()),
        Err(failure) => failure,
    };
    if !wrote_any {
        return Err(error.to_string());
    }
    match write_all(client, id, current, Phase::Restoring, progress).await {
        Ok(()) => Err(format!(
            "{error} Shuflo put the playlist back the way it was."
        )),
        Err((rollback_error, _)) => Err(format!(
            "{error} Putting the playlist back also failed ({rollback_error}). \
             Use Settings → Restore previous order."
        )),
    }
}

/// How long to wait before checking the track count a second time, in case Spotify
/// hasn't caught up with the last write yet.
const VERIFY_RETRY_DELAY: Duration = Duration::from_millis(800);

/// Stops if the playlist changed since it was read: someone else (another collaborator,
/// or a second Shuflo) is editing it, and writing now would overwrite their change.
async fn ensure_unchanged(client: &SpotifyClient, id: &str, read: &Current) -> Result<(), String> {
    let now = client.playlist(id).await?;
    if now.snapshot_id != read.info.snapshot_id {
        return Err(
            "The playlist changed while Shuflo was reading it, so nothing was written. \
             Try again."
                .to_string(),
        );
    }
    Ok(())
}

/// Replace and append calls can't be tied to a playlist version, so a concurrent
/// writer shows up as a wrong track count afterwards.
async fn verify_written(client: &SpotifyClient, id: &str, expected: usize) -> Result<(), String> {
    for attempt in 0..2 {
        if client.playlist(id).await?.total == expected {
            return Ok(());
        }
        if attempt == 0 {
            tokio::time::sleep(VERIFY_RETRY_DELAY).await;
        }
    }
    Err(
        "Someone else changed the playlist while Shuflo was writing it. Check it in Spotify, \
         or use Settings → Restore previous order."
            .to_string(),
    )
}

fn backup_of(id: &str, current: &Current) -> Backup {
    Backup {
        playlist_id: id.to_string(),
        playlist_name: current.info.name.clone(),
        snapshot_id: current.info.snapshot_id.clone(),
        created_at: backup::now_ms(),
        uris: current.uris.clone(),
    }
}

/// Backs up the playlist, then rewrites it in a uniformly random order.
pub async fn shuffle(
    client: &SpotifyClient,
    id: &str,
    user_id: &str,
    backups_root: &Path,
    progress: ProgressFn<'_>,
) -> Result<Outcome, String> {
    let current = read_current(client, id, user_id, progress).await?;
    ensure_unchanged(client, id, &current).await?;
    backup::write(backups_root, &backup_of(id, &current))?;

    let mut target = current.uris.clone();
    // ChaCha12 seeded from the OS random source for this run.
    let mut rng =
        StdRng::try_from_rng(&mut SysRng).map_err(|e| format!("OS random source failed: {e}"))?;
    fisher_yates(&mut target, &mut rng);

    apply_order(client, id, &current.uris, &target, progress).await?;
    verify_written(client, id, target.len()).await?;
    Ok(Outcome { info: current.info })
}

/// Backs up the current order, then rewrites the playlist with a saved backup.
pub async fn restore(
    client: &SpotifyClient,
    id: &str,
    user_id: &str,
    backups_root: &Path,
    saved: &Backup,
    progress: ProgressFn<'_>,
) -> Result<Outcome, String> {
    if saved.playlist_id != id {
        return Err("That backup belongs to a different playlist.".to_string());
    }
    let current = read_current(client, id, user_id, progress).await?;
    ensure_unchanged(client, id, &current).await?;
    backup::write(backups_root, &backup_of(id, &current))?;
    apply_order(client, id, &current.uris, &saved.uris, progress).await?;
    verify_written(client, id, saved.uris.len()).await?;
    Ok(Outcome { info: current.info })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::spotify::api::test_support::{client, FakeTokens};

    const P: &str = "37i9dQZF1DXcBWIGoYBM5M";
    const OWNER: &str = "owner";

    fn uris(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("spotify:track:t{i}")).collect()
    }

    fn info(snapshot: &str, total: usize) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "id": P, "name": "My Mix", "owner": { "id": OWNER },
            "collaborative": false, "snapshot_id": snapshot, "items": { "total": total }
        }))
    }

    /// Answers the playlist-info call with `first` for the first `times` calls, then `then`.
    async fn mount_info_sequence(
        server: &MockServer,
        times: u64,
        first: ResponseTemplate,
        then: ResponseTemplate,
    ) {
        Mock::given(method("GET"))
            .and(path(format!("/v1/playlists/{P}")))
            .respond_with(first)
            .up_to_n_times(times)
            .with_priority(1)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/v1/playlists/{P}")))
            .respond_with(then)
            .with_priority(2)
            .mount(server)
            .await;
    }

    async fn mount_playlist(server: &MockServer, uris: &[String], local_at: Option<usize>) {
        let base = format!("{}/v1", server.uri());
        Mock::given(method("GET"))
            .and(path(format!("/v1/playlists/{P}")))
            .respond_with(info("snap", uris.len()))
            .mount(server)
            .await;
        for (page, chunk) in uris.chunks(50).enumerate() {
            let offset = page * 50;
            let items: Vec<Value> = chunk
                .iter()
                .enumerate()
                .map(|(i, uri)| {
                    json!({ "is_local": local_at == Some(offset + i), "item": { "uri": uri } })
                })
                .collect();
            let next = if offset + 50 < uris.len() {
                json!(format!(
                    "{base}/playlists/{P}/items?offset={}&limit=50",
                    offset + 50
                ))
            } else {
                Value::Null
            };
            Mock::given(method("GET"))
                .and(path(format!("/v1/playlists/{P}/items")))
                .and(query_param("offset", offset.to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "items": items, "next": next, "total": uris.len()
                })))
                .mount(server)
                .await;
        }
    }

    fn ok_write() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "snapshot_id": "new" }))
    }

    async fn mount_writes(server: &MockServer) {
        for verb in ["PUT", "POST"] {
            Mock::given(method(verb))
                .and(path(format!("/v1/playlists/{P}/items")))
                .respond_with(ok_write())
                .with_priority(10)
                .mount(server)
                .await;
        }
    }

    /// Replays the received writes to get what the playlist would now contain.
    async fn final_playlist(server: &MockServer) -> (Vec<String>, usize, usize) {
        let mut list = Vec::new();
        let (mut puts, mut posts) = (0, 0);
        for req in server.received_requests().await.unwrap() {
            let verb = req.method.as_str().to_string();
            if verb != "PUT" && verb != "POST" {
                continue;
            }
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let chunk: Vec<String> = serde_json::from_value(body["uris"].clone()).unwrap();
            if verb == "PUT" {
                puts += 1;
                list = chunk;
            } else {
                posts += 1;
                list.extend(chunk);
            }
        }
        (list, puts, posts)
    }

    fn collect_progress() -> (Arc<Mutex<Vec<Progress>>>, impl Fn(Progress) + Send + Sync) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        (seen, move |p: Progress| sink.lock().unwrap().push(p))
    }

    #[tokio::test]
    async fn shuffles_1000_tracks_with_one_replace_and_nine_appends() {
        let server = MockServer::start().await;
        let original = uris(1_000);
        mount_playlist(&server, &original, None).await;
        mount_writes(&server).await;
        let backups = tempfile::tempdir().unwrap();
        let (seen, progress) = collect_progress();

        let outcome = shuffle(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            OWNER,
            backups.path(),
            &progress,
        )
        .await
        .unwrap();
        assert_eq!(outcome.info.name, "My Mix");
        assert_eq!(outcome.info.total, 1_000);

        let (after, puts, posts) = final_playlist(&server).await;
        assert_eq!((puts, posts), (1, 9));
        assert_ne!(after, original);
        let mut sorted = after.clone();
        sorted.sort();
        let mut expected = original.clone();
        expected.sort();
        assert_eq!(sorted, expected);

        let saved = backup::list(backups.path(), P).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            backup::load(backups.path(), P, &saved[0].id).unwrap().uris,
            original
        );

        let seen = seen.lock().unwrap();
        let last_read = seen.iter().rfind(|p| p.phase == Phase::Reading).unwrap();
        assert_eq!((last_read.done, last_read.total), (1_000, 1_000));
        let last = seen.last().unwrap();
        assert_eq!(
            (last.phase, last.done, last.total),
            (Phase::Writing, 1_000, 1_000)
        );
    }

    #[tokio::test]
    async fn failed_append_puts_the_original_order_back() {
        let server = MockServer::start().await;
        let original = uris(250);
        mount_playlist(&server, &original, None).await;
        // First append succeeds, the second fails on every retry, later writes succeed.
        Mock::given(method("POST"))
            .and(path(format!("/v1/playlists/{P}/items")))
            .respond_with(ok_write())
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/v1/playlists/{P}/items")))
            .respond_with(ResponseTemplate::new(502))
            .up_to_n_times(6)
            .with_priority(2)
            .mount(&server)
            .await;
        mount_writes(&server).await;
        let backups = tempfile::tempdir().unwrap();

        let err = shuffle(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            OWNER,
            backups.path(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("put the playlist back"), "{err}");

        let (after, puts, _) = final_playlist(&server).await;
        assert_eq!(puts, 2);
        assert_eq!(after, original);
    }

    #[tokio::test]
    async fn failed_first_replace_does_not_attempt_a_rollback() {
        let server = MockServer::start().await;
        mount_playlist(&server, &uris(10), None).await;
        Mock::given(method("PUT"))
            .and(path(format!("/v1/playlists/{P}/items")))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let backups = tempfile::tempdir().unwrap();

        let err = shuffle(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            OWNER,
            backups.path(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            "Only the playlist's owner or its collaborators can shuffle it."
        );
        let (_, puts, posts) = final_playlist(&server).await;
        assert_eq!((puts, posts), (1, 0));
    }

    #[tokio::test]
    async fn local_files_stop_the_shuffle_before_any_write() {
        let server = MockServer::start().await;
        mount_playlist(&server, &uris(120), Some(70)).await;
        mount_writes(&server).await;
        let backups = tempfile::tempdir().unwrap();

        let err = shuffle(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            OWNER,
            backups.path(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("1 local or unavailable track"), "{err}");
        let (_, puts, posts) = final_playlist(&server).await;
        assert_eq!((puts, posts), (0, 0));
        assert!(backup::list(backups.path(), P).unwrap().is_empty());
    }

    #[tokio::test]
    async fn playlists_owned_by_someone_else_are_refused() {
        let server = MockServer::start().await;
        mount_playlist(&server, &uris(5), None).await;
        let backups = tempfile::tempdir().unwrap();
        let err = shuffle(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            "someone-else",
            backups.path(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            "Only the playlist's owner or its collaborators can shuffle it."
        );
    }

    #[tokio::test]
    async fn an_edit_while_reading_stops_before_any_write() {
        let server = MockServer::start().await;
        let original = uris(120);
        // First read sees "snap"; by the pre-write check someone else has edited it.
        mount_info_sequence(&server, 1, info("snap", 120), info("edited", 121)).await;
        mount_playlist(&server, &original, None).await;
        mount_writes(&server).await;
        let backups = tempfile::tempdir().unwrap();

        let err = shuffle(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            OWNER,
            backups.path(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("changed while Shuflo was reading"), "{err}");
        let (_, puts, posts) = final_playlist(&server).await;
        assert_eq!((puts, posts), (0, 0));
        assert!(backup::list(backups.path(), P).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_concurrent_writer_is_reported_after_writing() {
        let server = MockServer::start().await;
        let original = uris(250);
        // Read and pre-write check agree; afterwards another writer's tracks show up.
        mount_info_sequence(&server, 2, info("snap", 250), info("other", 350)).await;
        mount_playlist(&server, &original, None).await;
        mount_writes(&server).await;
        let backups = tempfile::tempdir().unwrap();

        let err = shuffle(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            OWNER,
            backups.path(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("while Shuflo was writing"), "{err}");
        assert_eq!(backup::list(backups.path(), P).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn restore_writes_the_saved_order_and_backs_up_the_current_one() {
        let server = MockServer::start().await;
        let current = uris(150);
        mount_playlist(&server, &current, None).await;
        mount_writes(&server).await;
        let backups = tempfile::tempdir().unwrap();
        let mut saved_uris = current.clone();
        saved_uris.reverse();
        let saved = Backup {
            playlist_id: P.to_string(),
            playlist_name: "My Mix".to_string(),
            snapshot_id: "old".to_string(),
            created_at: 1,
            uris: saved_uris.clone(),
        };

        restore(
            &client(&server.uri(), Arc::new(FakeTokens::default())),
            P,
            OWNER,
            backups.path(),
            &saved,
            &|_| {},
        )
        .await
        .unwrap();

        let (after, _, _) = final_playlist(&server).await;
        assert_eq!(after, saved_uris);
        let listed = backup::list(backups.path(), P).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            backup::load(backups.path(), P, &listed[0].id).unwrap().uris,
            current
        );
    }
}
