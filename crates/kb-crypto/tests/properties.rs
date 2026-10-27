//! Property tests: every construction round-trips random inputs, tampering is always
//! detected, and no decoder panics on arbitrary bytes or text (threat model G8:
//! every decoder typed and exercised with hostile input).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::envelope::EnvelopeKind;
use kb_crypto::kit;
use kb_crypto::{
    AgreementPublic, AgreementSecret, Argon2Params, CryptoError, Envelope, Nonce24,
    PassphraseWrapped, Secret32, Signature, SigningKey, StreamOpener, StreamSealer, TAG_LEN,
    VerifyingKey, WrappedKey, aead_open, aead_seal, bucket_size, open_envelope, open_sealed,
    pad_to_bucket, seal_envelope, seal_to, sign, unpad, unwrap_key, unwrap_with_passphrase, verify,
    wrap_key, wrap_with_passphrase,
};
use kb_types::testing::SeededRng;
use proptest::collection::vec;
use proptest::prelude::*;

fn key32() -> impl Strategy<Value = [u8; 32]> {
    prop::array::uniform32(any::<u8>())
}

fn bytes(max: usize) -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..max)
}

/// Context names as the code uses them: ASCII words with `/` and `-`, no NUL.
fn context() -> impl Strategy<Value = String> {
    "[a-z][a-z/-]{0,23}"
}

