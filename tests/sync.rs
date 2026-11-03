use sshm::{
    model::{Connection, Profile},
    store::Store,
    sync::model::{Batch, Change, Portable},
};
use std::{collections::BTreeSet, fs};
use uuid::Uuid;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        directory: dir.path().join("config"),
    };
    (dir, store)
}
fn profile(name: &str) -> Profile {
    Profile::new(
        name.into(),
        Connection::Manual {
            host: "host.test".into(),
            user: "tester".into(),
            port: 22,
            identity_file: None,
        },
    )
}
fn login(store: &Store) {
    store
        .sync_account("account", "test@example.test", false)
        .unwrap();
}
fn changes(store: &Store) -> Batch {
    store.sync_capture().unwrap().batch()
}
fn exchange(a: &Store, b: &Store) {
    let ba = changes(a);
    let bb = changes(b);
    a.sync_merge("account", &[bb], &[], &[]).unwrap();
    b.sync_merge("account", &[ba], &[], &[]).unwrap();
}

#[test]
fn first_login_merges_without_losing_local_records_and_retries_are_idempotent() {
    let (_a, a) = store();
    let (_b, b) = store();
    a.save(profile("a"), None).unwrap();
    b.save(profile("b"), None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    for _ in 0..3 {
        exchange(&a, &b);
    }
    assert_eq!(a.load().unwrap().len(), 2);
    assert_eq!(a.load().unwrap(), b.load().unwrap());
    assert_eq!(a.sync_capture().unwrap().events.len(), 2);
}

#[test]
fn concurrent_edits_converge_to_stable_copies_and_deletion_resolves_conflict() {
    let (_a, a) = store();
    let (_b, b) = store();
    a.save(profile("original"), None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    let mut pa = a.load().unwrap().remove(0);
    let mut pb = b.load().unwrap().remove(0);
    pa.name = "alpha".into();
    pb.name = "beta".into();
    a.save(pa.clone(), Some(pa.revision)).unwrap();
    b.save(pb.clone(), Some(pb.revision)).unwrap();
    exchange(&a, &b);
    let before = a.load().unwrap();
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|p| p.sync_conflict));
    assert_eq!(before, b.load().unwrap());
    exchange(&a, &b);
    assert_eq!(before, a.load().unwrap());
    a.remove(&before[0]).unwrap();
    exchange(&a, &b);
    assert_eq!(a.load().unwrap().len(), 1);
    assert!(!a.load().unwrap()[0].sync_conflict);
    let mut remaining = a.load().unwrap().remove(0);
    remaining.name = "resolved".into();
    a.save(remaining.clone(), Some(remaining.revision)).unwrap();
    exchange(&a, &b);
    assert!(!a.load().unwrap()[0].sync_conflict);
    assert_eq!(a.load().unwrap()[0].name, "resolved");
    assert_eq!(a.sync_capture().unwrap().events.len(), 5);
}

