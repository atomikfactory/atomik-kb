//! The invite secret, its derivations, the bundles and the invite signatures.

#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use kb_crypto::derive_key;
use kb_types::testing::SeededRng;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The secret `00 01 .. 1f` of the cross-checked vectors.
fn ramp_secret() -> InviteSecret {
    InviteSecret::from_secret(Secret32::from_bytes(core::array::from_fn(|i| {
        u8::try_from(i).unwrap()
    })))
}

fn secret_of(byte: u8) -> InviteSecret {
    InviteSecret::from_secret(Secret32::from_bytes([byte; 32]))
}

fn sk(byte: u8) -> SpaceKey {
    SpaceKey::from_secret(Secret32::from_bytes([byte; 32]))
}

fn head(seq: u64, byte: u8) -> ChainHead {
    ChainHead {
        seq,
        hash: [byte; 32],
    }
}

fn feed(byte: u8) -> FeedId {
    FeedId::from_bytes([byte; 16])
}

fn account(byte: u8) -> AccountId {
    AccountId::from_bytes([byte; 16])
}

fn corrupted(result: &Result<impl core::fmt::Debug, KeysError>) -> bool {
    matches!(result, Err(KeysError::Corrupted(_)))
}

// ----- base32 ------------------------------------------------------------------------------

#[test]
fn base32_vectors() {
    // Computed by hand with the Crockford alphabet 0-9 A-Z without I L O U: 256 bits in groups of
    // five, the last group padded with four zero bits.
    assert_eq!(
        ramp_secret().to_base32(),
        "000G40R40M30E209185GR38E1W8124GK2GAHC5RR34D1P70X3RFG"
    );
    assert_eq!(secret_of(0).to_base32(), "0".repeat(52));
    // 255 one bits are 51 symbols `Z`; the 256th bit and four pad zeros are 10000 = `G`.
    assert_eq!(secret_of(0xff).to_base32(), format!("{}G", "Z".repeat(51)));
    assert_eq!(
        secret_of(0xa5).to_base32(),
        "MPJTB9D5MPJTB9D5MPJTB9D5MPJTB9D5MPJTB9D5MPJTB9D5MPJG"
    );
    for s in [
        ramp_secret(),
        secret_of(0),
        secret_of(0xff),
        secret_of(0x5a),
    ] {
        assert_eq!(s.to_base32().len(), 52);
    }
}

#[test]
fn base32_round_trips() {
    for seed in 0..64u64 {
        let secret = InviteSecret::generate(&SeededRng::new(seed));
        let text = secret.to_base32();
        assert_eq!(text.len(), 52);
        assert!(
            text.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        );
        assert!(!text.contains(['I', 'L', 'O', 'U']));
        assert_eq!(InviteSecret::from_base32(&text).unwrap(), secret);
    }
}

#[test]
fn base32_is_forgiving_like_the_pairing_code() {
    let secret = ramp_secret();
    let text = secret.to_base32();
    // Case, spaces, dashes, line breaks.
    assert_eq!(
        InviteSecret::from_base32(&text.to_ascii_lowercase()).unwrap(),
        secret
    );
    let spaced = text
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-");
    assert_eq!(InviteSecret::from_base32(&spaced).unwrap(), secret);
    assert_eq!(
        InviteSecret::from_base32(&format!("\n {}\t{} \u{2013}", &text[..26], &text[26..]))
            .unwrap(),
        secret,
        "whitespace and dashes (U+2013 is an en dash) are separators"
    );
    // O is 0, I and L are 1.
    let zeros = "0".repeat(52);
    let confusable = "O".repeat(52);
    assert_eq!(
        InviteSecret::from_base32(&confusable).unwrap(),
        InviteSecret::from_base32(&zeros).unwrap()
    );
    // I and L are 1: tested on the first symbol (on the last one `1` would not be canonical).
    let with_one = format!("1{}", "0".repeat(51));
    for variant in ["I", "L", "l", "i"] {
        assert_eq!(
            InviteSecret::from_base32(&format!("{variant}{}", "0".repeat(51))).unwrap(),
            InviteSecret::from_base32(&with_one).unwrap(),
            "{variant}"
        );
    }
}