fn kind() -> impl Strategy<Value = EnvelopeKind> {
    prop::sample::select(EnvelopeKind::ALL.to_vec())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn aead_round_trips_and_detects_any_bit_flip(
        key in key32(),
        nonce in prop::array::uniform24(any::<u8>()),
        aad in bytes(64),
        plaintext in bytes(300),
        flip in any::<prop::sample::Index>(),
        bit in 0u8..8,
    ) {
        let key = Secret32::from_bytes(key);
        let nonce = Nonce24::from_bytes(nonce);
        let sealed = aead_seal(&key, &nonce, &aad, &plaintext);
        prop_assert_eq!(sealed.len(), plaintext.len() + TAG_LEN);
        let opened = aead_open(&key, &nonce, &aad, &sealed).unwrap();
        prop_assert_eq!(opened.as_slice(), plaintext.as_slice());
        let mut tampered = sealed.clone();
        let i = flip.index(tampered.len());
        tampered[i] ^= 1 << bit;
        prop_assert_eq!(aead_open(&key, &nonce, &aad, &tampered), Err(CryptoError::Authentication));
    }

    #[test]
    fn wrap_round_trips_and_binds_the_subject(
        key in key32(),
        kek in key32(),
        subject in bytes(48),
        other in bytes(48),
        seed in any::<u64>(),
    ) {
        let (key, kek) = (Secret32::from_bytes(key), Secret32::from_bytes(kek));
        let wrapped = wrap_key(&key, &kek, &subject, &SeededRng::new(seed));
        let decoded = WrappedKey::from_bytes(&wrapped.to_bytes()).unwrap();
        prop_assert_eq!(unwrap_key(&decoded, &kek, &subject).unwrap(), key);
        if other != subject {
            prop_assert_eq!(unwrap_key(&decoded, &kek, &other), Err(CryptoError::Authentication));
        }
    }

    #[test]
    fn signatures_round_trip_and_separate_contexts(
        seed in key32(),
        first in context(),
        second in context(),
        message in bytes(200),
    ) {
        let signing = SigningKey::from_seed(&Secret32::from_bytes(seed));
        let public = VerifyingKey::from_bytes(&signing.verifying_key().to_bytes()).unwrap();
        let signature = sign(&signing, &first, &message);
        let parsed = Signature::from_bytes(&signature.to_bytes()).unwrap();
        prop_assert!(verify(&public, &first, &message, &parsed).is_ok());
        if first != second {
            prop_assert_eq!(
                verify(&public, &second, &message, &parsed),
                Err(CryptoError::Authentication)
            );
        }
    }

    #[test]
    fn sealed_boxes_round_trip(
        recipient in key32(),
        stranger in key32(),
        context in context(),
        plaintext in bytes(200),
        seed in any::<u64>(),
    ) {
        let recipient = AgreementSecret::from_seed(&Secret32::from_bytes(recipient));
        let public = AgreementPublic::from_bytes(&recipient.public().to_bytes()).unwrap();
        let sealed = seal_to(&public, &context, &plaintext, &SeededRng::new(seed));
        let opened = open_sealed(&recipient, &context, &sealed).unwrap();
        prop_assert_eq!(opened.as_slice(), plaintext.as_slice());
        let stranger = AgreementSecret::from_seed(&Secret32::from_bytes(stranger));
        if stranger.public() != recipient.public() {
            prop_assert_eq!(
                open_sealed(&stranger, &context, &sealed),
                Err(CryptoError::Authentication)
            );
        }
    }

    #[test]
    fn envelopes_round_trip(
        kind in kind(),
        header in bytes(64),
        plaintext in bytes(300),
        key in key32(),
        signer in key32(),
        signed in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let key = Secret32::from_bytes(key);
        let signer = SigningKey::from_seed(&Secret32::from_bytes(signer));
        let verifier = signer.verifying_key();
        let rng = SeededRng::new(seed);
        let bytes =
            seal_envelope(kind, &header, &key, &plaintext, signed.then_some(&signer), &rng);
        let decoded = Envelope::decode(&bytes).unwrap();
        prop_assert_eq!(decoded.encode(), bytes.clone());
        prop_assert_eq!(&decoded.header, &header);
        let (_, opened) = open_envelope(&bytes, &key, signed.then_some(&verifier)).unwrap();
        prop_assert_eq!(opened.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn mutated_signed_envelopes_never_open_under_the_verifier(
        kind in kind(),
        header in bytes(32),
        plaintext in bytes(64),
        position in any::<prop::sample::Index>(),
        xor in 1u8..=255,
        seed in any::<u64>(),
    ) {
        let key = Secret32::from_bytes([7; 32]);
        let signer = SigningKey::from_seed(&Secret32::from_bytes([8; 32]));
        let rng = SeededRng::new(seed);
        let bytes = seal_envelope(kind, &header, &key, &plaintext, Some(&signer), &rng);
        let mut mutated = bytes.clone();
        let i = position.index(mutated.len());
        mutated[i] ^= xor;
        prop_assert!(open_envelope(&mutated, &key, Some(&signer.verifying_key())).is_err());
    }

    #[test]
    fn envelope_structs_encode_canonically(
        kind in kind(),
        header in bytes(64),
        nonce in prop::array::uniform24(any::<u8>()),
        ciphertext in vec(any::<u8>(), TAG_LEN..200),
        signature in proptest::option::of(vec(any::<u8>(), 64)),
    ) {
        let envelope = Envelope {
            kind,
            header,
            nonce: Nonce24::from_bytes(nonce),
            ciphertext,
            signature: signature.map(|s| Signature::from_bytes(&s).unwrap()),
        };
        let bytes = envelope.encode();
        prop_assert_eq!(Envelope::decode(&bytes).unwrap(), envelope);
    }

    #[test]
    fn padding_round_trips_into_minimal_buckets(plaintext in bytes(10_000), seed in any::<u64>()) {
        let padded = pad_to_bucket(&plaintext, &SeededRng::new(seed));
        prop_assert_eq!(padded.len(), bucket_size(plaintext.len() + 4));
        prop_assert_eq!(unpad(&padded).unwrap(), plaintext.as_slice());
    }

    #[test]
    fn buckets_are_minimal(len in 0usize..(1 << 26)) {
        let bucket = bucket_size(len);
        prop_assert!(bucket >= len.max(1));
        prop_assert_eq!(bucket_size(bucket), bucket, "a bucket is its own bucket");
        if bucket > 256 {
            // The next smaller bucket would not fit.
            let smaller = if bucket <= 4096 { bucket - 256 } else { bucket / 2 };
            prop_assert!(smaller < len);
        }
    }

    #[test]
    fn streams_round_trip(
        chunks in vec(bytes(300), 1..6),
        key in key32(),
        prefix in bytes(40),
        seed in any::<u64>(),
    ) {
        let rng = SeededRng::new(seed);
        let mut sealer = StreamSealer::new(Secret32::from_bytes(key), prefix.clone());
        let last = chunks.len() - 1;
        let sealed: Vec<Vec<u8>> = chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| sealer.seal_chunk(chunk, i == last, &rng).unwrap())
            .collect();
        let mut opener = StreamOpener::new(Secret32::from_bytes(key), prefix);
        for (i, (bytes, chunk)) in sealed.iter().zip(&chunks).enumerate() {
            let opened = opener.open_chunk(bytes, i == last).unwrap();
            prop_assert_eq!(opened.as_slice(), chunk.as_slice());
        }
        prop_assert!(opener.finish().is_ok());
        // Nothing may follow the final chunk.
        prop_assert!(opener.open_chunk(&sealed[0], false).is_err());
    }

    #[test]
    fn kits_round_trip_through_both_forms(key in key32()) {
        let key = Secret32::from_bytes(key);
        let words = kit::words(&key);
        prop_assert_eq!(kit::from_words(&words).unwrap(), key.clone());
        let prefixes: Vec<&str> = words.iter().map(|w| &w[..w.len().min(4)]).collect();
        prop_assert_eq!(kit::from_words(&prefixes).unwrap(), key.clone());
        prop_assert_eq!(kit::parse(&words.join(" ").to_uppercase()).unwrap(), key.clone());
        let code = kit::code(&key);
        prop_assert_eq!(kit::from_code(&code.to_lowercase()).unwrap(), key.clone());
        prop_assert_eq!(kit::parse(&code.replace('-', " ")).unwrap(), key);
    }

    #[test]
    fn decoders_never_panic_on_arbitrary_bytes(bytes in bytes(400)) {
        let key = Secret32::from_bytes([1; 32]);
        let _ = Envelope::decode(&bytes);
        let _ = open_envelope(&bytes, &key, None);
        let _ = WrappedKey::from_bytes(&bytes);
        let _ = PassphraseWrapped::from_bytes(&bytes);
        let _ = VerifyingKey::from_bytes(&bytes);
        let _ = AgreementPublic::from_bytes(&bytes);
        let _ = Signature::from_bytes(&bytes);
        let _ = unpad(&bytes);
        let _ = open_sealed(&AgreementSecret::from_seed(&key), "ctx", &bytes);
        let _ = StreamOpener::new(key.clone(), Vec::new()).open_chunk(&bytes, true);
        let _ = aead_open(&key, &Nonce24::from_bytes([0; 24]), b"", &bytes);
    }

    #[test]
    fn decoders_never_panic_on_bucket_sized_input(bytes in vec(any::<u8>(), 256)) {
        let _ = unpad(&bytes);
    }

    #[test]
    fn kit_parsers_never_panic_on_arbitrary_text(text in ".{0,160}") {
        let _ = kit::parse(&text);
        let _ = kit::from_code(&text);
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let _ = kit::from_words(&tokens);
    }

    #[test]
    fn kit_parsers_never_panic_on_kit_like_text(
        words in vec("[a-zA-Z]{1,9}", 20..28),
        code in "[A-Za-z2-7 -]{40,80}",
    ) {
        let _ = kit::parse(&words.join(" "));
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let _ = kit::from_words(&refs);
        let _ = kit::parse(&code);
    }
}

proptest! {
    // Argon2id is the slow part: a few cases with the smallest valid memory.
    #![proptest_config(ProptestConfig::with_cases(8))]

    #[test]
    fn passphrase_wrapping_round_trips(
        key in key32(),
        passphrase in bytes(64),
        subject in bytes(32),
        seed in any::<u64>(),
    ) {
        let tiny = Argon2Params { memory_kib: 8, iterations: 1, parallelism: 1 };
        let key = Secret32::from_bytes(key);
        let rng = SeededRng::new(seed);
        let blob = wrap_with_passphrase(&key, &passphrase, tiny, &subject, &rng).unwrap();
        let decoded = PassphraseWrapped::from_bytes(&blob.to_bytes()).unwrap();
        let unwrapped = unwrap_with_passphrase(&decoded, &passphrase, &subject, &tiny).unwrap();
        prop_assert_eq!(unwrapped, key);
        let mut wrong = passphrase.clone();
        wrong.push(0);
        prop_assert_eq!(
            unwrap_with_passphrase(&decoded, &wrong, &subject, &tiny),
            Err(CryptoError::Authentication)
        );
    }
}
