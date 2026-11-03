use fs2::FileExt;
use sshm::{
    model::{Connection, Profile},
    store::Store,
};
use std::{fs, fs::OpenOptions};

fn sample(name: &str) -> Profile {
    Profile::new(
        name.into(),
        Connection::Manual {
            host: "example.com".into(),
            user: "user".into(),
            port: 22,
            identity_file: None,
        },
    )
}
fn temporary_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        directory: dir.path().join("sshm"),
    };
    (dir, store)
}

#[test]
fn read_only_empty_store_does_not_create_files() {
    let (_dir, store) = temporary_store();
    assert!(store.load().unwrap().is_empty());
    assert!(!store.directory.exists());
}

#[test]
fn saves_edits_deletes_and_matches_full_names_only() {
    let (_dir, store) = temporary_store();
    let mut p = store.save(sample("开发 Prod"), None).unwrap();
    assert_eq!(store.find("开发 prod").unwrap(), p);
    assert!(store.find("开发").is_err());
    assert!(store.save(sample("开发 PROD"), None).is_err());
    p.name = "renamed".into();
    let edited = store.save(p.clone(), Some(p.revision)).unwrap();
    assert_eq!(edited.revision, 2);
    assert!(store.find("开发 Prod").is_err());
    store.remove(&edited).unwrap();
    assert!(store.load().unwrap().is_empty());
}

#[test]
fn stale_writes_never_overwrite_another_windows_changes() {
    let (_dir, store) = temporary_store();
    let original = store.save(sample("dev"), None).unwrap();
    let mut other = original.clone();
    other.tags.push("new tag".into());
    store.save(other, Some(original.revision)).unwrap();
    assert!(
        store
            .save(original.clone(), Some(original.revision))
            .is_err()
    );
    assert!(store.remove(&original).is_err());
    assert_eq!(store.find("dev").unwrap().tags, ["new tag"]);
}

#[test]
fn usage_updates_do_not_invalidate_edits_or_get_lost() {
    let (_dir, store) = temporary_store();
    let original = store.save(sample("dev"), None).unwrap();
    store.mark_used(original.id, 123).unwrap();
    let saved = store
        .save(original.clone(), Some(original.revision))
        .unwrap();
    assert_eq!(saved.last_used, Some(123));
    store.mark_used(original.id, 100).unwrap();
    assert_eq!(store.find("dev").unwrap().last_used, Some(123));
}

#[test]
fn sort_is_recent_first_then_case_insensitive_name() {
    let (_dir, store) = temporary_store();
    store.save(sample("zulu"), None).unwrap();
    store.save(sample("Alpha"), None).unwrap();
    let p = store.save(sample("used"), None).unwrap();
    store.mark_used(p.id, 10).unwrap();
    assert_eq!(
        store
            .load()
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["used", "Alpha", "zulu"]
    );
}

#[test]
fn corrupt_or_future_data_remains_untouched() {
    let (_dir, store) = temporary_store();
    fs::create_dir_all(&store.directory).unwrap();
    for data in ["{bad json", r#"{"version":999,"profiles":[]}"#] {
        fs::write(store.path(), data).unwrap();
        assert!(store.save(sample("dev"), None).is_err());
        assert_eq!(fs::read_to_string(store.path()).unwrap(), data);
    }
}

#[test]
fn concurrent_writer_is_rejected_without_truncation() {
    let (_dir, store) = temporary_store();
    store.save(sample("old"), None).unwrap();
    let before = fs::read(store.path()).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.directory.join("connections.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    assert!(store.save(sample("new"), None).is_err());
    assert_eq!(fs::read(store.path()).unwrap(), before);
}

#[test]
fn reimport_is_deduplicated_by_source_even_after_renaming() {
    let (_dir, store) = temporary_store();
    let path = store.directory.join("original-config");
    let a = Profile::new(
        "alias".into(),
        Connection::SshConfig {
            path: path.clone(),
            alias: "alias".into(),
        },
    );
    assert_eq!(store.import(vec![a.clone()]).unwrap().added, 1);
    let mut old = store.find("alias").unwrap();
    old.name = "renamed".into();
    store.save(old.clone(), Some(old.revision)).unwrap();
    let b = Profile::new(
        "renamed".into(),
        Connection::SshConfig {
            path,
            alias: "different".into(),
        },
    );
    let result = store.import(vec![a, b]).unwrap();
    assert_eq!(result.added, 0);
    assert_eq!(result.skipped.len(), 2);
    assert_eq!(store.load().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn configuration_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, store) = temporary_store();
    store.save(sample("dev"), None).unwrap();
    assert_eq!(
        fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
