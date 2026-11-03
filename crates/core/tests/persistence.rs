use m3u8_core::{filesystem, media, store::Store};

#[test]
fn new_versions_are_never_reinitialized() {
    let dir = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("tasks.sqlite")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    assert_eq!(
        Store::open(dir.path()).err().unwrap().code,
        "MIGRATION_FAILED"
    );
    let db = rusqlite::Connection::open(dir.path().join("tasks.sqlite")).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        99
    );
}

#[test]
fn config_replacement_never_exposes_a_missing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    filesystem::atomic_write(&file, b"old").unwrap();
    filesystem::atomic_write(&file, b"new").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"new");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn task_paths_and_output_names_cannot_escape() {
    let root = tempfile::tempdir().unwrap();
    assert!(filesystem::task_cache(root.path(), "../outside").is_err());
    assert_eq!(media::safe_name("CON.txt"), "_CON.txt");
    assert!(!media::safe_name("../untrusted\\path").contains(['/', '\\']));
}

#[test]
fn iv_only_override_requires_reentry_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let engine = m3u8_core::Engine::open(root.path()).unwrap();
    let mut input = m3u8_core::TaskInput {
        source: "https://example.invalid/index.m3u8".into(),
        output_dir: root.path().join("output").to_string_lossy().into_owned(),
        ..Default::default()
    };
    input
        .manual_ivs
        .insert("0".into(), "00000000000000000000000000000064".into());
    let task = engine.add(input).unwrap();
    drop(engine);
    let engine = m3u8_core::Engine::open(root.path()).unwrap();
    assert!(engine.list()[0].needs_credentials);
    let mut restored = engine.input(&task.id).unwrap().unwrap();
    restored.confirm_gaps = true;
    engine.update_options(&task.id, restored, false).unwrap();
    assert_eq!(
        engine.start(&task.id).unwrap_err().code,
        "CREDENTIAL_REQUIRED"
    );
}

#[test]
fn concurrent_segment_receipts_are_durable_on_reopen() {
    use m3u8_core::store::SegmentRecord;
    use std::sync::{Arc, Barrier};
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(root.path()).unwrap());
    let barrier = Arc::new(Barrier::new(8));
    let workers = (0..8)
        .map(|track| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for index in 0..16 {
                    let record = SegmentRecord {
                        track,
                        index,
                        identity: "identity".into(),
                        bytes: 188,
                        digest: "digest".into(),
                        file: format!("{track}-{index}.ts"),
                    };
                    store.commit_segment("task", &record).unwrap();
                    assert_eq!(
                        store.segment("task", track, index).unwrap().unwrap().bytes,
                        188
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    drop(store);
    let store = Store::open(root.path()).unwrap();
    for track in 0..8 {
        for index in 0..16 {
            assert!(store.segment("task", track, index).unwrap().is_some());
        }
    }
}
