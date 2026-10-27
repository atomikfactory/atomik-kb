//! Milestone 4 tests: device keys, recovery kit, keyring, root key rotation, app lock
//! and the backup file, through `dispatch` and the binary APIs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_doc::{Doc, DocKind};
use kb_keys::{KeySubject, Keystore};
use kb_model::{BlockKind, PageDoc};
use kb_storage::SecurityQueries;
use kb_types::{
    BlockId, DeviceEntry, DocId, ErrorCode, Event, PageId, RecoveryKit, Request, Response,
    SecurityInfo, SpaceId,
};

use crate::security::{META_ARK_GENERATION, META_ROTATION_PENDING};
use crate::test_support::{Fixture, create_page, create_space, create_workspace, spaces};
use crate::{Core, Opened};

const PASSPHRASE: &str = "correct horse battery";

fn security(core: &mut Core) -> SecurityInfo {
    match core.dispatch(Request::SecurityStatus).unwrap() {
        Response::Security(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn kit_create(core: &mut Core) -> RecoveryKit {
    match core.dispatch(Request::RecoveryKitCreate).unwrap() {
        Response::RecoveryKit(kit) => kit,
        other => panic!("unexpected {other:?}"),
    }
}

fn kit_reissue(core: &mut Core, passphrase: Option<&str>) -> RecoveryKit {
    match core
        .dispatch(Request::RecoveryKitReissue {
            passphrase: passphrase.map(str::to_owned),
        })
        .unwrap()
    {
        Response::RecoveryKit(kit) => kit,
        other => panic!("unexpected {other:?}"),
    }
}

fn verify(core: &mut Core, kit: &str) -> Result<Response, kb_types::ApiError> {
    core.dispatch(Request::RecoveryKitVerify { kit: kit.into() })
}

fn devices(core: &mut Core) -> Vec<DeviceEntry> {
    match core.dispatch(Request::DeviceList).unwrap() {
        Response::Devices(list) => list,
        other => panic!("unexpected {other:?}"),
    }
}

/// Puts some text into a page the way the editor does.
fn write_page(core: &mut Core, page_id: PageId, text: &str, n: u8) {
    let snapshot = core.page_doc_open(page_id).unwrap();
    let blob = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &snapshot);
    let doc = PageDoc::from_doc(
        Doc::load(DocKind::Page, 0x5000 + u64::from(n), Some(&blob), &[]).unwrap(),
    )
    .unwrap();
    let before = doc.doc().version();
    let last = doc.block_ids().last().copied();
    doc.insert_block(
        last,
        BlockKind::Paragraph,
        text,
        BlockId::from_random_bytes([n; 16]),
    )
    .unwrap();
    let update = doc
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    core.page_doc_import(page_id, payload).unwrap();
    core.page_doc_close(page_id).unwrap();
}

fn content(core: &mut Core, page_id: PageId) -> serde_json::Value {
    match core.dispatch(Request::PageContentGet { page_id }).unwrap() {
        Response::PageContent(c) => c.content,
        other => panic!("unexpected {other:?}"),
    }
}

fn first_space(core: &mut Core) -> SpaceId {
    spaces(core)[0].id
}

#[test]
fn open_creates_device_keys_and_reports_an_empty_security_state() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    let dsk = fx.keystore.get(&format!("dsk:{}", fx.install_id)).unwrap();
    let dak = fx.keystore.get(&format!("dak:{}", fx.install_id)).unwrap();
    assert_eq!(dsk.map(|v| v.len()), Some(32));
    assert_eq!(dak.map(|v| v.len()), Some(32));

    create_workspace(&mut core, "Home", Some("Personal"));
    let info = security(&mut core);
    assert_eq!(info.account_id, None);
    assert!(info.recovery_kit.is_none());
    assert!(!info.app_lock.enabled);
    assert_eq!(info.devices, 0);
    assert_eq!(info.keyring.spaces, 1, "the first space has its key record");
    assert_eq!(
        info.keyring.documents, 1,
        "its structure document has one too"
    );
    assert!(devices(&mut core).is_empty());
    drop(core);

    // The same keys come back.
    let core = fx.open(2, 1);
    assert_eq!(
        fx.keystore
            .get(&format!("dsk:{}", fx.install_id))
            .unwrap()
            .unwrap(),
        dsk_bytes(&fx)
    );
    drop(core);
}

fn dsk_bytes(fx: &Fixture) -> zeroize::Zeroizing<Vec<u8>> {
    fx.keystore
        .get(&format!("dsk:{}", fx.install_id))
        .unwrap()
        .unwrap()
}

#[test]
fn recovery_kit_is_created_verified_and_refused_when_wrong() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    assert_eq!(
        core.dispatch(Request::RecoveryKitCreate).unwrap_err().code,
        ErrorCode::InvalidInput,
        "no workspace yet"
    );
    create_workspace(&mut core, "Home", Some("Personal"));
    core.drain_events();

    let kit = kit_create(&mut core);
    assert_eq!(kit.words.len(), 24);
    assert!(
        kit.words
            .iter()
            .all(|w| w.chars().all(|c| c.is_ascii_lowercase()))
    );
    let groups: Vec<&str> = kit.code.split('-').collect();
    assert_eq!(
        groups.len(),
        14,
        "13 data groups and a checksum group: {}",
        kit.code
    );
    assert!(groups[..13].iter().all(|g| g.len() == 4));
    assert_eq!(groups[13].len(), 2);
    assert_eq!(kit.created_at, 100);
    assert_eq!(core.drain_events(), vec![Event::SecurityChanged]);

    let info = security(&mut core);
    assert_eq!(info.account_id, Some(kit.account_id));
    assert_eq!(info.devices, 1);
    let kit_info = info.recovery_kit.unwrap();
    assert_eq!(kit_info.created_at, 100);
    assert_eq!(kit_info.verified_at, None);
    let list = devices(&mut core);
    assert_eq!(list.len(), 1);
    assert!(list[0].is_this_device);
    assert!(!list[0].added_via_kit);
    assert_eq!(list[0].signing_key.len(), 64);

    assert_eq!(
        core.dispatch(Request::RecoveryKitCreate).unwrap_err().code,
        ErrorCode::AlreadyExists
    );

    // Words, code, upper case and four-letter prefixes all verify.
    verify(&mut core, &kit.words.join(" ")).unwrap();
    verify(&mut core, &kit.code).unwrap();
    verify(&mut core, &kit.code.to_lowercase().replace('-', " ")).unwrap();
    let prefixes: Vec<String> = kit
        .words
        .iter()
        .map(|w| w.chars().take(4).collect::<String>().to_uppercase())
        .collect();
    verify(&mut core, &prefixes.join("\n")).unwrap();
    assert!(
        security(&mut core)
            .recovery_kit
            .unwrap()
            .verified_at
            .is_some()
    );

    // A shuffled kit, garbage and another kit's words are refused.
    let mut shuffled = kit.words.clone();
    shuffled.swap(0, 1);
    let err = verify(&mut core, &shuffled.join(" ")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(err.details, Some(serde_json::json!({ "field": "kit" })));
    assert_eq!(
        verify(&mut core, "not a kit").unwrap_err().code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        verify(&mut core, &"abandon ".repeat(24)).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    for word in &kit.words {
        assert!(
            !format!("{err:?}").contains(word.as_str()),
            "kit words never leak into errors"
        );
    }

    // Everything survives a reopen.
    drop(core);
    let mut core = fx.open(2, 200);
    let info = security(&mut core);
    assert_eq!(info.account_id, Some(kit.account_id));
    assert!(info.recovery_kit.unwrap().verified_at.is_some());
    verify(&mut core, &kit.words.join(" ")).unwrap();
}

#[test]
fn keyring_follows_spaces_and_documents_and_repairs_itself() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = first_space(&mut core);
    let other = create_space(&mut core, "Work");
    let page = create_page(&mut core, space, None, "Notes");
    write_page(&mut core, page.id, "hello", 1);
    let gone = create_page(&mut core, space, None, "Temp");
    write_page(&mut core, gone.id, "bye", 2);

    let info = security(&mut core);
    assert_eq!(info.keyring.spaces, 2);
    assert_eq!(
        info.keyring.documents, 4,
        "two structure docs and two page docs"
    );
    assert_eq!(info.keyring.tombstones, 0);

    // Purge tombstones the page document's key (crypto-erase).
    core.dispatch(Request::PageTrash { page_id: gone.id })
        .unwrap();
    core.dispatch(Request::PagePurge { page_id: gone.id })
        .unwrap();
    let info = security(&mut core);
    assert_eq!(info.keyring.tombstones, 1);
    assert_eq!(info.keyring.documents, 3);
    assert!(core.security.keyring.is_tombstoned(
        space,
        &KeySubject::Doc {
            doc_id: DocId::for_page(gone.id),
            epoch: 0
        }
    ));

    // Deleting an empty space forgets its keys.
    core.dispatch(Request::SpaceDelete { space_id: other.id })
        .unwrap();
    assert_eq!(security(&mut core).keyring.spaces, 1);
    assert!(core.db.keyring_by_space(other.id).unwrap().is_empty());

    // A database whose keyring rows are gone (a Milestone 3 database) is repaired at open.
    core.db.keyring_delete_space(space).unwrap();
    drop(core);
    let mut core = fx.open(2, 1);
    let info = security(&mut core);
    assert_eq!(info.keyring.spaces, 1);
    assert_eq!(
        info.keyring.documents, 2,
        "structure doc + the live page doc"
    );
    let rows = core.db.keyring_all().unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn reissuing_the_kit_rotates_the_root_key() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = first_space(&mut core);
    let page = create_page(&mut core, space, None, "Keep me");
    write_page(&mut core, page.id, "precious", 1);
    let kit = kit_create(&mut core);
    let old_ark = fx
        .keystore
        .get(&format!("ark:{}", fx.install_id))
        .unwrap()
        .unwrap();
    let space_key_before = core.ensure_space_key(space).unwrap();
    let old_recovery_key = core.security.log_state.as_ref().unwrap().recovery_key;
    core.drain_events();

    let new_kit = kit_reissue(&mut core, None);
    assert_ne!(new_kit.words, kit.words);
    assert_eq!(
        new_kit.account_id, kit.account_id,
        "the account does not change"
    );
    assert_eq!(core.drain_events(), vec![Event::SecurityChanged]);

    let new_ark = fx
        .keystore
        .get(&format!("ark:{}", fx.install_id))
        .unwrap()
        .unwrap();
    assert_ne!(new_ark, old_ark, "a new root key");
    assert!(
        fx.keystore
            .get(&format!("ark-next:{}", fx.install_id))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        core.db.meta_get(META_ARK_GENERATION).unwrap().as_deref(),
        Some("1")
    );
    assert_eq!(
        core.db.meta_get(META_ROTATION_PENDING).unwrap().as_deref(),
        Some("0")
    );

    // The first space key is the same key, re-wrapped under the new root key; the
    // rotation also opened a new epoch with a new key (ADR-0022).
    assert_eq!(
        core.space_key_at(space, 0).unwrap().unwrap(),
        space_key_before
    );
    let record = core
        .security
        .keyring
        .record(space, &KeySubject::Space { epoch: 0 })
        .unwrap();
    assert_eq!(record.wrap_epoch, 1);
    assert_eq!(core.space_epoch(space), 1);
    assert_ne!(core.ensure_space_key(space).unwrap(), space_key_before);

    // The old kit is gone, the new one works, the log recorded the rotation.
    assert_eq!(
        verify(&mut core, &kit.words.join(" ")).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    verify(&mut core, &new_kit.words.join(" ")).unwrap();
    let state = core.security.log_state.as_ref().unwrap();
    assert_ne!(state.recovery_key, old_recovery_key);
    assert_eq!(state.root_generation, 1);
    // Genesis, RecoveryRotated, RootRotated (no box: this is the only device).
    assert_eq!(core.security.log.as_ref().unwrap().entries().len(), 3);
    let info = security(&mut core);
    assert!(info.recovery_kit.unwrap().reissued_at.is_some());

    // Content and keys survive a reopen with the rotated key.
    drop(core);
    let mut core = fx.open(2, 5);
    assert_eq!(
        core.space_key_at(space, 0).unwrap().unwrap(),
        space_key_before
    );
    let json = content(&mut core, page.id);
    assert!(json.to_string().contains("precious"));
    verify(&mut core, &new_kit.code).unwrap();
}

#[test]
fn the_app_lock_wraps_the_root_key_and_gates_the_core() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = first_space(&mut core);
    create_page(&mut core, space, None, "Locked away");

    let short = core
        .dispatch(Request::AppLockEnable {
            passphrase: "short".into(),
        })
        .unwrap_err();
    assert_eq!(short.code, ErrorCode::InvalidInput);
    core.dispatch(Request::AppLockEnable {
        passphrase: PASSPHRASE.into(),
    })
    .unwrap();
    let info = security(&mut core);
    assert!(info.app_lock.enabled);
    assert_eq!(
        info.app_lock.iterations, 1,
        "the fixture's small parameters"
    );
    assert!(
        fx.keystore
            .get(&format!("ark:{}", fx.install_id))
            .unwrap()
            .is_none()
    );
    assert!(
        fx.keystore
            .get(&format!("ark-locked:{}", fx.install_id))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        core.dispatch(Request::AppLockEnable {
            passphrase: PASSPHRASE.into(),
        })
        .unwrap_err()
        .code,
        ErrorCode::AlreadyExists
    );

    // Lock, then the plain open refuses and the two-phase open yields a locked core.
    let locked = core.lock().map_err(|(_, e)| e).unwrap();
    assert_eq!(locked.params().iterations, 1);
    let err = Core::open(fx.deps(2, 1)).err().unwrap();
    assert_eq!(err.code, ErrorCode::Locked);
    let Opened::Locked(locked2) = Core::open_or_locked(fx.deps(2, 1)).unwrap() else {
        panic!("expected a locked core");
    };
    drop(locked2);

    let (locked, err) = locked.unlock("wrong passphrase!").err().unwrap();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(
        err.details,
        Some(serde_json::json!({ "field": "passphrase" }))
    );
    let mut core = locked.unlock(PASSPHRASE).map_err(|(_, e)| e).unwrap();
    assert_eq!(
        spaces(&mut core).len(),
        1,
        "the unlocked core serves requests"
    );

    // Re-issuing the kit while locked needs the passphrase and keeps the lock.
    kit_create(&mut core);
    let err = core
        .dispatch(Request::RecoveryKitReissue { passphrase: None })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = core
        .dispatch(Request::RecoveryKitReissue {
            passphrase: Some("nope nope nope".into()),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    kit_reissue(&mut core, Some(PASSPHRASE));
    assert!(
        fx.keystore
            .get(&format!("ark-locked:{}", fx.install_id))
            .unwrap()
            .is_some()
    );
    assert!(
        fx.keystore
            .get(&format!("ark:{}", fx.install_id))
            .unwrap()
            .is_none()
    );

    // Change and disable.
    assert_eq!(
        core.dispatch(Request::AppLockChange {
            current: "wrong".into(),
            new: "another passphrase".into()
        })
        .unwrap_err()
        .code,
        ErrorCode::InvalidInput
    );
    core.dispatch(Request::AppLockChange {
        current: PASSPHRASE.into(),
        new: "another passphrase".into(),
    })
    .unwrap();
    assert_eq!(
        core.dispatch(Request::AppLockDisable {
            passphrase: PASSPHRASE.into()
        })
        .unwrap_err()
        .code,
        ErrorCode::InvalidInput
    );
    core.dispatch(Request::AppLockDisable {
        passphrase: "another passphrase".into(),
    })
    .unwrap();
    assert!(!security(&mut core).app_lock.enabled);
    assert!(core.lock().is_err(), "nothing to lock without the app lock");
    let core = Core::open(fx.deps(3, 2)).unwrap();
    drop(core);
}

/// Exports a backup into memory.
fn export(core: &mut Core) -> (Vec<u8>, kb_types::BackupSummary) {
    let mut bytes = Vec::new();
    let summary = core
        .backup_export(&mut |chunk: &[u8]| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
    assert_eq!(summary.bytes, bytes.len() as u64);
    (bytes, summary)
}

/// Imports a backup from memory.
fn import(
    core: &mut Core,
    bytes: &[u8],
    kit: Option<&str>,
) -> Result<kb_types::BackupSummary, kb_types::ApiError> {
    let mut pos = 0usize;
    core.backup_import(
        &mut |buf: &mut [u8]| {
            let n = buf.len().min(bytes.len() - pos);
            buf[..n].copy_from_slice(&bytes[pos..pos + n]);
            pos += n;
            Ok(n)
        },
        kit,
    )
}

#[test]
fn a_backup_restores_the_whole_workspace_on_a_fresh_installation() {
    let fx = Fixture::new();
    let mut a = fx.open(1, 1_000);
    create_workspace(&mut a, "Home", Some("Personal"));
    let personal = first_space(&mut a);
    let work = create_space(&mut a, "Work");
    let plan = create_page(&mut a, personal, None, "Plan");
    let child = create_page(&mut a, personal, Some(plan.id), "Child");
    let report = create_page(&mut a, work.id, None, "Report");
    write_page(&mut a, plan.id, "the quarterly roadmap", 1);
    write_page(&mut a, child.id, "a nested thought", 2);
    write_page(&mut a, report.id, "figures and tables", 3);
    a.dispatch(Request::SettingsSet {
        key: "ui.theme".into(),
        value: serde_json::json!("dark"),
    })
    .unwrap();
    // Device-bound settings (a remote AI endpoint with its consent, the backup folder) never
    // travel in a backup.
    for (key, value) in [
        (
            kb_types::ai::AI_ENDPOINT_SETTING,
            serde_json::json!({ "url": "http://10.0.0.9:11434", "remoteConsent": true }),
        ),
        (
            kb_types::interop::BACKUP_FOLDER_SETTING,
            serde_json::json!(r"C:\backups"),
        ),
    ] {
        a.dispatch(Request::SettingsSet {
            key: key.into(),
            value,
        })
        .unwrap();
    }
    assert_eq!(
        a.backup_export(&mut |_| Ok(())).unwrap_err().code,
        ErrorCode::InvalidInput,
        "no kit, no backup"
    );
    let kit = kit_create(&mut a);
    let plan_json = content(&mut a, plan.id);
    let (bytes, summary) = export(&mut a);
    assert_eq!(
        summary.documents,
        1 + 2 + 3,
        "account, two structures, three pages"
    );
    assert_eq!(summary.settings, 1);
    assert_eq!(summary.account_id, kit.account_id);
    assert!(bytes.starts_with(crate::BACKUP_MAGIC));
    assert!(
        !String::from_utf8_lossy(&bytes).contains("quarterly"),
        "content is encrypted"
    );

    // A fresh installation: new keystore, new install id, new database.
    let fx_b = Fixture::new();
    let mut b = fx_b.open(7, 2_000);
    let err = import(&mut b, &bytes, None).unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidInput,
        "the kit is required: {err}"
    );
    let err = import(&mut b, &bytes, Some(&"abandon ".repeat(24))).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let mut damaged = bytes.clone();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0x55;
    let err = import(&mut b, &damaged, Some(&kit.words.join(" "))).unwrap_err();
    assert_eq!(err.code, ErrorCode::Crypto, "{err}");
    let err = import(
        &mut b,
        &bytes[..bytes.len() - 20],
        Some(&kit.words.join(" ")),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Crypto, "truncated: {err}");
    assert!(
        b.dispatch(Request::WorkspaceGet).unwrap() == Response::Workspace(None),
        "failed imports leave the installation empty"
    );

    b.drain_events();
    let restored = import(&mut b, &bytes, Some(&kit.code)).unwrap();
    assert_eq!(restored.documents, summary.documents);
    assert_eq!(restored.account_id, kit.account_id);
    let events = b.drain_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::WorkspaceCreated { .. }))
    );
    assert!(events.contains(&Event::SpacesChanged));
    assert!(events.contains(&Event::SecurityChanged));

    let names: Vec<String> = spaces(&mut b).into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["Personal", "Work"]);
    assert_eq!(content(&mut b, plan.id), plan_json);
    assert_eq!(
        b.dispatch(Request::SettingsGet {
            key: "ui.theme".into()
        })
        .unwrap(),
        Response::Setting(Some(serde_json::json!("dark")))
    );
    for key in [
        kb_types::ai::AI_ENDPOINT_SETTING,
        kb_types::interop::BACKUP_FOLDER_SETTING,
    ] {
        assert_eq!(
            b.dispatch(Request::SettingsGet { key: key.into() })
                .unwrap(),
            Response::Setting(None),
            "{key} is not restored"
        );
    }
    match b
        .dispatch(Request::Search {
            query: "quarterly".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => assert_eq!(hits.len(), 1),
        other => panic!("unexpected {other:?}"),
    }
    let info = security(&mut b);
    assert_eq!(info.account_id, Some(kit.account_id));
    assert_eq!(info.devices, 2, "the original device and this one");
    assert!(!info.app_lock.enabled);
    let list = devices(&mut b);
    let me = list.iter().find(|d| d.is_this_device).unwrap();
    assert!(me.added_via_kit);
    assert_eq!(info.keyring.spaces, 2);
    assert!(info.keyring.documents >= 5);
    // The restored installation now holds the account's root key and the kit verifies.
    assert_eq!(
        fx_b.keystore
            .get(&format!("ark:{}", fx_b.install_id))
            .unwrap()
            .unwrap(),
        fx.keystore
            .get(&format!("ark:{}", fx.install_id))
            .unwrap()
            .unwrap()
    );
    verify(&mut b, &kit.words.join(" ")).unwrap();

    // A populated installation refuses a restore; the restored one reopens.
    assert_eq!(
        import(&mut a, &bytes, Some(&kit.code)).unwrap_err().code,
        ErrorCode::AlreadyExists
    );
    drop(b);
    let mut b = fx_b.open(8, 3_000);
    assert_eq!(content(&mut b, plan.id), plan_json);
    assert_eq!(security(&mut b).devices, 2);
}
