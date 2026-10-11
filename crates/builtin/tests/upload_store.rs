//! Port of `tests/unit/builtin_upload_store.spec.ts`.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::{StreamExt, stream};
use mymcps_builtin::upload_store::{
    BUILTIN_UPLOAD_MINUTES, BuiltinUpload, BuiltinUploadTarget, UploadError, UploadRefusal,
    UploadStore,
};

const MCP: i64 = 7;
const HOUR_MS: i64 = BUILTIN_UPLOAD_MINUTES * 60_000;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn target(max_bytes: u64) -> BuiltinUploadTarget {
    BuiltinUploadTarget {
        id: uuid(),
        filename: "report.pdf".into(),
        content_type: Some("application/pdf".into()),
        max_bytes,
    }
}

fn body(pieces: &[&str]) -> impl futures::Stream<Item = Result<Bytes, Infallible>> + use<> {
    stream::iter(
        pieces
            .iter()
            .map(|piece| Ok(Bytes::from(piece.to_string())))
            .collect::<Vec<_>>(),
    )
}

fn refusal(result: Result<BuiltinUpload, UploadError>) -> Option<UploadRefusal> {
    match result {
        Ok(_) => None,
        Err(UploadError::Refused(reason)) => Some(reason),
        Err(other) => panic!("unexpected error: {other}"),
    }
}

struct Fixture {
    store: UploadStore,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            store: UploadStore::at(dir.path().join("builtin-uploads")),
            _dir: dir,
        }
    }

    fn directory(&self, mcp_id: i64) -> PathBuf {
        self.store.root().join(mcp_id.to_string())
    }

    fn stored(&self, mcp_id: i64) -> Option<Vec<String>> {
        let mut names: Vec<String> = std::fs::read_dir(self.directory(mcp_id))
            .ok()?
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        Some(names)
    }

    async fn text(&self, mcp_id: i64, id: &str) -> Option<String> {
        self.store
            .read(mcp_id, id)
            .await
            .unwrap()
            .map(|content| String::from_utf8(content).unwrap())
    }

    /// A file as a finished upload leaves it, without writing its bytes.
    fn waiting(&self, mcp_id: i64, size: u64, expires_at: i64) -> String {
        let id = uuid();
        let path = self.directory(mcp_id).join(&id);
        std::fs::create_dir_all(self.directory(mcp_id)).unwrap();
        std::fs::File::create(&path).unwrap().set_len(size).unwrap();
        let metadata = serde_json::json!({ "id": id, "filename": "big.bin", "size": size, "expiresAt": expires_at });
        std::fs::write(format!("{}.json", path.display()), metadata.to_string()).unwrap();
        id
    }
}

#[tokio::test]
async fn keeps_the_file_a_link_was_made_for_and_gives_it_back() {
    let fixture = Fixture::new();
    let file = target(1_000);
    let before = now_ms();
    let upload = fixture
        .store
        .save(MCP, &file, body(&["%PDF-", "1.4\n", "%%EOF\n"]))
        .await
        .unwrap();

    assert_eq!(
        BuiltinUpload {
            expires_at: 0,
            ..upload.clone()
        },
        BuiltinUpload {
            id: file.id.clone(),
            filename: "report.pdf".into(),
            content_type: Some("application/pdf".into()),
            size: 15,
            expires_at: 0,
        }
    );
    assert!(upload.expires_at >= before + HOUR_MS);
    assert!(upload.expires_at <= now_ms() + HOUR_MS);

    assert_eq!(
        fixture.store.find(MCP, &file.id).await.unwrap(),
        Some(upload)
    );
    assert_eq!(
        fixture.text(MCP, &file.id).await.as_deref(),
        Some("%PDF-1.4\n%%EOF\n")
    );
    assert_eq!(
        fixture.stored(MCP).unwrap(),
        [file.id.clone(), format!("{}.json", file.id)]
    );
    // The metadata is what the Node app wrote, so files survive the upgrade.
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.directory(MCP).join(format!("{}.json", file.id))).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["contentType"], "application/pdf");
    assert!(metadata["expiresAt"].is_i64());

    // Uploads belong to one MCP.
    assert_eq!(fixture.store.find(MCP + 1, &file.id).await.unwrap(), None);
    assert_eq!(fixture.store.read(MCP + 1, &file.id).await.unwrap(), None);
    assert_eq!(fixture.store.find(MCP, &uuid()).await.unwrap(), None);
}

