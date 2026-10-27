//! App lock: keystore states, transitions, passphrases, Argon2id minimums.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::test_support::{self, Faulty, TINY, install_id, other_install_id};
use crate::{KeystoreError, MemoryKeystore, load_or_create_ark};
use kb_types::testing::SeededRng;

const PASS: &str = "correct horse battery";
const OTHER_PASS: &str = "another passphrase";

/// Stronger than [`TINY`], still fast.
const SMALL: Argon2Params = Argon2Params {
    memory_kib: 16,
    iterations: 1,
    parallelism: 1,
};

fn plain() -> String {
    ark_entry_name(&install_id())
}

fn locked() -> String {
    lock_entry_name(&install_id())
}

/// A keystore holding a fresh plain ARK.
fn unlocked_keystore() -> (Faulty, Ark) {
    let keystore = Faulty::default();
    let ark = create_ark(&keystore, &SeededRng::new(1), &install_id()).unwrap();
    keystore.clear_ops();
    (keystore, ark)
}

fn locked_ark(keystore: &dyn Keystore) -> LockedArk {
    let state = load_ark_state(keystore, &install_id()).unwrap();
    assert!(matches!(state, ArkState::Locked(_)), "{state:?}");
    match state {
        ArkState::Locked(locked) => Some(locked),
        _ => None,
    }
    .unwrap()
}

fn unlocked_ark(keystore: &dyn Keystore) -> Ark {
    let state = load_ark_state(keystore, &install_id()).unwrap();
    assert!(matches!(state, ArkState::Unlocked(_)), "{state:?}");
    match state {
        ArkState::Unlocked(ark) => Some(ark),
        _ => None,
    }
    .unwrap()
}

#[test]
fn entry_names() {
    assert_eq!(locked(), "ark-locked:12345678-9abc-4def-8123-456789abcdef");
    assert_eq!(plain(), "ark:12345678-9abc-4def-8123-456789abcdef");
}

#[test]
fn an_empty_keystore_is_missing() {
    let keystore = MemoryKeystore::new();
    assert!(matches!(
        load_ark_state(&keystore, &install_id()).unwrap(),
        ArkState::Missing
    ));
}

#[test]
fn a_plain_entry_is_unlocked() {
    let keystore = MemoryKeystore::new();
    keystore.set(&plain(), &[7; 32]).unwrap();
    assert_eq!(
        unlocked_ark(&keystore),
        Ark::from_secret(Secret32::from_bytes([7; 32]))
    );
}

#[test]
fn malformed_entries_are_corrupted_and_left_untouched() {
    // A plain entry of the wrong length.
    let keystore = MemoryKeystore::new();
    keystore.set(&plain(), &[7; 31]).unwrap();
    assert!(matches!(
        load_ark_state(&keystore, &install_id()),
        Err(KeysError::Corrupted(_))
    ));
    assert_eq!(keystore.get(&plain()).unwrap().unwrap().len(), 31);

    // A locked entry that does not parse never displaces a plain one.
    let keystore = MemoryKeystore::new();
    keystore.set(&plain(), &[7; 32]).unwrap();
    keystore.set(&locked(), b"garbage").unwrap();
    let err = load_ark_state(&keystore, &install_id()).unwrap_err();
    assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
    assert!(!err.to_string().contains("garbage"));
    assert_eq!(keystore.get(&plain()).unwrap().unwrap().as_slice(), [7; 32]);
    assert_eq!(
        keystore.get(&locked()).unwrap().unwrap().as_slice(),
        b"garbage"
    );
}

#[test]
fn create_ark_stores_a_plain_entry_and_reads_it_back() {
    let keystore = Faulty::default();
    let ark = create_ark(&keystore, &SeededRng::new(5), &install_id()).unwrap();
    assert_eq!(
        ark.secret(),
        &Secret32::generate(&SeededRng::new(5)),
        "one 32-byte draw"
    );
    assert_eq!(
        keystore.ops(),
        [
            format!("get:{}", locked()),
            format!("get:{}", plain()),
            format!("set:{}", plain()),
            format!("get:{}", plain()),
        ]
    );
    assert_eq!(unlocked_ark(&keystore), ark);
}

#[test]
fn create_ark_never_overwrites_a_root_key() {
    for existing in [plain(), locked()] {
        let keystore = MemoryKeystore::new();
        keystore.set(&existing, &[7; 32]).unwrap();
        let err = create_ark(&keystore, &SeededRng::new(5), &install_id()).unwrap_err();
        assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
        assert_eq!(
            keystore.get(&existing).unwrap().unwrap().as_slice(),
            [7; 32]
        );
    }
}