#[test]
fn identical_concurrent_edits_are_coalesced() {
    let (_a, a) = store();
    let (_b, b) = store();
    a.save(profile("original"), None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    for s in [&a, &b] {
        let mut p = s.load().unwrap().remove(0);
        p.tags.push("same".into());
        s.save(p.clone(), Some(p.revision)).unwrap();
    }
    exchange(&a, &b);
    assert_eq!(a.load().unwrap().len(), 1);
    assert!(!a.load().unwrap()[0].sync_conflict);
}

#[test]
fn offline_delete_does_not_resurrect_and_delete_edit_conflict_keeps_edit() {
    let (_a, a) = store();
    let (_b, b) = store();
    a.save(profile("old"), None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    let old_batch = changes(&a);
    a.remove(&a.load().unwrap()[0]).unwrap();
    exchange(&a, &b);
    b.sync_merge("account", &[old_batch], &[], &[]).unwrap();
    assert!(b.load().unwrap().is_empty());
    a.save(profile("next"), None).unwrap();
    exchange(&a, &b);
    let mut edited = b.load().unwrap().remove(0);
    edited.tags.push("offline edit".into());
    b.save(edited.clone(), Some(edited.revision)).unwrap();
    a.remove(&a.load().unwrap()[0]).unwrap();
    exchange(&a, &b);
    assert_eq!(a.load().unwrap().len(), 1);
    assert!(a.load().unwrap()[0].sync_conflict);
}

#[test]
fn private_paths_keys_recent_and_verification_are_never_in_wire_data() {
    let (_a, a) = store();
    let (_b, b) = store();
    let mut p = profile("server");
    p.set_identity_file(a.directory.join("TOP_SECRET_PRIVATE_KEY"));
    p.key_login_configured = true;
    let saved = a.save(p, None).unwrap();
    a.mark_used(saved.id, 123456789).unwrap();
    login(&a);
    login(&b);
    let wire = serde_json::to_string(&changes(&a)).unwrap();
    for forbidden in [
        "identity",
        "PRIVATE_KEY",
        "last_used",
        "123456789",
        "key_login",
        "refresh_token",
        "path",
    ] {
        assert!(!wire.contains(forbidden), "{forbidden}");
    }
    exchange(&a, &b);
    assert_eq!(a.load().unwrap()[0].last_used, Some(123456789));
    assert!(a.load().unwrap()[0].key_login_configured);
    let received = &b.load().unwrap()[0];
    assert!(received.identity_file().is_none());
    assert!(!received.key_login_configured);
    let count = a.sync_capture().unwrap().events.len();
    a.mark_used(saved.id, 123456790).unwrap();
    assert_eq!(a.sync_capture().unwrap().events.len(), count);
}

#[test]
fn target_change_resets_local_verification_and_preserves_local_key_path() {
    let (_a, a) = store();
    let (_b, b) = store();
    let mut p = profile("host");
    p.set_identity_file(a.directory.join("key"));
    p.key_login_configured = true;
    a.save(p, None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    let mut p = b.load().unwrap().remove(0);
    if let Connection::Manual { host, .. } = &mut p.connection {
        *host = "different.test".into();
    }
    b.save(p.clone(), Some(p.revision)).unwrap();
    exchange(&a, &b);
    assert!(!a.load().unwrap()[0].key_login_configured);
    assert!(a.load().unwrap()[0].identity_file().is_some());
}

#[test]
fn imported_source_is_local_and_new_device_must_bind_all_aliases() {
    let (_a, a) = store();
    let (bdir, b) = store();
    let cfg = a.directory.with_file_name("source");
    fs::write(&cfg, "Host one two\n").unwrap();
    a.import(sshm::import::preview(&cfg).unwrap().profiles)
        .unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    let received = b.load().unwrap();
    assert_eq!(received.len(), 2);
    assert!(received.iter().all(Profile::needs_binding));
    assert!(sshm::ssh::arguments(&received[0]).is_err());
    let local = bdir.path().join("localconfig");
    fs::write(&local, "Host one\n").unwrap();
    assert!(b.bind_source(&received[0], &local).is_err());
    fs::write(&local, "Host one two\n").unwrap();
    b.bind_source(&received[0], &local).unwrap();
    assert!(b.load().unwrap().iter().all(|p| !p.needs_binding()));
    assert_eq!(changes(&b).changes.len(), 0);
    assert!(
        !serde_json::to_string(&changes(&a))
            .unwrap()
            .contains(&cfg.display().to_string())
    );
}

#[test]
fn same_names_with_different_ids_are_preserved_and_stably_renamed() {
    let (_a, a) = store();
    let (_b, b) = store();
    a.save(profile("duplicate"), None).unwrap();
    b.save(profile("duplicate"), None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    let names: BTreeSet<_> = a.load().unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(names.len(), 2);
    exchange(&a, &b);
    assert_eq!(
        names,
        b.load().unwrap().into_iter().map(|p| p.name).collect()
    );
    assert_eq!(a.sync_capture().unwrap().events.len(), 2);
}

#[test]
fn v1_migration_backs_up_exact_bytes_and_retains_ids_and_local_fields() {
    let (_dir, a) = store();
    fs::create_dir_all(&a.directory).unwrap();
    let p = profile("old");
    let old = serde_json::to_vec(&serde_json::json!({"version": 1, "profiles": [p]})).unwrap();
    fs::write(a.path(), &old).unwrap();
    login(&a);
    assert_eq!(
        fs::read(a.directory.join("connections.v1.backup.json")).unwrap(),
        old
    );
    assert_eq!(a.load().unwrap()[0].id, p.id);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(a.path()).unwrap()).unwrap()["version"],
        2
    );
}

#[test]
fn external_changes_are_captured_but_missing_and_corrupt_files_are_not_deletions() {
    let (_dir, a) = store();
    a.save(profile("old"), None).unwrap();
    login(&a);
    let stale = a.load().unwrap().remove(0);
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(a.path()).unwrap()).unwrap();
    data["profiles"][0]["name"] = "external".into();
    fs::write(a.path(), serde_json::to_vec(&data).unwrap()).unwrap();
    assert!(a.save(stale.clone(), Some(stale.revision)).is_err());
    a.sync_capture().unwrap();
    assert_eq!(a.load().unwrap()[0].name, "external");
    assert_eq!(a.sync_capture().unwrap().events.len(), 2);
    fs::write(a.path(), "{broken").unwrap();
    assert!(a.sync_capture().is_err());
    assert_eq!(fs::read_to_string(a.path()).unwrap(), "{broken");
    fs::remove_file(a.path()).unwrap();
    assert!(a.sync_capture().is_err());
    assert!(!a.path().exists());
}

#[test]
fn malformed_remote_history_leaves_all_local_bytes_untouched() {
    let (_dir, a) = store();
    a.save(profile("safe"), None).unwrap();
    login(&a);
    let before = fs::read(a.path()).unwrap();
    let event = Change {
        id: Uuid::new_v4(),
        record: Uuid::new_v4(),
        parents: vec![Uuid::new_v4()],
        value: Some(Portable::from_profile(&profile("bad"))),
        branch_delete: false,
    };
    assert!(
        a.sync_merge(
            "account",
            &[Batch {
                version: 1,
                changes: vec![event]
            }],
            &["file".into()],
            &[]
        )
        .is_err()
    );
    assert_eq!(fs::read(a.path()).unwrap(), before);
}

#[test]
fn restart_preserves_outbox_and_account_switch_requires_confirmation() {
    let (_dir, a) = store();
    a.save(profile("safe"), None).unwrap();
    login(&a);
    let pending = changes(&a).changes;
    let reopened = Store {
        directory: a.directory.clone(),
    };
    assert_eq!(changes(&reopened).changes, pending);
    assert!(a.sync_account("other", "other@test", false).is_err());
    assert_eq!(
        a.sync_capture().unwrap().account.as_deref(),
        Some("account")
    );
    a.sync_account("other", "other@test", true).unwrap();
    assert_eq!(changes(&a).changes.len(), 1);
    assert_ne!(changes(&a).changes[0].id, pending[0].id);
}

#[test]
fn switching_accounts_preserves_offline_deletions_for_the_original_account() {
    let (_dir, a) = store();
    a.save(profile("deleted offline"), None).unwrap();
    login(&a);
    let p = a.load().unwrap().remove(0);
    a.remove(&p).unwrap();
    let original = changes(&a).changes;
    a.sync_account("other", "other@test", true).unwrap();
    a.save(profile("from other"), None).unwrap();
    a.sync_account("account", "test@example.test", true)
        .unwrap();
    let state = a.sync_capture().unwrap();
    for event in original {
        assert_eq!(state.events.get(&event.id), Some(&event));
        assert!(state.pending.contains(&event.id));
    }
    a.sync_merge("account", &[], &[], &[]).unwrap();
    assert_eq!(a.load().unwrap().len(), 1);
    assert_eq!(a.load().unwrap()[0].name, "from other");
}

#[test]
fn conflict_branch_edits_keep_the_correct_machine_local_key() {
    let (_a, a) = store();
    let (_b, b) = store();
    a.save(profile("original"), None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    for (s, name) in [(&a, "a"), (&b, "b")] {
        let mut p = s.load().unwrap().remove(0);
        p.name = name.into();
        s.save(p.clone(), Some(p.revision)).unwrap();
    }
    exchange(&a, &b);
    for mut p in a.load().unwrap() {
        p.set_identity_file(a.directory.join(format!("key-{}", p.id)));
        a.save(p.clone(), Some(p.revision)).unwrap();
    }
    let mut changed = a.load().unwrap().remove(0);
    let expected = changed.identity_file().cloned();
    changed.tags.push("edit branch".into());
    a.save(changed.clone(), Some(changed.revision)).unwrap();
    exchange(&a, &b);
    let profiles = a.load().unwrap();
    let changed = profiles
        .iter()
        .find(|p| p.tags.iter().any(|t| t == "edit branch"))
        .unwrap();
    assert_eq!(changed.identity_file().cloned(), expected);
    assert_eq!(changed.name.matches("（冲突").count(), 1);
}

#[test]
fn remote_merge_invalidates_open_forms_but_keeps_usage() {
    let (_a, a) = store();
    let (_b, b) = store();
    a.save(profile("old"), None).unwrap();
    login(&a);
    login(&b);
    exchange(&a, &b);
    let stale = a.load().unwrap().remove(0);
    a.mark_used(stale.id, 42).unwrap();
    let mut edited = b.load().unwrap().remove(0);
    edited.name = "remote".into();
    b.save(edited.clone(), Some(edited.revision)).unwrap();
    exchange(&a, &b);
    assert!(a.save(stale.clone(), Some(stale.revision)).is_err());
    assert_eq!(a.load().unwrap()[0].last_used, Some(42));
}