#[test]
fn base32_refuses_anything_else_without_repeating_it() {
    let good = ramp_secret().to_base32();
    let long = format!("{good}0");
    let short = good[..51].to_owned();
    // The last symbol carries four padding bits that must be zero: `H` is 10001.
    let non_canonical = format!("{}H", &good[..51]);
    let with_u = format!("U{}", &good[1..]);
    let with_symbol = format!("*{}", &good[1..]);
    for bad in [
        String::new(),
        "0".repeat(53),
        long,
        short,
        non_canonical,
        with_u,
        with_symbol,
        "\u{ff10}".repeat(52), // full-width digits are not digits here
    ] {
        let result = InviteSecret::from_base32(&bad);
        assert!(corrupted(&result), "{bad:?}");
        if let Err(error) = result {
            let message = error.to_string();
            assert!(!message.contains(&bad) || bad.is_empty(), "{message}");
        }
    }
}

#[test]
fn a_secret_never_prints() {
    let text = format!("{:?}", secret_of(0xAB));
    assert_eq!(text, "InviteSecret(Secret32(<redacted>))");
}

// ----- derivations -------------------------------------------------------------------------

#[test]
fn the_derivations_follow_the_documented_labels() {
    let s = ramp_secret();
    let id = invite_id(&s);
    let okm = derive_key(s.secret(), kb_crypto::Label::INVITE_ID, b"");
    assert_eq!(id.as_bytes(), &okm.expose()[..16]);
    // RFC 5869 HKDF-SHA256 written out with HMAC in Python (salt "atomik-kb/v1", info
    // "atomik-kb/v1/<label>" 00), ikm 00 01 .. 1f.
    assert_eq!(hex(id.as_bytes()), "27877c8ae786b7bfff6aa3ffebee604a");
    assert_eq!(
        hex(derive_key(s.secret(), kb_crypto::Label::INVITE_SIGN, b"").expose()),
        "6eeb000c01018a44bd0dd13cd406b7982d803336a797c38d1b33c6a9a56a61b3"
    );
    assert_eq!(
        hex(invite_bundle_key(&s).expose()),
        "55d5e4214cc84415537d416a60417de992df27457b02cd4411a7dbab02eddb6d"
    );
    // The signing key is the Ed25519 key of the derived seed.
    let seed = derive_key(s.secret(), kb_crypto::Label::INVITE_SIGN, b"");
    assert_eq!(
        invite_signing_key(&s).verifying_key(),
        SigningKey::from_seed(&seed).verifying_key()
    );
    assert_eq!(invite_signing_key(&s).to_seed(), seed);
}

#[test]
fn the_derivations_are_unrelated_and_secret_specific() {
    let (a, b) = (secret_of(1), secret_of(2));
    assert_ne!(invite_id(&a), invite_id(&b));
    assert_ne!(
        invite_signing_key(&a).verifying_key(),
        invite_signing_key(&b).verifying_key()
    );
    assert_ne!(invite_bundle_key(&a), invite_bundle_key(&b));
    assert_ne!(invite_bundle_key(&a), invite_signing_key(&a).to_seed());
    assert_eq!(invite_id(&a), invite_id(&a));
}

// ----- bundles -----------------------------------------------------------------------------

fn link_bundle() -> LinkBundle {
    LinkBundle {
        epoch: 3,
        space_key: sk(0x42),
        space_name: "Space".to_owned(),
        inviter_name: "Alice".to_owned(),
    }
}

#[test]
fn the_link_bundle_has_the_documented_layout() {
    let mut expected = vec![1u8, 3, 0, 0, 0];
    expected.extend_from_slice(&[0x42; 32]);
    expected.extend_from_slice(&[5, 0, 0, 0]);
    expected.extend_from_slice(b"Space");
    expected.extend_from_slice(&[5, 0, 0, 0]);
    expected.extend_from_slice(b"Alice");
    assert_eq!(link_bundle().encode(), expected);
    assert_eq!(LinkBundle::decode(&expected).unwrap(), link_bundle());
}

