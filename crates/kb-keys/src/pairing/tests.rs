//! Pairing: the code, the key agreement and its digits, the sealed messages, the
//! identity and the bundle (ADR-0022).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::log::{DeviceLog, encrypt_name};
use crate::recovery::{RecoveryBlob, RecoveryKey};
use crate::rotation::{RotationBox, seal_root_for};
use crate::test_support::{ark, device};
use kb_crypto::{Envelope, SigningKey};
use kb_types::devices::PAIRING_MAX_MESSAGE_BYTES;
use kb_types::testing::SeededRng;
use proptest::prelude::*;

const SERVER: &str = "https://sync.example.com";
const MAILBOX: [u8; MAILBOX_ID_LEN] = [1, 2, 3, 4, 5, 6];
const T0: i64 = 1_760_000_000_000;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A joiner (ephemeral key from `SeededRng::new(joiner_seed)`) and a sponsor (from
/// `sponsor_seed`) that completed offer, answer and reveal over `mailbox`.
fn pair_with(joiner_seed: u64, sponsor_seed: u64, mailbox: [u8; 6]) -> (Session, Session) {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(joiner_seed));
    joiner.set_mailbox(mailbox);
    let (mut sponsor, answer) =
        Session::sponsor(mailbox, &offer, &SeededRng::new(sponsor_seed)).unwrap();
    let reveal = joiner.accept_answer(&answer).unwrap();
    sponsor.accept_reveal(&reveal).unwrap();
    (joiner, sponsor)
}

/// `BLAKE3("atomik-kb/v1/pairing-commit" || key)`, written out.
fn commit(key: &AgreementPublic) -> [u8; 32] {
    let mut input = b"atomik-kb/v1/pairing-commit".to_vec();
    input.extend_from_slice(&key.to_bytes());
    kb_crypto::hash(&input)
}

fn pair() -> (Session, Session) {
    pair_with(1, 2, MAILBOX)
}

/// The ephemeral key a session draws from `SeededRng::new(seed)`.
fn ephemeral(seed: u64) -> AgreementSecret {
    AgreementSecret::from_seed(&Secret32::generate(&SeededRng::new(seed)))
}

fn is_auth<T>(result: Result<T, KeysError>) -> bool {
    matches!(result, Err(KeysError::Crypto(CryptoError::Authentication)))
}

fn is_encoding<T>(result: Result<T, KeysError>) -> bool {
    matches!(result, Err(KeysError::Crypto(CryptoError::Encoding(_))))
}

fn is_corrupted<T>(result: Result<T, KeysError>) -> bool {
    matches!(result, Err(KeysError::Corrupted(_)))
}

fn channel_key(session: &Session) -> &Secret32 {
    session.channel_key.as_ref().unwrap()
}

// ---------------------------------------------------------------------------------
// The code

#[test]
fn a_code_is_twelve_crockford_symbols_and_every_typed_form_decodes() {
    let ids = [
        [0u8; 6],
        [0xff; 6],
        MAILBOX,
        [0x08, 0x42, 0x10, 0x84, 0x21, 0x08],
    ];
    for id in ids {
        let code = PairingCode { mailbox_id: id };
        for server in [SERVER, "http://127.0.0.1:8787", ""] {
            let text = code.encode(server);
            assert_eq!(text.len(), PAIRING_CODE_LEN);
            assert!(text.bytes().all(|b| CROCKFORD.contains(&b)), "{text}");
            assert_eq!(PairingCode::decode(&text, server).unwrap(), code);

            let (a, b, c) = (&text[..4], &text[4..8], &text[8..]);
            let shown = code.display(server);
            assert_eq!(shown, format!("{a} {b} {c}"));
            for typed in [
                shown.clone(),
                text.to_lowercase(),
                format!("{a}-{b}-{c}"),
                format!("  {a}\t{b}\n{c} "),
                format!("{a}\u{2013}{b}\u{2014}{c}"),
                format!("{a}\u{00A0}{b}\u{2212}{c}"),
                format!("{}-", text.to_lowercase()),
            ] {
                assert_eq!(
                    PairingCode::decode(&typed, server).unwrap(),
                    code,
                    "{typed:?}"
                );
            }
        }
    }
}

#[test]
fn confusable_letters_are_read_as_digits() {
    let zeros = PairingCode { mailbox_id: [0; 6] };
    let ones = PairingCode {
        mailbox_id: [0x08, 0x42, 0x10, 0x84, 0x21, 0x08],
    };
    let zero_text = zeros.encode(SERVER);
    let one_text = ones.encode(SERVER);
    assert!(zero_text.starts_with("000000000"), "{zero_text}");
    assert!(one_text.starts_with("111111111"), "{one_text}");
    for letter in ["O", "o"] {
        let typed = zero_text.replace('0', letter);
        assert_eq!(PairingCode::decode(&typed, SERVER).unwrap(), zeros);
    }
    for letter in ["I", "i", "L", "l"] {
        let typed = one_text.replace('1', letter);
        assert_eq!(PairingCode::decode(&typed, SERVER).unwrap(), ones);
    }
}

#[test]
fn a_code_has_the_documented_layout() {
    let code = PairingCode {
        mailbox_id: MAILBOX,
    };
    let mut input = PAIRING_CODE_DOMAIN.as_bytes().to_vec();
    input.extend_from_slice(SERVER.as_bytes());
    input.extend_from_slice(&MAILBOX);
    let digest = kb_crypto::hash(&input);
    let checksum = (u64::from(digest[0]) << 4) | (u64::from(digest[1]) >> 4);
    let value = (0x0102_0304_0506u64 << 12) | checksum;
    let expected: String = (0..12)
        .map(|i| char::from(CROCKFORD[usize::try_from((value >> (55 - 5 * i)) & 31).unwrap()]))
        .collect();
    assert_eq!(code.encode(SERVER), expected);

    // Frozen: a change here strands every code shown by an older build.
    assert_eq!(code.encode(SERVER), "041061050SDF");
    assert_eq!(code.display(SERVER), "0410 6105 0SDF");
    assert_eq!(
        PairingCode {
            mailbox_id: [0xff; 6]
        }
        .encode("http://127.0.0.1:8787"),
        "ZZZZZZZZZX4C"
    );
}

#[test]
fn malformed_codes_are_refused_without_echoing_them() {
    let good = PairingCode {
        mailbox_id: MAILBOX,
    }
    .encode(SERVER);
    let cases = [
        String::new(),
        "   ".to_owned(),
        "- -".to_owned(),
        good[..11].to_owned(),
        format!("{good}0"),
        format!("{good}{good}"),
        format!("{}U", &good[..11]),
        format!("{}u", &good[..11]),
        format!("{}.", &good[..11]),
        format!("{}_{}", &good[..6], &good[6..]),
        format!("{}\u{00E9}{}", &good[..6], &good[7..]),
        format!("{}\u{FF10}{}", &good[..6], &good[7..]),
        format!("{}\u{1F4F1}{}", &good[..6], &good[7..]),
        format!("{}+{}", &good[..6], &good[7..]),
        format!("{}/{}", &good[..6], &good[7..]),
    ];
    for text in cases {
        let err = PairingCode::decode(&text, SERVER).unwrap_err();
        assert!(matches!(err, KeysError::Corrupted(_)), "{text:?}: {err:?}");
        let message = err.to_string();
        assert!(!message.contains(&good[..4]), "{message}");
        assert!(!message.contains(&good[8..]), "{message}");
    }
}

#[test]
fn a_code_for_another_server_is_refused() {
    let code = PairingCode {
        mailbox_id: MAILBOX,
    };
    let text = code.encode(SERVER);
    for other in [
        "https://sync.example.org",
        "http://sync.example.com",
        "https://sync.example.com:8443",
        "https://sync.example.com/v1",
        "",
    ] {
        assert_ne!(
            checksum(&MAILBOX, other),
            checksum(&MAILBOX, SERVER),
            "{other}"
        );
        let err = PairingCode::decode(&text, other).unwrap_err();
        assert_eq!(
            err.to_string(),
            "corrupted key material: this code is not for this server, or it was mistyped",
            "{other}"
        );
    }
    assert_eq!(PairingCode::decode(&text, SERVER).unwrap(), code);
}