#[tokio::test]
async fn takes_one_file_for_each_link_also_while_the_first_is_arriving() {
    let fixture = Fixture::new();
    let file = target(1_000);
    let (arrived, wait) = tokio::sync::oneshot::channel::<()>();
    let slow =
        stream::iter([Ok::<_, Infallible>(Bytes::from("first"))]).chain(stream::once(async move {
            let _ = wait.await;
            Ok(Bytes::new())
        }));

    let store = fixture.store.clone();
    let slow_target = file.clone();
    let first = tokio::spawn(async move { store.save(MCP, &slow_target, slow).await });
    while fixture.stored(MCP).is_none_or(|names| names.is_empty()) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Nothing can be attached before the file is whole.
    assert_eq!(fixture.store.find(MCP, &file.id).await.unwrap(), None);
    assert_eq!(
        refusal(fixture.store.save(MCP, &file, body(&["second"])).await),
        Some(UploadRefusal::Taken)
    );

    arrived.send(()).unwrap();
    first.await.unwrap().unwrap();
    assert_eq!(
        refusal(fixture.store.save(MCP, &file, body(&["third"])).await),
        Some(UploadRefusal::Taken)
    );
    assert_eq!(fixture.text(MCP, &file.id).await.as_deref(), Some("first"));
}

#[tokio::test]
async fn keeps_nothing_of_a_file_that_is_empty_or_too_large_so_the_link_can_be_tried_again() {
    let fixture = Fixture::new();
    let file = target(10);

    assert_eq!(
        refusal(fixture.store.save(MCP, &file, body(&[])).await),
        Some(UploadRefusal::Empty)
    );
    assert_eq!(
        refusal(
            fixture
                .store
                .save(MCP, &file, body(&["12345", "678901"]))
                .await
        ),
        Some(UploadRefusal::TooLarge)
    );
    assert_eq!(fixture.stored(MCP).unwrap(), Vec::<String>::new());
    assert_eq!(fixture.store.find(MCP, &file.id).await.unwrap(), None);

    // A body that breaks off is not a file either.
    let broken = stream::iter([Ok(Bytes::from("1234")), Err("aborted")]);
    let error = fixture.store.save(MCP, &file, broken).await.unwrap_err();
    assert_eq!(error.to_string(), "aborted");
    assert_eq!(fixture.stored(MCP).unwrap(), Vec::<String>::new());

    let upload = fixture
        .store
        .save(MCP, &file, body(&["1234567890"]))
        .await
        .unwrap();
    assert_eq!(upload.size, 10);
}

#[tokio::test]
async fn only_stores_under_a_uuid_for_a_saved_mcp() {
    let fixture = Fixture::new();
    for id in [
        "../escape".to_string(),
        "..".into(),
        "report.pdf".into(),
        String::new(),
        format!("{}.json", uuid()),
        "A".repeat(36),
    ] {
        let named = BuiltinUploadTarget {
            id: id.clone(),
            ..target(1_000)
        };
        assert!(
            matches!(
                fixture.store.save(MCP, &named, body(&["x"])).await,
                Err(UploadError::NotAnUploadId)
            ),
            "{id}"
        );
        assert!(matches!(
            fixture.store.find(MCP, &id).await,
            Err(UploadError::NotAnUploadId)
        ));
        assert!(matches!(
            fixture.store.read(MCP, &id).await,
            Err(UploadError::NotAnUploadId)
        ));
    }
    assert_eq!(UploadError::NotAnUploadId.to_string(), "Not an upload id");
    for mcp_id in [0, -1] {
        assert!(matches!(
            fixture
                .store
                .save(mcp_id, &target(1_000), body(&["x"]))
                .await,
            Err(UploadError::UnsavedMcp)
        ));
        assert!(matches!(
            fixture.store.remove_all(mcp_id).await,
            Err(UploadError::UnsavedMcp)
        ));
    }
    assert_eq!(fixture.stored(MCP), None);
}

#[tokio::test]
async fn holds_at_most_50_files_and_100_mb_for_an_mcp() {
    let fixture = Fixture::new();
    for _ in 0..49 {
        fixture.waiting(MCP, 1, now_ms() + HOUR_MS);
    }
    fixture
        .store
        .save(MCP, &target(1_000), body(&["fiftieth"]))
        .await
        .unwrap();
    assert_eq!(
        refusal(
            fixture
                .store
                .save(MCP, &target(1_000), body(&["one more"]))
                .await
        ),
        Some(UploadRefusal::Full)
    );
    // Another MCP has its own room.
    fixture
        .store
        .save(MCP + 1, &target(1_000), body(&["other"]))
        .await
        .unwrap();

    std::fs::remove_dir_all(fixture.store.root()).unwrap();
    for _ in 0..3 {
        fixture.waiting(MCP, 25_000_000, now_ms() + HOUR_MS);
    }
    fixture.waiting(MCP, 24_999_990, now_ms() + HOUR_MS);
    // 10 bytes of room are left: a file is cut off at the first byte too many.
    let large = target(20_000_000);
    assert_eq!(
        refusal(
            fixture
                .store
                .save(MCP, &large, body(&["12345678901"]))
                .await
        ),
        Some(UploadRefusal::Full)
    );
    fixture
        .store
        .save(MCP, &large, body(&["1234567890"]))
        .await
        .unwrap();
    assert_eq!(
        refusal(fixture.store.save(MCP, &target(1_000), body(&["x"])).await),
        Some(UploadRefusal::Full)
    );
}