#[test]
fn create_ark_detects_a_mangling_keystore() {
    let keystore = Faulty::mangling();
    let err = create_ark(&keystore, &SeededRng::new(5), &install_id()).unwrap_err();
    assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
    assert!(
        keystore.inner.get(&plain()).unwrap().is_none(),
        "removed again"
    );
}

#[test]
fn passphrase_length_is_counted_in_bytes() {
    for (passphrase, ok) in [
        (String::new(), false),
        ("1234567".to_owned(), false),
        ("12345678".to_owned(), true),
        ("x".repeat(256), true),
        ("x".repeat(257), false),
        // 'é' is two bytes in UTF-8.
        ("\u{e9}".repeat(3), false),
        ("\u{e9}".repeat(4), true),
        ("\u{e9}".repeat(128), true),
        ("\u{e9}".repeat(129), false),
    ] {
        let result = check_passphrase(&passphrase);
        assert_eq!(result.is_ok(), ok, "{} bytes", passphrase.len());
        if !ok {
            assert!(matches!(result, Err(KeysError::PassphraseLength)));
        }
    }
    assert_eq!(
        KeysError::PassphraseLength.to_string(),
        "passphrase must be 8 to 256 bytes"
    );
}

#[test]
fn enable_unlock_disable_round_trip() {
    let (keystore, ark) = unlocked_keystore();

    // Enable: locked entry written and read back, then the plain entry deleted.
    enable_lock(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap();
    assert_eq!(
        keystore.ops(),
        [
            format!("set:{}", locked()),
            format!("get:{}", locked()),
            format!("delete:{}", plain()),
        ]
    );
    assert!(keystore.inner.get(&plain()).unwrap().is_none());
    let state = locked_ark(&keystore);
    assert_eq!(state.params(), TINY);
    assert!(matches!(
        load_or_create_ark(&keystore, &SeededRng::new(9), &install_id()),
        Err(KeysError::Locked)
    ));

    // Unlock.
    assert_eq!(state.unlock_with_minimum(PASS, &TINY).unwrap(), ark);
    assert!(matches!(
        state.unlock_with_minimum("wrong passphrase", &TINY),
        Err(KeysError::WrongPassphrase)
    ));

    // Disable: plain entry written and read back, then the locked entry deleted.
    keystore.clear_ops();
    disable_lock(&keystore, &install_id(), &ark, &SeededRng::new(3)).unwrap();
    assert_eq!(
        keystore.ops(),
        [
            format!("set:{}", plain()),
            format!("get:{}", plain()),
            format!("delete:{}", locked()),
        ]
    );
    assert!(keystore.inner.get(&locked()).unwrap().is_none());
    assert_eq!(unlocked_ark(&keystore), ark);
    assert_eq!(
        load_or_create_ark(&keystore, &SeededRng::new(9), &install_id()).unwrap(),
        ark
    );
}

#[test]
fn unlock_refuses_blobs_weaker_than_the_minimum() {
    let (keystore, ark) = unlocked_keystore();
    enable_lock(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap();
    let state = locked_ark(&keystore);

    // The production minimum refuses the small parameters, whatever the passphrase.
    for passphrase in [PASS, "wrong passphrase"] {
        assert!(matches!(
            state.unlock(passphrase),
            Err(KeysError::Crypto(CryptoError::WeakParameters))
        ));
    }
    for minimum in [SMALL, Argon2Params::MINIMUM, Argon2Params::DEFAULT] {
        assert!(matches!(
            state.unlock_with_minimum(PASS, &minimum),
            Err(KeysError::Crypto(CryptoError::WeakParameters))
        ));
    }
    // A blob at least as strong as the minimum opens.
    enable_lock(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        SMALL,
        &SeededRng::new(4),
    )
    .unwrap();
    let state = locked_ark(&keystore);
    assert_eq!(state.params(), SMALL);
    assert_eq!(state.unlock_with_minimum(PASS, &TINY).unwrap(), ark);
    assert_eq!(state.unlock_with_minimum(PASS, &SMALL).unwrap(), ark);
}

#[test]
fn the_locked_blob_follows_the_documented_construction() {
    let (keystore, ark) = unlocked_keystore();
    enable_lock(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap();
    let stored = keystore.inner.get(&locked()).unwrap().unwrap();
    let blob = PassphraseWrapped::from_bytes(&stored).unwrap();
    assert_eq!(blob.params, TINY);

    let mut subject = b"ark-locked".to_vec();
    subject.extend_from_slice(install_id().as_bytes());
    let unwrapped =
        kb_crypto::unwrap_with_passphrase(&blob, PASS.as_bytes(), &subject, &TINY).unwrap();
    assert_eq!(&unwrapped, ark.secret());

    // Bound to its installation.
    let elsewhere = LockedArk::new(blob, other_install_id());
    assert!(matches!(
        elsewhere.unlock_with_minimum(PASS, &TINY),
        Err(KeysError::WrongPassphrase)
    ));
}

#[test]
fn both_entries_present_resolve_to_locked() {
    let (keystore, ark) = unlocked_keystore();
    // A crash after the locked entry was written, before the plain one was deleted.
    let blob = kb_crypto::wrap_with_passphrase(
        ark.secret(),
        PASS.as_bytes(),
        TINY,
        &locked_subject(&install_id()),
        &SeededRng::new(2),
    )
    .unwrap();
    keystore.inner.set(&locked(), &blob.to_bytes()).unwrap();

    let state = locked_ark(&keystore);
    assert!(
        keystore.inner.get(&plain()).unwrap().is_none(),
        "the plain entry is deleted"
    );
    assert_eq!(state.unlock_with_minimum(PASS, &TINY).unwrap(), ark);
    // And it stays that way.
    assert!(matches!(
        load_ark_state(&keystore, &install_id()).unwrap(),
        ArkState::Locked(_)
    ));
}

#[test]
fn an_interrupted_enable_resolves_to_locked_at_the_next_open() {
    let ark = test_support::ark(1);
    let failing = Faulty {
        fail_delete: true,
        ..Faulty::default()
    };
    failing.inner.set(&plain(), ark.secret().expose()).unwrap();

    let err = enable_lock(
        &failing,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap_err();
    assert!(
        matches!(&err, KeysError::Keystore(KeystoreError::Other(m)) if m == "delete refused"),
        "{err:?}"
    );
    // Both entries now exist; with deletes still failing the state cannot be settled.
    assert!(matches!(
        load_ark_state(&failing, &install_id()),
        Err(KeysError::Keystore(_))
    ));
    // Once deletes work again, the locked entry wins.
    let state = locked_ark(&failing.inner);
    assert!(failing.inner.get(&plain()).unwrap().is_none());
    assert_eq!(state.unlock_with_minimum(PASS, &TINY).unwrap(), ark);
}

#[test]
fn enable_checks_the_passphrase_before_touching_the_keystore() {
    let (keystore, ark) = unlocked_keystore();
    for passphrase in ["short", &"x".repeat(257)] {
        let err = enable_lock(
            &keystore,
            &install_id(),
            &ark,
            passphrase,
            TINY,
            &SeededRng::new(2),
        )
        .unwrap_err();
        assert!(matches!(err, KeysError::PassphraseLength));
    }
    assert!(keystore.ops().is_empty());
    assert_eq!(unlocked_ark(&keystore), ark);
}

#[test]
fn a_mangled_locked_write_keeps_the_plain_entry() {
    let (_, ark) = unlocked_keystore();
    let keystore = Faulty {
        mangle_set: Some("ark-locked:"),
        ..Faulty::default()
    };
    keystore.inner.set(&plain(), ark.secret().expose()).unwrap();

    let err = enable_lock(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap_err();
    assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
    assert!(
        keystore.inner.get(&locked()).unwrap().is_none(),
        "removed again"
    );
    assert_eq!(unlocked_ark(&keystore), ark, "the plain entry is untouched");
}

#[test]
fn a_mangled_plain_write_keeps_the_locked_entry() {
    let (unlocked, ark) = unlocked_keystore();
    enable_lock(
        &unlocked,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap();
    let blob = unlocked.inner.get(&locked()).unwrap().unwrap();

    let keystore = Faulty {
        mangle_set: Some("ark:"),
        ..Faulty::default()
    };
    keystore.inner.set(&locked(), &blob).unwrap();
    let err = disable_lock(&keystore, &install_id(), &ark, &SeededRng::new(3)).unwrap_err();
    assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
    assert!(
        keystore.inner.get(&plain()).unwrap().is_none(),
        "removed again"
    );
    let state = locked_ark(&keystore);
    assert_eq!(state.unlock_with_minimum(PASS, &TINY).unwrap(), ark);
}

#[test]
fn change_passphrase_verifies_the_current_one() {
    let (keystore, ark) = unlocked_keystore();
    enable_lock(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap();
    let before = keystore.inner.get(&locked()).unwrap().unwrap();
    let rng = SeededRng::new(3);

    // Wrong current passphrase: nothing changes.
    let err = change_passphrase(
        &keystore,
        &install_id(),
        &ark,
        "not the one",
        OTHER_PASS,
        TINY,
        &rng,
    )
    .unwrap_err();
    assert!(matches!(err, KeysError::WrongPassphrase), "{err}");
    // A new passphrase of the wrong length is refused before any Argon2id work.
    let err =
        change_passphrase(&keystore, &install_id(), &ark, PASS, "short", TINY, &rng).unwrap_err();
    assert!(matches!(err, KeysError::PassphraseLength), "{err}");
    // The minimum comes from `params`: a stored blob weaker than them is refused.
    let err = change_passphrase(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        OTHER_PASS,
        SMALL,
        &rng,
    )
    .unwrap_err();
    assert!(
        matches!(err, KeysError::Crypto(CryptoError::WeakParameters)),
        "{err}"
    );
    // Another ARK than the stored one.
    let err = change_passphrase(
        &keystore,
        &install_id(),
        &test_support::ark(9),
        PASS,
        OTHER_PASS,
        TINY,
        &rng,
    )
    .unwrap_err();
    assert!(matches!(err, KeysError::Corrupted(_)), "{err}");
    assert_eq!(
        keystore.inner.get(&locked()).unwrap().unwrap().as_slice(),
        before.as_slice(),
        "every refusal leaves the blob alone"
    );

    change_passphrase(&keystore, &install_id(), &ark, PASS, OTHER_PASS, TINY, &rng).unwrap();
    let state = locked_ark(&keystore);
    assert!(matches!(
        state.unlock_with_minimum(PASS, &TINY),
        Err(KeysError::WrongPassphrase)
    ));
    assert_eq!(state.unlock_with_minimum(OTHER_PASS, &TINY).unwrap(), ark);
    assert!(keystore.inner.get(&plain()).unwrap().is_none());
}

#[test]
fn change_passphrase_without_a_lock_is_a_wrong_passphrase() {
    let (keystore, ark) = unlocked_keystore();
    let err = change_passphrase(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        OTHER_PASS,
        TINY,
        &SeededRng::new(3),
    )
    .unwrap_err();
    assert!(matches!(err, KeysError::WrongPassphrase), "{err}");
    assert_eq!(unlocked_ark(&keystore), ark);
}

#[test]
fn store_ark_replaces_the_stored_root_key() {
    let (keystore, ark) = unlocked_keystore();
    let rotated = test_support::ark(2);
    assert_ne!(rotated, ark);

    store_ark(&keystore, &install_id(), &rotated, None, &SeededRng::new(3)).unwrap();
    assert_eq!(unlocked_ark(&keystore), rotated);

    store_ark(
        &keystore,
        &install_id(),
        &ark,
        Some((PASS, TINY)),
        &SeededRng::new(4),
    )
    .unwrap();
    store_ark(
        &keystore,
        &install_id(),
        &rotated,
        Some((OTHER_PASS, TINY)),
        &SeededRng::new(5),
    )
    .unwrap();
    let state = locked_ark(&keystore);
    assert_eq!(
        state.unlock_with_minimum(OTHER_PASS, &TINY).unwrap(),
        rotated
    );
    assert!(matches!(
        state.unlock_with_minimum(PASS, &TINY),
        Err(KeysError::WrongPassphrase)
    ));
}

#[test]
fn debug_output_holds_no_key_material() {
    let (keystore, ark) = unlocked_keystore();
    enable_lock(
        &keystore,
        &install_id(),
        &ark,
        PASS,
        TINY,
        &SeededRng::new(2),
    )
    .unwrap();
    let stored = keystore.inner.get(&locked()).unwrap().unwrap();
    let blob = PassphraseWrapped::from_bytes(&stored).unwrap();
    let state = ArkState::Locked(locked_ark(&keystore));

    let text = format!("{state:?} {state:#?}");
    assert!(text.contains("LockedArk"), "{text}");
    assert!(text.contains("memory_kib"), "{text}");
    assert!(
        !text.contains("salt") && !text.contains("wrapped"),
        "{text}"
    );
    assert!(!text.contains(&format!("{:?}", blob.salt)), "{text}");
    assert!(
        !text.contains(&format!("{:?}", blob.wrapped.nonce)),
        "{text}"
    );
    assert!(!text.contains(PASS));

    let unlocked = format!("{:?}", ArkState::Unlocked(ark));
    assert_eq!(unlocked, "Unlocked(Ark(Secret32(<redacted>)))");
}