#[test]
fn the_server_address_is_canonicalised_on_both_sides() {
    let code = PairingCode {
        mailbox_id: MAILBOX,
    };
    let text = code.encode(SERVER);
    for same in [
        "https://sync.example.com/",
        "https://sync.example.com///",
        "HTTPS://Sync.Example.COM",
        "  https://sync.example.com/ \n",
        "\thttps://SYNC.example.com/",
    ] {
        assert_eq!(canonical_server_url(same), SERVER, "{same:?}");
        assert_eq!(code.encode(same), text, "{same:?}");
        assert_eq!(PairingCode::decode(&text, same).unwrap(), code, "{same:?}");
        assert_eq!(
            PairingCode::decode(&code.encode(same), SERVER).unwrap(),
            code
        );
    }
    // Only ASCII letters are lowercased; the path and the port are kept.
    assert_eq!(
        canonical_server_url("https://\u{00C9}x.example/Path/"),
        "https://\u{00C9}x.example/path"
    );
    assert_eq!(
        canonical_server_url("http://127.0.0.1:8787/"),
        "http://127.0.0.1:8787"
    );
}

#[test]
fn single_symbol_typos_and_swaps_are_caught() {
    let text = PairingCode {
        mailbox_id: MAILBOX,
    }
    .encode(SERVER);
    let original = text.as_bytes();
    let mut tried = 0;
    for (position, &current) in original.iter().enumerate() {
        for &symbol in CROCKFORD {
            if symbol == current {
                continue;
            }
            let mut typo = original.to_vec();
            typo[position] = symbol;
            tried += 1;
            let typo = String::from_utf8(typo).unwrap();
            assert!(
                is_corrupted(PairingCode::decode(&typo, SERVER)),
                "{typo} slipped through"
            );
        }
    }
    assert_eq!(tried, PAIRING_CODE_LEN * 31);
    for (position, neighbours) in original.windows(2).enumerate() {
        if neighbours[0] == neighbours[1] {
            continue;
        }
        let mut swapped = original.to_vec();
        swapped.swap(position, position + 1);
        let swapped = String::from_utf8(swapped).unwrap();
        assert!(
            is_corrupted(PairingCode::decode(&swapped, SERVER)),
            "{swapped}"
        );
    }
}

#[test]
fn the_mailbox_id_round_trips_through_hex() {
    let code = PairingCode {
        mailbox_id: [0x0a, 0xbc, 0x01, 0xff, 0x00, 0x9e],
    };
    assert_eq!(code.mailbox_hex(), "0abc01ff009e");
    assert_eq!(PairingCode::from_mailbox_hex("0abc01ff009e").unwrap(), code);
    assert_eq!(PairingCode::from_mailbox_hex("0ABC01FF009E").unwrap(), code);
    for bad in [
        "",
        "0abc01ff009",
        "0abc01ff009e0",
        "0abc01ff009g",
        " abc01ff009e",
        "0abc01ff00\u{00E9}",
        "0x0abc01ff00",
        "+abc01ff009e",
    ] {
        assert!(is_corrupted(PairingCode::from_mailbox_hex(bad)), "{bad:?}");
    }
}

// ---------------------------------------------------------------------------------
// The key agreement

#[test]
fn offer_answer_and_reveal_have_the_documented_layout() {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    let e_joiner = ephemeral(1).public();
    assert_eq!(offer.len(), 34);
    assert_eq!(offer[..2], [1, 0]);
    assert_eq!(offer[2..], commit(&e_joiner), "a commitment, not the key");
    assert_ne!(offer[2..], e_joiner.to_bytes());
    assert_eq!(joiner.role(), Role::Opener);
    assert_eq!(joiner.mailbox_id(), None);
    assert!(!joiner.agreed());
    assert_eq!(joiner.sas(), None);

    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    assert_eq!(answer.len(), 34);
    assert_eq!(answer[..2], [1, 1]);
    assert_eq!(answer[2..], ephemeral(2).public().to_bytes());
    assert_eq!(
        hex(&answer[2..]),
        "8a47dd8ef6571bd3e8ff8eab1d1d9bddc673faf6b7699538c30710c099014311"
    );
    assert_eq!(sponsor.role(), Role::Typist);
    assert_eq!(sponsor.mailbox_id(), Some(MAILBOX));
    assert!(!sponsor.agreed(), "the sponsor waits for the reveal");
    assert_eq!(sponsor.sas(), None);

    joiner.set_mailbox(MAILBOX);
    let reveal = joiner.accept_answer(&answer).unwrap();
    assert_eq!(reveal.len(), 34);
    assert_eq!(reveal[..2], [1, 0]);
    assert_eq!(reveal[2..], e_joiner.to_bytes());
    assert_eq!(
        hex(&reveal[2..]),
        "40e913b735c3f408e7c85ca9bfbb5af8196911ea249fe96d4e96abafcbaa1d38"
    );
    assert!(joiner.agreed());
    assert!(joiner.sas().is_some());
    assert!(!sponsor.agreed());

    sponsor.accept_reveal(&reveal).unwrap();
    assert!(sponsor.agreed());
    assert_eq!(sponsor.sas(), joiner.sas());
}

/// Cross-checked with an independent implementation (Python `cryptography`: X25519
/// and HKDF-SHA256 with the label framing of `kb_crypto::derive_key`, the seeded
/// generator re-implemented).
#[test]
fn sas_and_channel_key_vectors() {
    for (joiner_seed, sponsor_seed, mailbox, sas, channel) in [
        (
            1,
            2,
            MAILBOX,
            "867987",
            "ba5308f9156e70bf5b812d7113fd2823d459319aafc7e8673a37c917f41cbb9f",
        ),
        (
            3,
            4,
            [0; 6],
            "554068",
            "97629c08ffa8ad4b945308c80ed057be482793db74bc9c02be39121ee6e8589f",
        ),
        (
            7,
            8,
            [0xff; 6],
            "919631",
            "5ea19593f81f8f0ef3e34de2577bebaf76e94e5d7a097bcf7a5cbc6f1506493a",
        ),
        (
            29,
            1029,
            MAILBOX,
            "009798",
            "0f7acd52e640a6048458d74ad08ce92eb7007e83bf8c62625b9e90032207407b",
        ),
    ] {
        let (joiner, sponsor) = pair_with(joiner_seed, sponsor_seed, mailbox);
        assert_eq!(joiner.sas().as_deref(), Some(sas));
        assert_eq!(sponsor.sas().as_deref(), Some(sas));
        assert_eq!(hex(channel_key(&joiner).expose()), channel);
        assert_eq!(hex(channel_key(&sponsor).expose()), channel);
    }
}

#[test]
fn the_digits_and_the_channel_key_follow_the_documented_construction() {
    let (joiner, sponsor) = pair();
    let (j, s) = (ephemeral(1), ephemeral(2));
    let shared = kb_crypto::shared_secret(&j, &s.public()).unwrap();
    let mut t = b"atomik-kb/v1/pairing".to_vec();
    t.extend_from_slice(&MAILBOX);
    t.extend_from_slice(&j.public().to_bytes());
    t.extend_from_slice(&s.public().to_bytes());

    let sas_key = kb_crypto::derive_key(&shared, Label::PAIRING_SAS, &t);
    let n = u32::from_be_bytes(sas_key.expose()[..4].try_into().unwrap()) % 1_000_000;
    assert_eq!(joiner.sas().unwrap(), format!("{n:06}"));

    let channel = kb_crypto::derive_key(&shared, Label::PAIRING_CHANNEL, &t);
    assert_eq!(channel_key(&joiner), &channel);
    assert_eq!(channel_key(&sponsor), &channel);
    assert_ne!(channel, sas_key);
    assert_ne!(&channel, &shared);
    // A message sealed by hand under that key opens.
    let sealed = kb_crypto::seal_envelope(
        EnvelopeKind::Pairing,
        &[1, 7, 0, 0, 0, 2],
        &channel,
        b"hello",
        None,
        &SeededRng::new(5),
    );
    let (kind, plaintext) = joiner.open(&sealed, 7).unwrap();
    assert_eq!(kind, MessageKind::Bundle);
    assert_eq!(plaintext.as_slice(), b"hello");
}

#[test]
fn both_sides_show_the_same_digits_and_talk_both_ways() {
    for seed in 0..16u64 {
        let (joiner, sponsor) = pair_with(seed, seed + 100, [u8::try_from(seed).unwrap(); 6]);
        let digits = joiner.sas().unwrap();
        assert_eq!(sponsor.sas().unwrap(), digits);
        assert_eq!(digits.len(), SAS_DIGITS);
        assert!(digits.bytes().all(|b| b.is_ascii_digit()), "{digits}");

        let rng = SeededRng::new(seed);
        let to_sponsor = joiner.seal(MessageKind::Identity, 0, b"identity", &rng);
        let (kind, plaintext) = sponsor.open(&to_sponsor, 0).unwrap();
        assert_eq!(
            (kind, plaintext.as_slice()),
            (MessageKind::Identity, &b"identity"[..])
        );
        let to_joiner = sponsor.seal(MessageKind::Bundle, 0, b"bundle", &rng);
        let (kind, plaintext) = joiner.open(&to_joiner, 0).unwrap();
        assert_eq!(
            (kind, plaintext.as_slice()),
            (MessageKind::Bundle, &b"bundle"[..])
        );
    }
}