#[tokio::test]
async fn stops_giving_a_file_back_an_hour_after_its_upload_and_deletes_it() {
    let fixture = Fixture::new();
    let expired = fixture.waiting(MCP, 5, now_ms() - 1);
    let fresh = fixture.waiting(MCP, 5, now_ms() + HOUR_MS);

    assert_eq!(fixture.store.find(MCP, &expired).await.unwrap(), None);
    assert!(fixture.store.find(MCP, &fresh).await.unwrap().is_some());
    let mut expected = vec![fresh.clone(), format!("{fresh}.json")];
    expected.sort();
    assert_eq!(fixture.stored(MCP).unwrap(), expected);

    // An expired file makes room for the next upload without being asked for.
    let other = fixture.waiting(MCP, 5, now_ms() - 1);
    fixture
        .store
        .save(MCP, &target(1_000), body(&["new"]))
        .await
        .unwrap();
    assert!(!fixture.stored(MCP).unwrap().contains(&other));
    assert_eq!(fixture.stored(MCP).unwrap().len(), 4);
}

#[tokio::test]
async fn prunes_expired_files_abandoned_uploads_and_the_directories_left_empty() {
    let fixture = Fixture::new();
    let kept = fixture.waiting(MCP, 5, now_ms() + HOUR_MS);
    let expired = fixture.waiting(MCP, 5, now_ms() - 1);
    fixture.waiting(MCP + 1, 5, now_ms() - 1);
    // Uploads that never finished: one still arriving, one left by a server that stopped.
    let arriving = uuid();
    let abandoned = uuid();
    std::fs::write(fixture.directory(MCP).join(&arriving), "half").unwrap();
    std::fs::write(fixture.directory(MCP).join(&abandoned), "half").unwrap();
    let long_ago = SystemTime::now() - Duration::from_millis(HOUR_MS as u64 + 1_000);
    std::fs::File::options()
        .write(true)
        .open(fixture.directory(MCP).join(&abandoned))
        .unwrap()
        .set_modified(long_ago)
        .unwrap();
    // Not ours to delete.
    std::fs::create_dir_all(fixture.store.root().join("notes")).unwrap();
    std::fs::write(fixture.directory(MCP).join("README"), "left alone").unwrap();

    fixture.store.prune().await.unwrap();

    let mut expected = vec![
        "README".to_string(),
        arriving,
        kept.clone(),
        format!("{kept}.json"),
    ];
    expected.sort();
    assert_eq!(fixture.stored(MCP).unwrap(), expected);
    assert!(!fixture.stored(MCP).unwrap().contains(&expired));
    assert_eq!(fixture.stored(MCP + 1), None);
    let mut roots: Vec<String> = std::fs::read_dir(fixture.store.root())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    roots.sort();
    assert_eq!(roots, [MCP.to_string(), "notes".to_string()]);

    // An hour later, everything of ours has expired.
    fixture
        .store
        .prune_at(now_ms() + HOUR_MS + 1_000)
        .await
        .unwrap();
    assert_eq!(fixture.stored(MCP).unwrap(), ["README"]);
    assert_eq!(
        std::fs::read_to_string(fixture.directory(MCP).join("README")).unwrap(),
        "left alone"
    );
}

#[tokio::test]
async fn sweeps_on_start_for_what_a_previous_run_left_and_deletes_the_files_of_an_mcp_with_it() {
    let fixture = Fixture::new();
    fixture.waiting(MCP, 5, now_ms() - 1);
    let kept = fixture.waiting(MCP, 5, now_ms() + HOUR_MS);

    let sweeper = Arc::new(fixture.store.clone()).start_sweeper(Duration::from_secs(60));
    while fixture.stored(MCP).unwrap().len() > 2 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    sweeper.abort();
    let mut expected = vec![kept.clone(), format!("{kept}.json")];
    expected.sort();
    assert_eq!(fixture.stored(MCP).unwrap(), expected);

    fixture.store.remove_all(MCP).await.unwrap();
    assert_eq!(fixture.stored(MCP), None);
    fixture.store.remove_all(MCP).await.unwrap();
}