#[test]
fn the_link_bundle_decodes_strictly() {
    let bytes = link_bundle().encode();
    for len in 0..bytes.len() {
        assert!(
            corrupted(&LinkBundle::decode(&bytes[..len])),
            "cut at {len}"
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(corrupted(&LinkBundle::decode(&longer)));
    let mut version = bytes.clone();
    version[0] = 2;
    assert!(corrupted(&LinkBundle::decode(&version)));
    // Not UTF-8 in either name.
    let mut bad_name = bytes.clone();
    bad_name[41] = 0xff;
    assert!(corrupted(&LinkBundle::decode(&bad_name)));
    let mut bad_inviter = bytes.clone();
    let last = bad_inviter.len() - 1;
    bad_inviter[last] = 0xff;
    assert!(corrupted(&LinkBundle::decode(&bad_inviter)));
    // A length pointing past the end.
    let mut long_length = bytes;
    long_length[37..41].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(corrupted(&LinkBundle::decode(&long_length)));
}

#[test]
fn a_sealed_link_bundle_opens_with_the_secret_only() {
    let rng = SeededRng::new(1);
    let secret = secret_of(7);
    let sealed = seal_link_bundle(&link_bundle(), &secret, &rng);
    assert_eq!(
        sealed.len(),
        NONCE_LEN + link_bundle().encode().len() + TAG_LEN
    );
    assert_eq!(open_link_bundle(&sealed, &secret).unwrap(), link_bundle());
    // The same bundle seals differently each time.
    assert_ne!(sealed, seal_link_bundle(&link_bundle(), &secret, &rng));

    let wrong = open_link_bundle(&sealed, &secret_of(8));
    assert!(matches!(
        wrong,
        Err(KeysError::Crypto(CryptoError::Authentication))
    ));
    for i in 0..sealed.len() {
        let mut bad = sealed.clone();
        bad[i] ^= 1;
        assert!(open_link_bundle(&bad, &secret).is_err(), "byte {i}");
    }
    for len in [0, 10, NONCE_LEN, NONCE_LEN + TAG_LEN - 1] {
        assert!(
            matches!(
                open_link_bundle(&sealed[..len], &secret),
                Err(KeysError::Crypto(CryptoError::Encoding(_)))
            ),
            "{len}"
        );
    }
}

#[test]
fn the_sealed_link_bundle_uses_the_documented_key_and_aad() {
    // Spelled out: nonce, then XChaCha20-Poly1305 under HKDF(s, "invite-bundle") with the invite id
    // as AAD.
    let rng = SeededRng::new(2);
    let secret = ramp_secret();
    let sealed = seal_link_bundle(&link_bundle(), &secret, &rng);
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let key = derive_key(secret.secret(), kb_crypto::Label::INVITE_BUNDLE, b"");
    let id = [
        0x27, 0x87, 0x7c, 0x8a, 0xe7, 0x86, 0xb7, 0xbf, 0xff, 0x6a, 0xa3, 0xff, 0xeb, 0xee, 0x60,
        0x4a,
    ];
    let plaintext = kb_crypto::aead_open(
        &key,
        &Nonce24::from_bytes(nonce.try_into().unwrap()),
        &id,
        ciphertext,
    )
    .unwrap();
    assert_eq!(plaintext.as_slice(), link_bundle().encode().as_slice());
    // Another AAD does not open it.
    assert!(
        kb_crypto::aead_open(
            &key,
            &Nonce24::from_bytes(nonce.try_into().unwrap()),
            &[0u8; 16],
            ciphertext
        )
        .is_err()
    );
}

fn code_bundle() -> CodeBundle {
    CodeBundle {
        feed_id: feed(0xf1),
        space_id: SpaceId::from_bytes([0x5a; 16]),
        inviter: account(0xa1),
        secret: secret_of(0x11),
        epoch: 9,
        space_key: sk(0x22),
        membership_head: head(4, 0x33),
        space_name: "Plans".to_owned(),
    }
}

#[test]
fn the_code_bundle_has_the_documented_layout() {
    let mut expected = vec![1u8];
    expected.extend_from_slice(&[0xf1; 16]);
    expected.extend_from_slice(&[0x5a; 16]);
    expected.extend_from_slice(&[0xa1; 16]);
    expected.extend_from_slice(&[0x11; 32]);
    expected.extend_from_slice(&[9, 0, 0, 0]);
    expected.extend_from_slice(&[0x22; 32]);
    expected.extend_from_slice(&[4, 0, 0, 0, 0, 0, 0, 0]);
    expected.extend_from_slice(&[0x33; 32]);
    expected.extend_from_slice(&[5, 0, 0, 0]);
    expected.extend_from_slice(b"Plans");
    assert_eq!(code_bundle().encode(), expected);
    assert_eq!(CodeBundle::decode(&expected).unwrap(), code_bundle());
}

#[test]
fn the_code_bundle_decodes_strictly() {
    let bytes = code_bundle().encode();
    for len in 0..bytes.len() {
        assert!(
            corrupted(&CodeBundle::decode(&bytes[..len])),
            "cut at {len}"
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(corrupted(&CodeBundle::decode(&longer)));
    let mut version = bytes.clone();
    version[0] = 0;
    assert!(corrupted(&CodeBundle::decode(&version)));
    let mut bad_name = bytes;
    let last = bad_name.len() - 1;
    bad_name[last] = 0xfe;
    assert!(corrupted(&CodeBundle::decode(&bad_name)));
}

// ----- signatures --------------------------------------------------------------------------

fn invite_pair(byte: u8) -> (SigningKey, VerifyingKey, InviteId) {
    let secret = secret_of(byte);
    let key = invite_signing_key(&secret);
    let public = key.verifying_key();
    (key, public, invite_id(&secret))
}

#[test]
fn a_join_proof_binds_every_field() {
    let (key, public, id) = invite_pair(1);
    let log = head(5, 0x77);
    let proof = sign_join_proof(&key, feed(1), id, account(2), &log);
    verify_join_proof(&public, feed(1), id, account(2), &log, &proof).unwrap();

    let (_, other_public, other_id) = invite_pair(2);
    let cases: [(&str, Result<(), KeysError>); 7] = [
        (
            "feed",
            verify_join_proof(&public, feed(9), id, account(2), &log, &proof),
        ),
        (
            "invite",
            verify_join_proof(&public, feed(1), other_id, account(2), &log, &proof),
        ),
        (
            "account",
            verify_join_proof(&public, feed(1), id, account(3), &log, &proof),
        ),
        (
            "seq",
            verify_join_proof(&public, feed(1), id, account(2), &head(6, 0x77), &proof),
        ),
        (
            "hash",
            verify_join_proof(&public, feed(1), id, account(2), &head(5, 0x78), &proof),
        ),
        (
            "key",
            verify_join_proof(&other_public, feed(1), id, account(2), &log, &proof),
        ),
        (
            "bytes",
            verify_join_proof(
                &public,
                feed(1),
                id,
                account(2),
                &log,
                &Signature::from_bytes(&[0u8; 64]).unwrap(),
            ),
        ),
    ];
    for (what, result) in cases {
        assert!(
            matches!(result, Err(KeysError::Crypto(CryptoError::Authentication))),
            "{what}"
        );
    }
}

#[test]
fn the_signed_messages_are_the_documented_ones() {
    let (key, public, id) = invite_pair(3);
    let epoch = [0xee; 16];

    // join proof: feed ‖ invite_id ‖ account ‖ u64 seq LE ‖ hash
    let log = head(0x0102, 0x55);
    let proof = sign_join_proof(&key, feed(0xf0), id, account(0xac), &log);
    let mut message = vec![0xf0; 16];
    message.extend_from_slice(id.as_bytes());
    message.extend_from_slice(&[0xac; 16]);
    message.extend_from_slice(&[0x02, 0x01, 0, 0, 0, 0, 0, 0]);
    message.extend_from_slice(&[0x55; 32]);
    kb_crypto::verify(&public, "invite", &message, &proof).unwrap();

    // signup: "signup" ‖ account ‖ server_epoch
    let signup = sign_signup(&key, account(0xac), epoch);
    let mut message = b"signup".to_vec();
    message.extend_from_slice(&[0xac; 16]);
    message.extend_from_slice(&epoch);
    kb_crypto::verify(&public, "invite", &message, &signup).unwrap();

    // open: "open" ‖ invite_id ‖ server_epoch
    let open = sign_open(&key, id, epoch);
    let mut message = b"open".to_vec();
    message.extend_from_slice(id.as_bytes());
    message.extend_from_slice(&epoch);
    kb_crypto::verify(&public, "invite", &message, &open).unwrap();
}

#[test]
fn signup_and_open_signatures_bind_their_fields_and_do_not_cross() {
    let (key, public, id) = invite_pair(4);
    let epoch = [1; 16];
    let signup = sign_signup(&key, account(5), epoch);
    verify_signup(&public, account(5), epoch, &signup).unwrap();
    assert!(verify_signup(&public, account(6), epoch, &signup).is_err());
    assert!(verify_signup(&public, account(5), [2; 16], &signup).is_err());
    assert!(verify_signup(&invite_pair(5).1, account(5), epoch, &signup).is_err());

    let open = sign_open(&key, id, epoch);
    verify_open(&public, id, epoch, &open).unwrap();
    assert!(verify_open(&public, InviteId::from_bytes([9; 16]), epoch, &open).is_err());
    assert!(verify_open(&public, id, [2; 16], &open).is_err());
    assert!(verify_open(&invite_pair(5).1, id, epoch, &open).is_err());

    // A signature of one purpose does not verify for another, even with the same bytes: the
    // messages differ by their fixed prefix and the join proof by its length.
    assert!(verify_open(&public, id, epoch, &signup).is_err());
    assert!(verify_signup(&public, account(5), epoch, &open).is_err());
    let join = sign_join_proof(&key, feed(1), id, account(5), &head(0, 0));
    assert!(verify_open(&public, id, epoch, &join).is_err());
    assert!(verify_signup(&public, account(5), epoch, &join).is_err());
}

#[test]
fn invite_signatures_do_not_verify_under_another_context() {
    let (key, public, id) = invite_pair(6);
    let mut message = b"open".to_vec();
    message.extend_from_slice(id.as_bytes());
    message.extend_from_slice(&[1; 16]);
    let other_context = kb_crypto::sign(&key, "device-log", &message);
    assert!(verify_open(&public, id, [1; 16], &other_context).is_err());
    assert_eq!(INVITE_CONTEXT, "invite");
    assert_eq!(
        format!("atomik-kb/v1/{INVITE_CONTEXT}"),
        kb_types::sharing::INVITE_TRANSCRIPT_DOMAIN
    );
}

// ----- properties ----------------------------------------------------------------------------

mod props {
    use super::*;
    use proptest::prelude::*;

    fn text() -> impl Strategy<Value = String> {
        r"\PC{0,24}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn a_secret_survives_its_link_form(bytes in any::<[u8; 32]>()) {
            let secret = InviteSecret::from_secret(Secret32::from_bytes(bytes));
            let text = secret.to_base32();
            prop_assert_eq!(text.len(), 52);
            prop_assert_eq!(InviteSecret::from_base32(&text).unwrap(), secret);
        }

        #[test]
        fn typed_text_decodes_only_to_its_own_canonical_form(typed in r"\PC{0,70}") {
            if let Ok(secret) = InviteSecret::from_base32(&typed) {
                // Whatever was accepted is the same 256 bits as its canonical spelling.
                prop_assert_eq!(InviteSecret::from_base32(&secret.to_base32()).unwrap(), secret);
            }
        }

        #[test]
        fn link_bundles_round_trip_and_decode_only_canonically(
            epoch in any::<u32>(),
            key in any::<[u8; 32]>(),
            space_name in text(),
            inviter_name in text(),
            noise in proptest::collection::vec(any::<u8>(), 0..200),
        ) {
            let bundle = LinkBundle {
                epoch,
                space_key: SpaceKey::from_secret(Secret32::from_bytes(key)),
                space_name,
                inviter_name,
            };
            let bytes = bundle.encode();
            prop_assert_eq!(LinkBundle::decode(&bytes).unwrap(), bundle);
            if let Ok(decoded) = LinkBundle::decode(&noise) {
                prop_assert_eq!(decoded.encode(), noise);
            }
        }

        #[test]
        fn code_bundles_round_trip_and_decode_only_canonically(
            parts in (any::<[u8; 16]>(), any::<[u8; 16]>(), any::<[u8; 16]>()),
            secrets in (any::<[u8; 32]>(), any::<[u8; 32]>(), any::<[u8; 32]>()),
            epoch in any::<u32>(),
            seq in any::<u64>(),
            name in text(),
            noise in proptest::collection::vec(any::<u8>(), 0..300),
        ) {
            let bundle = CodeBundle {
                feed_id: FeedId::from_bytes(parts.0),
                space_id: SpaceId::from_bytes(parts.1),
                inviter: AccountId::from_bytes(parts.2),
                secret: InviteSecret::from_secret(Secret32::from_bytes(secrets.0)),
                epoch,
                space_key: SpaceKey::from_secret(Secret32::from_bytes(secrets.1)),
                membership_head: ChainHead { seq, hash: secrets.2 },
                space_name: name,
            };
            prop_assert_eq!(CodeBundle::decode(&bundle.encode()).unwrap(), bundle);
            if let Ok(decoded) = CodeBundle::decode(&noise) {
                prop_assert_eq!(decoded.encode(), noise);
            }
        }
    }
}