#[test]
fn a_relay_that_swaps_an_ephemeral_key_changes_the_digits_and_cannot_read() {
    let rng = SeededRng::new(9);

    // A full man in the middle: the relay answers the joiner and offers to the
    // sponsor with keys of its own.
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    let (mut relay_as_joiner, relay_offer) = Session::joiner(&SeededRng::new(66));
    relay_as_joiner.set_mailbox(MAILBOX);
    let (mut sponsor, answer) =
        Session::sponsor(MAILBOX, &relay_offer, &SeededRng::new(2)).unwrap();
    let (mut relay_as_sponsor, relay_answer) =
        Session::sponsor(MAILBOX, &offer, &SeededRng::new(67)).unwrap();
    let reveal = joiner.accept_answer(&relay_answer).unwrap();
    relay_as_sponsor.accept_reveal(&reveal).unwrap();
    let relay_reveal = relay_as_joiner.accept_answer(&answer).unwrap();
    sponsor.accept_reveal(&relay_reveal).unwrap();
    assert_eq!(joiner.sas(), relay_as_sponsor.sas());
    assert_eq!(sponsor.sas(), relay_as_joiner.sas());
    assert_ne!(
        joiner.sas(),
        sponsor.sas(),
        "the user sees two different numbers"
    );
    // Forwarding a message unchanged gets nowhere.
    let identity = joiner.seal(MessageKind::Identity, 3, b"identity", &rng);
    assert!(is_auth(sponsor.open(&identity, 3)));
    let bundle = sponsor.seal(MessageKind::Bundle, 4, b"bundle", &rng);
    assert!(is_auth(joiner.open(&bundle, 4)));

    // Only the answer's key replaced.
    let relay_key = ephemeral(66).public();
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    let mut swapped = answer.clone();
    swapped[2..].copy_from_slice(&relay_key.to_bytes());
    let reveal = joiner.accept_answer(&swapped).unwrap();
    sponsor.accept_reveal(&reveal).unwrap();
    assert_ne!(joiner.sas(), sponsor.sas());
    assert!(is_auth(
        sponsor.open(&joiner.seal(MessageKind::Identity, 3, b"x", &rng), 3)
    ));
    assert!(is_auth(
        joiner.open(&sponsor.seal(MessageKind::Bundle, 4, b"x", &rng), 4)
    ));

    // The reveal replaced: it no longer matches the commitment.
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    let mut reveal = joiner.accept_answer(&answer).unwrap();
    reveal[2..].copy_from_slice(&relay_key.to_bytes());
    assert!(is_auth(sponsor.accept_reveal(&reveal)));
    assert!(!sponsor.agreed());

    // The offer's commitment replaced: the joiner's genuine reveal is refused, so the
    // relay has to play the joiner itself (the full man in the middle above).
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    let mut swapped = offer.clone();
    swapped[2..].copy_from_slice(&commit(&relay_key));
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &swapped, &SeededRng::new(2)).unwrap();
    let reveal = joiner.accept_answer(&answer).unwrap();
    assert!(is_auth(sponsor.accept_reveal(&reveal)));
    assert!(!sponsor.agreed());
}

/// Why the commitment is there: without it a relay that learnt the sponsor's digits
/// could try answer keys until the joiner's digits match (about a million X25519
/// operations). With it, every key is fixed before the key it is combined with is
/// known: the sponsor answers before it sees the joiner's key, the joiner's key is
/// bound by the offer, and the joiner takes one answer only.
#[test]
fn no_side_can_choose_its_key_after_seeing_the_other() {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    // The offer does not carry the key, only its commitment.
    assert_ne!(offer[2..], ephemeral(1).public().to_bytes());

    // The sponsor's answer and digits do not exist before the reveal.
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    assert_eq!(sponsor.sas(), None);
    assert!(
        sponsor
            .seal(MessageKind::Bundle, 4, b"x", &SeededRng::new(9))
            .is_empty()
    );

    // The joiner's key cannot change after the answer: only the committed one opens.
    let reveal = joiner.accept_answer(&answer).unwrap();
    for other_seed in [3u64, 4, 5] {
        let mut other = reveal.clone();
        other[2..].copy_from_slice(&ephemeral(other_seed).public().to_bytes());
        let mut fresh_sponsor = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2))
            .unwrap()
            .0;
        assert!(is_auth(fresh_sponsor.accept_reveal(&other)));
    }
    // And the joiner's sponsor key cannot change after the reveal.
    let (_, other_answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(3)).unwrap();
    let digits = joiner.sas();
    assert!(is_corrupted(joiner.accept_answer(&other_answer)));
    assert_eq!(joiner.sas(), digits);

    sponsor.accept_reveal(&reveal).unwrap();
    assert_eq!(sponsor.sas(), digits);
}

#[test]
fn the_transcript_binds_the_mailbox() {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox([9; 6]);
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    let reveal = joiner.accept_answer(&answer).unwrap();
    sponsor.accept_reveal(&reveal).unwrap();
    assert_ne!(joiner.sas(), sponsor.sas());
    let rng = SeededRng::new(9);
    assert!(is_auth(
        sponsor.open(&joiner.seal(MessageKind::Identity, 3, b"x", &rng), 3)
    ));

    // The mailbox is fixed once agreed.
    joiner.set_mailbox(MAILBOX);
    assert_eq!(joiner.mailbox_id(), Some([9; 6]));
    sponsor.set_mailbox([9; 6]);
    assert_eq!(sponsor.mailbox_id(), Some(MAILBOX));
}

#[test]
fn malformed_offers_are_refused() {
    let (_, offer) = Session::joiner(&SeededRng::new(1));
    let rng = SeededRng::new(2);
    let sponsor = |bytes: &[u8]| Session::sponsor(MAILBOX, bytes, &rng).map(|_| ());

    for len in [0, 1, 2, 33] {
        assert!(is_corrupted(sponsor(&offer[..len])), "{len} bytes");
    }
    let mut longer = offer.clone();
    longer.push(0);
    assert!(is_corrupted(sponsor(&longer)));
    for version in [0u8, 2, 0xff] {
        let mut other = offer.clone();
        other[0] = version;
        let err = sponsor(&other).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("corrupted key material: unsupported pairing version {version}")
        );
    }
    for role in [1u8, 2, 0xff] {
        let mut other = offer.clone();
        other[1] = role;
        assert!(is_corrupted(sponsor(&other)), "role {role}");
    }
    // Any 32 bytes are a commitment: they are checked by the reveal.
    for fill in [0x00u8, 0xff] {
        let mut other = offer.clone();
        other[2..].fill(fill);
        assert!(sponsor(&other).is_ok(), "commitment filled with {fill:#x}");
    }
}

#[test]
fn malformed_reveals_are_refused_and_leave_the_sponsor_waiting() {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    let reveal = joiner.accept_answer(&answer).unwrap();

    let mut new_version = reveal.clone();
    new_version[0] = 2;
    let mut from_sponsor = reveal.clone();
    from_sponsor[1] = 1;
    for (bad, what) in [
        (reveal[..33].to_vec(), "truncated"),
        ([reveal.as_slice(), &[0]].concat(), "trailing byte"),
        (new_version, "version"),
        (from_sponsor, "role"),
        (answer.clone(), "the answer reflected"),
    ] {
        assert!(is_corrupted(sponsor.accept_reveal(&bad)), "{what}");
        assert!(!sponsor.agreed());
    }
    let mut zero_key = reveal.clone();
    zero_key[2..].fill(0);
    assert!(is_encoding(sponsor.accept_reveal(&zero_key)), "low order");
    let mut own_key = reveal.clone();
    own_key[2..].copy_from_slice(&answer[2..]);
    assert!(is_auth(sponsor.accept_reveal(&own_key)), "its own key");
    // The commitment as a key: not a valid point, or not the committed key.
    assert!(
        matches!(sponsor.accept_reveal(&offer), Err(KeysError::Crypto(_))),
        "the commitment itself"
    );
    assert!(!sponsor.agreed());
    assert_eq!(sponsor.sas(), None);

    sponsor.accept_reveal(&reveal).unwrap();
    assert_eq!(sponsor.sas(), joiner.sas());
    let err = sponsor.accept_reveal(&reveal).unwrap_err();
    assert_eq!(
        err.to_string(),
        "corrupted key material: the pairing reveal was already taken"
    );
}

#[test]
fn malformed_answers_are_refused_and_leave_the_joiner_waiting() {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();

    let mut new_version = answer.clone();
    new_version[0] = 2;
    let mut from_joiner = answer.clone();
    from_joiner[1] = 0;
    let mut zero_key = answer.clone();
    zero_key[2..].fill(0);
    let mut own_key = answer.clone();
    own_key[2..].copy_from_slice(&ephemeral(1).public().to_bytes());

    for (bad, what) in [
        (answer[..33].to_vec(), "truncated"),
        ([answer.as_slice(), &[0]].concat(), "trailing byte"),
        (new_version, "version"),
        (from_joiner, "role"),
        (offer.clone(), "the offer reflected"),
    ] {
        assert!(is_corrupted(joiner.accept_answer(&bad)), "{what}");
        assert!(!joiner.agreed());
    }
    assert!(is_encoding(joiner.accept_answer(&zero_key)));
    assert!(
        is_auth(joiner.accept_answer(&own_key)),
        "its own key, relabelled"
    );
    assert!(!joiner.agreed());
    assert_eq!(joiner.sas(), None);

    let reveal = joiner.accept_answer(&answer).unwrap();
    sponsor.accept_reveal(&reveal).unwrap();
    assert_eq!(joiner.sas(), sponsor.sas());
}

#[test]
fn each_step_belongs_to_one_side_and_happens_once() {
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();

    let err = joiner.accept_answer(&answer).unwrap_err();
    assert_eq!(
        err.to_string(),
        "corrupted key material: the pairing mailbox is not known yet"
    );
    assert!(!joiner.agreed());
    assert!(is_corrupted(sponsor.accept_answer(&answer)));

    joiner.set_mailbox(MAILBOX);
    assert_eq!(joiner.mailbox_id(), Some(MAILBOX));
    let reveal = joiner.accept_answer(&answer).unwrap();
    let digits = joiner.sas();
    assert!(
        is_corrupted(joiner.accept_reveal(&reveal)),
        "a joiner takes no reveal"
    );

    let (_, other_answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(3)).unwrap();
    assert!(is_corrupted(joiner.accept_answer(&other_answer)));
    assert!(is_corrupted(joiner.accept_answer(&answer)));
    assert_eq!(joiner.sas(), digits, "a second answer changes nothing");

    sponsor.accept_reveal(&reveal).unwrap();
    assert_eq!(sponsor.sas(), digits);
    assert!(is_corrupted(sponsor.accept_answer(&answer)));
}

#[test]
fn nothing_is_sealed_or_opened_before_the_agreement() {
    let rng = SeededRng::new(9);
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    assert!(joiner.seal(MessageKind::Identity, 3, b"x", &rng).is_empty());
    assert_eq!(joiner.sas(), None);
    let (_, agreed_sponsor) = pair();
    let sealed = agreed_sponsor.seal(MessageKind::Bundle, 4, b"x", &rng);
    assert!(is_corrupted(joiner.open(&sealed, 4)));

    // The sponsor between its answer and the reveal.
    let (sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    assert!(sponsor.seal(MessageKind::Bundle, 4, b"x", &rng).is_empty());
    joiner.accept_answer(&answer).unwrap();
    let identity = joiner.seal(MessageKind::Identity, 3, b"x", &rng);
    assert!(is_corrupted(sponsor.open(&identity, 3)));
}

// ---------------------------------------------------------------------------------
// Sealed messages

#[test]
fn sealed_messages_have_the_documented_layout() {
    let (joiner, sponsor) = pair();
    let rng = SeededRng::new(9);
    let sealed = joiner.seal(MessageKind::Identity, 0x0102_0304, b"payload", &rng);
    assert_eq!(
        &sealed[..3],
        &[1, 1, 6],
        "envelope v1, suite 1, kind Pairing"
    );
    let envelope = Envelope::decode(&sealed).unwrap();
    assert_eq!(envelope.kind, EnvelopeKind::Pairing);
    assert_eq!(envelope.header, [0, 4, 3, 2, 1, 1]);
    assert!(envelope.signature.is_none());
    let (_, plaintext) = kb_crypto::open_envelope(&sealed, channel_key(&joiner), None).unwrap();
    assert_eq!(plaintext.as_slice(), b"payload");

    let abort = sponsor.seal(MessageKind::Abort, 7, b"", &rng);
    assert_eq!(Envelope::decode(&abort).unwrap().header, [1, 7, 0, 0, 0, 3]);

    let again = joiner.seal(MessageKind::Identity, 0x0102_0304, b"payload", &rng);
    assert_ne!(again, sealed, "a fresh nonce every time");
}

#[test]
fn sealed_messages_refuse_a_wrong_role_sequence_or_key() {
    let (joiner, sponsor) = pair();
    let rng = SeededRng::new(9);
    let sealed = joiner.seal(MessageKind::Identity, 3, b"identity", &rng);
    assert!(sponsor.open(&sealed, 3).is_ok());

    for seq in [0, 2, 4, u32::MAX] {
        assert!(is_auth(sponsor.open(&sealed, seq)), "seq {seq}");
    }
    assert!(is_auth(joiner.open(&sealed, 3)), "reflected to its sender");

    let (other_joiner, other_sponsor) = pair_with(3, 4, MAILBOX);
    assert!(is_auth(other_sponsor.open(&sealed, 3)), "another session");
    let foreign = other_joiner.seal(MessageKind::Identity, 3, b"identity", &rng);
    assert!(is_auth(sponsor.open(&foreign, 3)));

    for i in 0..sealed.len() {
        let mut tampered = sealed.clone();
        tampered[i] ^= 0x01;
        assert!(
            matches!(sponsor.open(&tampered, 3), Err(KeysError::Crypto(_))),
            "byte {i}"
        );
    }
    for len in 0..sealed.len() {
        assert!(
            matches!(sponsor.open(&sealed[..len], 3), Err(KeysError::Crypto(_))),
            "{len} bytes"
        );
    }
    // A signature (unchecked by `open_envelope` without a verifier) is refused.
    let signer = SigningKey::from_seed(&Secret32::from_bytes([3; 32]));
    let mut signed = sealed.clone();
    signed.extend_from_slice(&kb_crypto::sign(&signer, "envelope/pairing", &sealed).to_bytes());
    assert!(is_encoding(sponsor.open(&signed, 3)));

    // The right key with another envelope kind or header shape.
    let key = channel_key(&sponsor);
    let header = header(Role::Opener, 3, MessageKind::Identity);
    let seal = |kind: EnvelopeKind, header: &[u8]| {
        kb_crypto::seal_envelope(kind, header, key, b"x", None, &rng)
    };
    assert!(is_encoding(
        sponsor.open(&seal(EnvelopeKind::Entry, &header), 3)
    ));
    assert!(is_encoding(
        sponsor.open(&seal(EnvelopeKind::Pairing, &header[..5]), 3)
    ));
    assert!(is_encoding(sponsor.open(
        &seal(EnvelopeKind::Pairing, &[&header[..], &[0]].concat()),
        3
    )));
    assert!(is_auth(
        sponsor.open(&seal(EnvelopeKind::Pairing, &[2, 3, 0, 0, 0, 1]), 3)
    ));
    assert!(is_corrupted(
        sponsor.open(&seal(EnvelopeKind::Pairing, &[0, 3, 0, 0, 0, 9]), 3)
    ));
    assert!(
        sponsor
            .open(&seal(EnvelopeKind::Pairing, &header), 3)
            .is_ok()
    );
}

#[test]
fn either_side_can_abort() {
    let (joiner, sponsor) = pair();
    let rng = SeededRng::new(9);
    let (kind, plaintext) = sponsor
        .open(&joiner.seal(MessageKind::Abort, 1, b"", &rng), 1)
        .unwrap();
    assert_eq!(kind, MessageKind::Abort);
    assert!(plaintext.is_empty());
    let (kind, _) = joiner
        .open(&sponsor.seal(MessageKind::Abort, 2, b"", &rng), 2)
        .unwrap();
    assert_eq!(kind, MessageKind::Abort);
}

#[test]
fn roles_and_kinds_have_fixed_bytes() {
    for role in [Role::Opener, Role::Typist] {
        assert_eq!(Role::from_byte(role.to_byte()).unwrap(), role);
        assert_eq!(role.peer().peer(), role);
        assert_ne!(role.peer(), role);
    }
    assert_eq!((Role::Opener.to_byte(), Role::Typist.to_byte()), (0, 1));
    assert!(is_corrupted(Role::from_byte(2)));
    for kind in [
        MessageKind::Identity,
        MessageKind::Bundle,
        MessageKind::Abort,
        MessageKind::InviteIdentity,
        MessageKind::InviteBundle,
    ] {
        assert_eq!(MessageKind::from_byte(kind.to_byte()).unwrap(), kind);
    }
    assert_eq!(
        [
            MessageKind::Identity.to_byte(),
            MessageKind::Bundle.to_byte(),
            MessageKind::Abort.to_byte(),
            MessageKind::InviteIdentity.to_byte(),
            MessageKind::InviteBundle.to_byte()
        ],
        [1, 2, 3, 4, 5]
    );
    for byte in [0u8, 6, 0xff] {
        assert!(is_corrupted(MessageKind::from_byte(byte)));
    }
}

// ---------------------------------------------------------------------------------
// Identity

fn identity(name: &str) -> Identity {
    let (device_id, keys) = device(4);
    Identity {
        device_id,
        keys: keys.public(),
        name: name.to_owned(),
    }
}

#[test]
fn identities_round_trip_at_a_constant_length() {
    let long = "x".repeat(DEVICE_NAME_PAD);
    for name in [
        "",
        "Pixel 9",
        "Ben's laptop",
        "Pixel 9 \u{1F4F1} \u{00E9}t\u{00E9}",
        "nul\0inside",
        long.as_str(),
    ] {
        let identity = identity(name);
        let bytes = identity.encode();
        assert_eq!(bytes.len(), IDENTITY_LEN);
        assert_eq!(IDENTITY_LEN, 148);
        assert_eq!(Identity::decode(&bytes).unwrap(), identity, "{name:?}");
    }
}

#[test]
fn an_identity_has_the_documented_layout() {
    let identity = identity("Desk");
    let mut expected = identity.device_id.as_bytes().to_vec();
    expected.extend_from_slice(&identity.keys.signing.to_bytes());
    expected.extend_from_slice(&identity.keys.agreement.to_bytes());
    expected.extend_from_slice(&[4, 0, 0, 0]);
    expected.extend_from_slice(b"Desk");
    expected.resize(148, 0);
    assert_eq!(identity.encode(), expected);
}

#[test]
fn long_names_are_cut_on_a_character_boundary() {
    let phone = "\u{1F4F1}";
    for (name, kept) in [
        ("a".repeat(70), "a".repeat(64)),
        (format!("{}\u{00E9}", "a".repeat(63)), "a".repeat(63)),
        (format!("{}{phone}", "a".repeat(62)), "a".repeat(62)),
        (phone.repeat(17), phone.repeat(16)),
        (
            format!("{}\u{00E9}", "a".repeat(62)),
            format!("{}\u{00E9}", "a".repeat(62)),
        ),
    ] {
        let decoded = Identity::decode(&identity(&name).encode()).unwrap();
        assert_eq!(decoded.name, kept, "{name}");
        assert_eq!(truncate_utf8(&name, DEVICE_NAME_PAD), kept);
    }
    assert_eq!(truncate_utf8("abc", 0), "");
    assert_eq!(truncate_utf8("", 64), "");
}

#[test]
fn malformed_identities_are_refused() {
    let bytes = identity("Desk").encode();
    for len in 0..bytes.len() {
        assert!(is_corrupted(Identity::decode(&bytes[..len])), "{len} bytes");
    }
    assert!(is_corrupted(Identity::decode(
        &[bytes.as_slice(), &[0]].concat()
    )));

    let name_len_at = 16 + 32 + 32;
    let name_at = name_len_at + 4;
    let edit = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut other = bytes.clone();
        f(&mut other);
        Identity::decode(&other)
    };
    for len in [65u32, 0x100, u32::MAX] {
        assert!(is_corrupted(edit(&|b: &mut Vec<u8>| {
            b[name_len_at..name_at].copy_from_slice(&len.to_le_bytes());
        })));
    }
    // Padding must be zero.
    assert!(is_corrupted(edit(&|b: &mut Vec<u8>| b[name_at + 10] = 1)));
    assert!(is_corrupted(edit(&|b: &mut Vec<u8>| b[147] = 0x20)));
    assert!(
        is_corrupted(edit(&|b: &mut Vec<u8>| b[name_at] = 0xff)),
        "not UTF-8"
    );
    // Invalid keys: the identity point as signing key, a low-order agreement key.
    assert!(is_corrupted(edit(&|b: &mut Vec<u8>| {
        b[16..48].fill(0);
        b[16] = 1;
    })));
    assert!(is_corrupted(edit(&|b: &mut Vec<u8>| b[48..80].fill(0))));
}

// ---------------------------------------------------------------------------------
// Bundle

fn bundle() -> Bundle {
    Bundle {
        ark: ark(1),
        ark_generation: 3,
        account_id: AccountId::from_bytes([7; 16]),
        server_url: SERVER.to_owned(),
        log: vec![vec![1, 2, 3], Vec::new(), vec![9; 300]],
        recovery_id: [5; 32],
        recovery_blob: vec![8; 93],
        workspace_id: WorkspaceId::from_bytes([6; 16]),
        workspace_name: "Notes \u{00E9}t\u{00E9}".to_owned(),
    }
}

/// The error of a bundle that must not decode (`Bundle` has no `Debug`).
fn bundle_error(bytes: &[u8]) -> KeysError {
    match Bundle::decode(bytes) {
        Ok(_) => panic!("decoded"),
        Err(err) => err,
    }
}

/// A bundle for property tests: `Bundle` has no `Debug` (it holds the root key and the
/// recovery id), proptest needs one to report a failing case.
struct ArbBundle(Bundle);

impl core::fmt::Debug for ArbBundle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Bundle")
            .field("ark_generation", &self.0.ark_generation)
            .field("account_id", &self.0.account_id)
            .field("server_url", &self.0.server_url)
            .field("log", &self.0.log)
            .field("recovery_blob", &self.0.recovery_blob)
            .field("workspace_id", &self.0.workspace_id)
            .field("workspace_name", &self.0.workspace_name)
            .finish_non_exhaustive()
    }
}

fn assert_same(actual: &Bundle, expected: &Bundle) {
    assert_eq!(actual.ark, expected.ark);
    assert_eq!(actual.ark_generation, expected.ark_generation);
    assert_eq!(actual.account_id, expected.account_id);
    assert_eq!(actual.server_url, expected.server_url);
    assert_eq!(actual.log, expected.log);
    assert_eq!(actual.recovery_id, expected.recovery_id);
    assert_eq!(actual.recovery_blob, expected.recovery_blob);
    assert_eq!(actual.workspace_id, expected.workspace_id);
    assert_eq!(actual.workspace_name, expected.workspace_name);
}

#[test]
fn bundles_round_trip() {
    let full = bundle();
    let bytes = full.encode();
    assert_same(&Bundle::decode(&bytes).unwrap(), &full);
    assert_eq!(
        bytes.capacity(),
        bytes.len(),
        "allocated once at its final size"
    );

    let empty = Bundle {
        log: Vec::new(),
        server_url: String::new(),
        recovery_blob: Vec::new(),
        workspace_name: String::new(),
        ..bundle()
    };
    let bytes = empty.encode();
    assert_eq!(bytes.len(), 32 + 4 + 16 + 4 + 4 + 32 + 4 + 16 + 4);
    assert_same(&Bundle::decode(&bytes).unwrap(), &empty);
}

#[test]
fn a_bundle_has_the_documented_layout() {
    let b = bundle();
    let mut expected = b.ark.secret().expose().to_vec();
    expected.extend_from_slice(&3u32.to_le_bytes());
    expected.extend_from_slice(&[7; 16]);
    expected.extend_from_slice(&u32::try_from(SERVER.len()).unwrap().to_le_bytes());
    expected.extend_from_slice(SERVER.as_bytes());
    expected.extend_from_slice(&3u32.to_le_bytes());
    expected.extend_from_slice(&[3, 0, 0, 0, 1, 2, 3]);
    expected.extend_from_slice(&[0, 0, 0, 0]);
    expected.extend_from_slice(&300u32.to_le_bytes());
    expected.extend_from_slice(&[9; 300]);
    expected.extend_from_slice(&[5; 32]);
    expected.extend_from_slice(&93u32.to_le_bytes());
    expected.extend_from_slice(&[8; 93]);
    expected.extend_from_slice(&[6; 16]);
    let name = "Notes \u{00E9}t\u{00E9}".as_bytes();
    expected.extend_from_slice(&u32::try_from(name.len()).unwrap().to_le_bytes());
    expected.extend_from_slice(name);
    assert_eq!(b.encode().as_slice(), expected.as_slice());
}

#[test]
fn malformed_bundles_are_refused() {
    let bytes = bundle().encode();
    for len in 0..bytes.len() {
        assert!(is_corrupted(Bundle::decode(&bytes[..len])), "{len} bytes");
    }
    assert!(is_corrupted(Bundle::decode(
        &[bytes.as_slice(), &[0]].concat()
    )));

    let url_at = 32 + 4 + 16 + 4;
    let mut bad_url = bytes.to_vec();
    bad_url[url_at] = 0xff;
    let err = bundle_error(&bad_url);
    assert_eq!(
        err.to_string(),
        "corrupted key material: malformed pairing bundle: the server address is not UTF-8"
    );
    let mut bad_name = bytes.to_vec();
    let last = bad_name.len() - 1;
    bad_name[last] = 0xff;
    assert!(is_corrupted(Bundle::decode(&bad_name)));

    // A huge entry count runs out of input instead of allocating.
    let count_at = url_at + SERVER.len();
    let mut huge = bytes.to_vec();
    huge[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(is_corrupted(Bundle::decode(&huge)));
    // Errors carry no key bytes.
    let message = bundle_error(&bytes[..40]).to_string();
    assert!(!message.contains(&hex(&bytes[..4])), "{message}");
}

/// The bundle of a modest account (five devices, one revocation with the root key
/// rotated to the three that remain) travels in one mailbox message. Each further
/// rotation adds about 250 bytes per remaining device (see the report of ADR-0022).
#[test]
fn a_modest_accounts_bundle_fits_in_one_message() {
    let rng = SeededRng::new(9);
    let root = ark(1);
    let kit = RecoveryKey::generate(&rng);
    let devices: Vec<_> = (1..=5).map(device).collect();
    let name = |i: usize| encrypt_name(&root, devices[i].0, "A device name", &rng);
    let mut log = DeviceLog::genesis(
        devices[0].0,
        &devices[0].1,
        kit.verifying_key(),
        name(0),
        T0,
    )
    .unwrap();
    for (i, (device_id, keys)) in devices.iter().enumerate().skip(1) {
        log.add_device(
            *device_id,
            keys.public(),
            name(i),
            crate::log::LogSigner::Device(&devices[0].1, devices[0].0),
            T0,
        )
        .unwrap();
    }
    log.revoke_device(devices[4].0, (&devices[0].1, devices[0].0), T0)
        .unwrap();
    let new_kit = RecoveryKey::generate(&rng);
    log.rotate_recovery(new_kit.verifying_key(), (&devices[0].1, devices[0].0), T0)
        .unwrap();
    let new_root = ark(2);
    let blob = RecoveryBlob {
        account_id: log.verify().unwrap().account_id,
        generation: 1,
        wrapped: new_kit.wrap_ark(&new_root, &rng),
    }
    .encode();
    let content = RotationBox {
        ark: new_root.clone(),
        recovery_id: new_kit.recovery_id(),
        recovery_blob: blob.clone(),
    };
    let boxes = (1..4)
        .map(|i| {
            (
                devices[i].0,
                seal_root_for(&content, 1, &devices[i].1.public(), &rng),
            )
        })
        .collect();
    log.rotate_root(1, boxes, (&devices[0].1, devices[0].0), T0)
        .unwrap();

    let bundle = Bundle {
        ark: new_root,
        ark_generation: 1,
        account_id: log.verify().unwrap().account_id,
        server_url: SERVER.to_owned(),
        log: log
            .entries()
            .iter()
            .map(crate::log::LogEntry::encode)
            .collect(),
        recovery_id: new_kit.recovery_id(),
        recovery_blob: blob,
        workspace_id: WorkspaceId::from_bytes([6; 16]),
        workspace_name: "Notes".to_owned(),
    };
    let (_, sponsor) = pair();
    let sealed = sponsor.seal(MessageKind::Bundle, 0, &bundle.encode(), &rng);
    assert!(
        sealed.len() < PAIRING_MAX_MESSAGE_BYTES / 4,
        "{} bytes",
        sealed.len()
    );
}

// ---------------------------------------------------------------------------------
// Properties

fn arb_bundle() -> impl Strategy<Value = ArbBundle> {
    (
        any::<[u8; 32]>(),
        any::<u32>(),
        any::<[u8; 16]>(),
        "\\PC{0,40}",
        proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..40), 0..6),
        any::<[u8; 32]>(),
        proptest::collection::vec(any::<u8>(), 0..120),
        any::<[u8; 16]>(),
        "\\PC{0,40}",
    )
        .prop_map(
            |(ark, ark_generation, account, server_url, log, recovery_id, blob, ws, name)| {
                ArbBundle(Bundle {
                    ark: Ark::from_secret(Secret32::from_bytes(ark)),
                    ark_generation,
                    account_id: AccountId::from_bytes(account),
                    server_url,
                    log,
                    recovery_id,
                    recovery_blob: blob,
                    workspace_id: WorkspaceId::from_bytes(ws),
                    workspace_name: name,
                })
            },
        )
}

proptest! {
    #[test]
    fn codes_round_trip_for_any_mailbox_and_server(
        id in any::<[u8; 6]>(),
        server in "\\PC{0,64}",
    ) {
        let code = PairingCode { mailbox_id: id };
        let text = code.encode(&server);
        prop_assert_eq!(text.len(), PAIRING_CODE_LEN);
        prop_assert_eq!(PairingCode::decode(&text, &server).unwrap(), code);
        prop_assert_eq!(PairingCode::decode(&code.display(&server), &server).unwrap(), code);
        prop_assert_eq!(PairingCode::decode(&text.to_lowercase(), &server).unwrap(), code);
        let hex = code.mailbox_hex();
        prop_assert_eq!(PairingCode::from_mailbox_hex(&hex).unwrap(), code);
    }

    #[test]
    fn any_typed_text_decodes_only_to_its_own_canonical_code(text in "\\PC{0,40}") {
        if let Ok(code) = PairingCode::decode(&text, SERVER) {
            let normalised: String = text
                .chars()
                .filter(|&c| !c.is_whitespace() && !is_dash(c))
                .map(|c| match c.to_ascii_uppercase() {
                    'O' => '0',
                    'I' | 'L' => '1',
                    other => other,
                })
                .collect();
            prop_assert_eq!(code.encode(SERVER), normalised);
        }
    }

    #[test]
    fn symbol_strings_decode_exactly_when_the_checksum_matches(
        symbols in proptest::collection::vec(0usize..32, PAIRING_CODE_LEN),
    ) {
        let text: String = symbols.iter().map(|&i| char::from(CROCKFORD[i])).collect();
        match PairingCode::decode(&text, SERVER) {
            Ok(code) => prop_assert_eq!(code.encode(SERVER), text),
            Err(err) => prop_assert!(matches!(err, KeysError::Corrupted(_))),
        }
    }

    #[test]
    fn messages_round_trip_and_are_bound_to_role_sequence_and_bytes(
        seed in any::<u64>(),
        kind in 1u8..=3,
        seq in any::<u32>(),
        plaintext in proptest::collection::vec(any::<u8>(), 0..512),
        flip in any::<proptest::sample::Index>(),
    ) {
        let (joiner, sponsor) = pair_with(seed, seed ^ 0x5555, MAILBOX);
        prop_assert_eq!(joiner.sas(), sponsor.sas());
        let kind = MessageKind::from_byte(kind).unwrap();
        let rng = SeededRng::new(seed);

        let sealed = joiner.seal(kind, seq, &plaintext, &rng);
        let (opened_kind, opened) = sponsor.open(&sealed, seq).unwrap();
        prop_assert_eq!(opened_kind, kind);
        prop_assert_eq!(opened.as_slice(), plaintext.as_slice());
        prop_assert!(is_auth(sponsor.open(&sealed, seq.wrapping_add(1))));
        prop_assert!(is_auth(joiner.open(&sealed, seq)));
        let mut tampered = sealed.clone();
        let i = flip.index(tampered.len());
        tampered[i] ^= 0x80;
        prop_assert!(matches!(sponsor.open(&tampered, seq), Err(KeysError::Crypto(_))));

        let back = sponsor.seal(kind, seq, &plaintext, &rng);
        let (back_kind, back_plaintext) = joiner.open(&back, seq).unwrap();
        prop_assert_eq!(back_kind, kind);
        prop_assert_eq!(back_plaintext.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn opening_arbitrary_bytes_fails_without_panicking(
        bytes in proptest::collection::vec(any::<u8>(), 0..200),
        seq in any::<u32>(),
    ) {
        let (joiner, _) = pair();
        prop_assert!(joiner.open(&bytes, seq).is_err());
        let rng = SeededRng::new(1);
        prop_assert!(Session::sponsor(MAILBOX, &bytes, &rng).is_err() || bytes.len() == 34);
        let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
        joiner.set_mailbox(MAILBOX);
        let (mut sponsor, _) = Session::sponsor(MAILBOX, &offer, &rng).unwrap();
        prop_assert!(sponsor.accept_reveal(&bytes).is_err());
        prop_assert!(!sponsor.agreed());
        prop_assert!(joiner.accept_answer(&bytes).is_err() || bytes.len() == 34);
    }

    #[test]
    fn identities_round_trip_with_the_name_cut_to_the_pad(name in "\\PC{0,80}") {
        let identity = identity(&name);
        let bytes = identity.encode();
        prop_assert_eq!(bytes.len(), IDENTITY_LEN);
        let decoded = Identity::decode(&bytes).unwrap();
        prop_assert!(name.starts_with(decoded.name.as_str()));
        if name.len() <= DEVICE_NAME_PAD {
            prop_assert_eq!(&decoded, &identity);
        } else {
            prop_assert!(decoded.name.len() > DEVICE_NAME_PAD - 4);
            prop_assert!(decoded.name.len() <= DEVICE_NAME_PAD);
        }
        prop_assert_eq!(decoded.encode(), bytes);
    }

    #[test]
    fn bundles_round_trip_for_any_content(bundle in arb_bundle()) {
        let ArbBundle(bundle) = bundle;
        let bytes = bundle.encode();
        let decoded = Bundle::decode(&bytes).unwrap();
        assert_same(&decoded, &bundle);
        let again = decoded.encode();
        prop_assert_eq!(again.as_slice(), bytes.as_slice());
    }

    #[test]
    fn decoders_accept_only_canonical_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        if let Ok(identity) = Identity::decode(&bytes) {
            prop_assert_eq!(identity.encode(), bytes.clone());
        }
        if let Ok(bundle) = Bundle::decode(&bytes) {
            let again = bundle.encode();
            prop_assert_eq!(again.as_slice(), bytes.as_slice());
        }
    }
}

// ---------------------------------------------------------------------------------
// Invite sessions (M13): the opener and the typist over the invite transcript domain

#[test]
fn an_invite_session_agrees_like_a_pairing_and_the_domain_separates_them() {
    let domain = kb_types::sharing::INVITE_TRANSCRIPT_DOMAIN;
    let open = |opener_domain: &'static str, typist_domain: &'static str| {
        let (mut opener, offer) = Session::opener(opener_domain, &SeededRng::new(1));
        opener.set_mailbox(MAILBOX);
        let (mut typist, answer) =
            Session::typist(typist_domain, MAILBOX, &offer, &SeededRng::new(2)).unwrap();
        let reveal = opener.accept_answer(&answer).unwrap();
        typist.accept_reveal(&reveal).unwrap();
        (opener, typist)
    };

    let (opener, typist) = open(domain, domain);
    assert_eq!(opener.role(), Role::Opener);
    assert_eq!(typist.role(), Role::Typist);
    assert!(opener.agreed() && typist.agreed());
    assert_eq!(
        opener.sas(),
        typist.sas(),
        "both sides show the same digits"
    );

    // The same keys under the device pairing domain give another channel and other digits.
    let (joiner, sponsor) = pair();
    assert_ne!(joiner.sas(), opener.sas());
    let rng = SeededRng::new(9);
    let sealed = opener.seal(MessageKind::InviteIdentity, 3, b"x", &rng);
    assert!(typist.open(&sealed, 3).is_ok());
    assert!(
        is_auth(sponsor.open(&sealed, 3)),
        "a pairing session cannot open an invite message"
    );

    // A typist and an opener that disagree on the domain never share a channel.
    let (opener, typist) = open(domain, PAIRING_TRANSCRIPT_DOMAIN);
    assert_ne!(opener.sas(), typist.sas());
    let sealed = opener.seal(MessageKind::InviteBundle, 4, b"y", &rng);
    assert!(is_auth(typist.open(&sealed, 4)));

    // `joiner` and `sponsor` are the opener and the typist of the pairing domain.
    let (opener, typist) = open(PAIRING_TRANSCRIPT_DOMAIN, PAIRING_TRANSCRIPT_DOMAIN);
    let (joiner, sponsor) = pair();
    assert_eq!(opener.sas(), joiner.sas());
    assert_eq!(typist.sas(), sponsor.sas());
}

// ---------------------------------------------------------------------------------
// Invite sessions under attack, and the invitee's identity message

const INVITE_DOMAIN: &str = kb_types::sharing::INVITE_TRANSCRIPT_DOMAIN;

/// An opener (the inviter) and a typist (the invitee) over the invite domain that completed
/// offer, answer and reveal.
fn invite_pair(opener_seed: u64, typist_seed: u64) -> (Session, Session) {
    let (mut opener, offer) = Session::opener(INVITE_DOMAIN, &SeededRng::new(opener_seed));
    opener.set_mailbox(MAILBOX);
    let (mut typist, answer) =
        Session::typist(INVITE_DOMAIN, MAILBOX, &offer, &SeededRng::new(typist_seed)).unwrap();
    let reveal = opener.accept_answer(&answer).unwrap();
    typist.accept_reveal(&reveal).unwrap();
    (opener, typist)
}

#[test]
fn an_invite_relay_that_swaps_keys_changes_the_digits_and_cannot_read() {
    let rng = SeededRng::new(9);

    // A full man in the middle between the inviter and the invitee.
    let (mut inviter, offer) = Session::opener(INVITE_DOMAIN, &SeededRng::new(1));
    inviter.set_mailbox(MAILBOX);
    let (mut relay_as_inviter, relay_offer) = Session::opener(INVITE_DOMAIN, &SeededRng::new(66));
    relay_as_inviter.set_mailbox(MAILBOX);
    let (mut invitee, answer) =
        Session::typist(INVITE_DOMAIN, MAILBOX, &relay_offer, &SeededRng::new(2)).unwrap();
    let (mut relay_as_invitee, relay_answer) =
        Session::typist(INVITE_DOMAIN, MAILBOX, &offer, &SeededRng::new(67)).unwrap();
    let reveal = inviter.accept_answer(&relay_answer).unwrap();
    relay_as_invitee.accept_reveal(&reveal).unwrap();
    let relay_reveal = relay_as_inviter.accept_answer(&answer).unwrap();
    invitee.accept_reveal(&relay_reveal).unwrap();
    assert_ne!(
        inviter.sas(),
        invitee.sas(),
        "the two people see two different numbers"
    );
    // Forwarding a sealed message unchanged gets nowhere.
    let identity = invitee.seal(MessageKind::InviteIdentity, 3, b"identity", &rng);
    assert!(is_auth(inviter.open(&identity, 3)));
    let bundle = inviter.seal(MessageKind::InviteBundle, 4, b"bundle", &rng);
    assert!(is_auth(invitee.open(&bundle, 4)));

    // Only the answer key replaced.
    let relay_key = ephemeral(66).public();
    let (mut inviter, offer) = Session::opener(INVITE_DOMAIN, &SeededRng::new(1));
    inviter.set_mailbox(MAILBOX);
    let (mut invitee, answer) =
        Session::typist(INVITE_DOMAIN, MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    let mut swapped = answer.clone();
    swapped[2..].copy_from_slice(&relay_key.to_bytes());
    let reveal = inviter.accept_answer(&swapped).unwrap();
    invitee.accept_reveal(&reveal).unwrap();
    assert_ne!(inviter.sas(), invitee.sas());

    // The reveal replaced: it no longer matches the commitment of the offer.
    let (mut inviter, offer) = Session::opener(INVITE_DOMAIN, &SeededRng::new(1));
    inviter.set_mailbox(MAILBOX);
    let (mut invitee, answer) =
        Session::typist(INVITE_DOMAIN, MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    let mut reveal = inviter.accept_answer(&answer).unwrap();
    reveal[2..].copy_from_slice(&relay_key.to_bytes());
    assert!(is_auth(invitee.accept_reveal(&reveal)));
    assert!(!invitee.agreed());
}

#[test]
fn invite_messages_refuse_a_wrong_role_sequence_domain_or_session() {
    let (opener, typist) = invite_pair(1, 2);
    let rng = SeededRng::new(9);
    let sealed = typist.seal(MessageKind::InviteIdentity, 3, b"identity", &rng);
    let (kind, plaintext) = opener.open(&sealed, 3).unwrap();
    assert_eq!(kind, MessageKind::InviteIdentity);
    assert_eq!(plaintext.as_slice(), b"identity");

    // A wrong sequence number, a message reflected to its sender, another session.
    for seq in [0, 2, 4, u32::MAX] {
        assert!(is_auth(opener.open(&sealed, seq)), "seq {seq}");
    }
    assert!(is_auth(typist.open(&sealed, 3)), "reflected to its sender");
    let (other_opener, other_typist) = invite_pair(3, 4);
    assert!(is_auth(other_opener.open(&sealed, 3)), "another session");
    let foreign = other_typist.seal(MessageKind::InviteIdentity, 3, b"identity", &rng);
    assert!(is_auth(opener.open(&foreign, 3)));

    // The same ephemeral keys over the device pairing domain: another channel entirely.
    let (mut joiner, offer) = Session::joiner(&SeededRng::new(1));
    joiner.set_mailbox(MAILBOX);
    let (mut sponsor, answer) = Session::sponsor(MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    let reveal = joiner.accept_answer(&answer).unwrap();
    sponsor.accept_reveal(&reveal).unwrap();
    assert!(is_auth(sponsor.open(&sealed, 3)), "wrong domain");
    assert_ne!(
        opener.sas(),
        joiner.sas(),
        "the digits differ across domains"
    );
    let pairing_message = joiner.seal(MessageKind::Identity, 3, b"identity", &rng);
    assert!(is_auth(opener.open(&pairing_message, 3)));

    // Tampering with any byte.
    for i in 0..sealed.len() {
        let mut tampered = sealed.clone();
        tampered[i] ^= 0x01;
        assert!(opener.open(&tampered, 3).is_err(), "byte {i}");
    }
}

#[test]
fn an_invite_opener_and_typist_keep_to_their_steps() {
    // A typist cannot take an answer; an opener cannot take a reveal; a typist needs an offer.
    let (mut opener, offer) = Session::opener(INVITE_DOMAIN, &SeededRng::new(1));
    opener.set_mailbox(MAILBOX);
    let (mut typist, answer) =
        Session::typist(INVITE_DOMAIN, MAILBOX, &offer, &SeededRng::new(2)).unwrap();
    assert!(is_corrupted(typist.accept_answer(&answer)));
    assert!(is_corrupted(
        Session::typist(INVITE_DOMAIN, MAILBOX, &answer, &SeededRng::new(3)).map(|_| ())
    ));
    let reveal = opener.accept_answer(&answer).unwrap();
    assert!(is_corrupted(opener.accept_reveal(&reveal)));
    typist.accept_reveal(&reveal).unwrap();
}

fn invite_identity() -> InviteIdentity {
    InviteIdentity {
        account_id: AccountId::from_bytes([0xa1; 16]),
        genesis_hash: [0xb2; 32],
        log_head: crate::membership::ChainHead {
            seq: 0x0102,
            hash: [0xc3; 32],
        },
        log: Some(vec![vec![1, 2, 3], vec![9; 5]]),
        display_name: "Bob".to_owned(),
    }
}

#[test]
fn an_invite_identity_has_the_documented_layout() {
    let mut expected = vec![1u8];
    expected.extend_from_slice(&[0xa1; 16]);
    expected.extend_from_slice(&[0xb2; 32]);
    expected.extend_from_slice(&[0x02, 0x01, 0, 0, 0, 0, 0, 0]);
    expected.extend_from_slice(&[0xc3; 32]);
    expected.push(1);
    expected.extend_from_slice(&[2, 0, 0, 0]);
    expected.extend_from_slice(&[3, 0, 0, 0, 1, 2, 3]);
    expected.extend_from_slice(&[5, 0, 0, 0, 9, 9, 9, 9, 9]);
    expected.extend_from_slice(&[3, 0, 0, 0]);
    expected.extend_from_slice(b"Bob");
    assert_eq!(invite_identity().encode(), expected);
    assert_eq!(
        InviteIdentity::decode(&expected).unwrap(),
        invite_identity()
    );

    // Without a log: one flag byte and no list.
    let no_log = InviteIdentity {
        log: None,
        ..invite_identity()
    };
    let bytes = no_log.encode();
    assert_eq!(bytes[1 + 16 + 32 + 8 + 32], 0);
    assert_eq!(bytes.len(), 1 + 16 + 32 + 8 + 32 + 1 + 4 + 3);
    assert_eq!(InviteIdentity::decode(&bytes).unwrap(), no_log);
}

#[test]
fn an_invite_identity_decodes_strictly() {
    let bytes = invite_identity().encode();
    for len in 0..bytes.len() {
        assert!(
            is_corrupted(InviteIdentity::decode(&bytes[..len])),
            "cut at {len}"
        );
    }
    assert!(is_corrupted(InviteIdentity::decode(
        &[bytes.as_slice(), &[0]].concat()
    )));
    let edit = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut other = bytes.clone();
        f(&mut other);
        InviteIdentity::decode(&other)
    };
    assert!(is_corrupted(edit(&|b: &mut Vec<u8>| b[0] = 2)), "version");
    let flag_at = 1 + 16 + 32 + 8 + 32;
    assert!(
        is_corrupted(edit(&|b: &mut Vec<u8>| b[flag_at] = 2)),
        "not a boolean"
    );
    assert!(is_corrupted(edit(&|b: &mut Vec<u8>| {
        b[flag_at + 1..flag_at + 5].copy_from_slice(&u32::MAX.to_le_bytes());
    })));
    let last = bytes.len() - 1;
    assert!(
        is_corrupted(edit(&|b: &mut Vec<u8>| b[last] = 0xff)),
        "not UTF-8"
    );
}

#[test]
fn an_invite_identity_limits_its_log_and_its_name() {
    let max = kb_types::sharing::INVITE_BUNDLE_LOG_MAX_BYTES;
    // At the limit the log is accepted, one byte more is not.
    let at_limit = InviteIdentity {
        log: Some(vec![vec![7; max / 2], vec![8; max - max / 2]]),
        ..invite_identity()
    };
    assert_eq!(
        InviteIdentity::decode(&at_limit.encode()).unwrap(),
        at_limit
    );
    let over = InviteIdentity {
        log: Some(vec![vec![7; max / 2], vec![8; max - max / 2 + 1]]),
        ..invite_identity()
    };
    assert!(is_corrupted(InviteIdentity::decode(&over.encode())));
    // Many empty entries are bounded by their four-byte length prefixes.
    let many = InviteIdentity {
        log: Some(vec![Vec::new(); 100]),
        ..invite_identity()
    };
    assert_eq!(InviteIdentity::decode(&many.encode()).unwrap(), many);

    // The name: 1 to PROFILE_NAME_MAX_CHARS characters.
    let max_chars = kb_types::sharing::PROFILE_NAME_MAX_CHARS;
    for (name, ok) in [
        (String::new(), false),
        ("x".to_owned(), true),
        ("\u{1F4F1}".repeat(max_chars), true),
        ("\u{1F4F1}".repeat(max_chars + 1), false),
        ("x".repeat(max_chars + 1), false),
    ] {
        let identity = InviteIdentity {
            display_name: name.clone(),
            ..invite_identity()
        };
        assert_eq!(
            InviteIdentity::decode(&identity.encode()).is_ok(),
            ok,
            "{} characters",
            name.chars().count()
        );
    }
}

#[test]
fn an_invite_identity_travels_in_a_sealed_message() {
    let (opener, typist) = invite_pair(1, 2);
    let rng = SeededRng::new(5);
    let sealed = typist.seal(
        MessageKind::InviteIdentity,
        3,
        &invite_identity().encode(),
        &rng,
    );
    let (kind, plaintext) = opener.open(&sealed, 3).unwrap();
    assert_eq!(kind, MessageKind::InviteIdentity);
    assert_eq!(
        InviteIdentity::decode(&plaintext).unwrap(),
        invite_identity()
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn invite_identities_round_trip_and_decode_only_canonically(
        account in any::<[u8; 16]>(),
        genesis in any::<[u8; 32]>(),
        head in (any::<u64>(), any::<[u8; 32]>()),
        log in proptest::option::of(proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..60), 0..6)),
        name in r"\PC{1,20}",
        noise in proptest::collection::vec(any::<u8>(), 0..300),
    ) {
        let identity = InviteIdentity {
            account_id: AccountId::from_bytes(account),
            genesis_hash: genesis,
            log_head: crate::membership::ChainHead { seq: head.0, hash: head.1 },
            log,
            display_name: name,
        };
        prop_assert_eq!(InviteIdentity::decode(&identity.encode()).unwrap(), identity);
        if let Ok(decoded) = InviteIdentity::decode(&noise) {
            prop_assert_eq!(decoded.encode(), noise);
        }
    }
}
