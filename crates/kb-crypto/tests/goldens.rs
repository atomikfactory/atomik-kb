//! The golden files of suite 1 (`fixtures/crypto/*-v1.json`, written by
//! `examples/make_vectors.rs`): every one decodes and opens with today's code, and
//! regenerating it from its recorded inputs gives the same bytes. A failure here
//! means a persisted or transmitted format changed: that needs a new suite or format
//! version, not a regenerated file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_crypto::envelope::EnvelopeKind;
use kb_crypto::kit;
use kb_crypto::{
    AgreementSecret, Argon2Params, CryptoError, Envelope, PassphraseWrapped, Secret32, Signature,
    SigningKey, StreamOpener, StreamSealer, VerifyingKey, WrappedKey, bucket_size, open_envelope,
    open_sealed, pad_to_bucket, seal_envelope, seal_to, sign, unpad, unwrap_key,
    unwrap_with_passphrase, verify, wrap_key, wrap_with_passphrase,
};
use kb_types::testing::SeededRng;
use serde_json::Value;

fn parse(text: &str) -> Value {
    let file: Value = serde_json::from_str(text).unwrap();
    assert_eq!(file["suite"], 1);
    assert!(
        file["description"]
            .as_str()
            .unwrap()
            .contains("make_vectors.rs"),
        "goldens name their generator"
    );
    file
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    assert_eq!(text.len() % 2, 0, "odd number of hex digits");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn secret(value: &Value) -> Secret32 {
    Secret32::from_bytes(unhex(value).try_into().unwrap())
}

fn seed(value: &Value) -> SeededRng {
    SeededRng::new(value.as_u64().unwrap())
}

fn as_u32(value: &Value) -> u32 {
    u32::try_from(value.as_u64().unwrap()).unwrap()
}

#[test]
fn wrapped_key_golden() {
    let file = parse(include_str!("../../../fixtures/crypto/wrap-v1.json"));
    let kek = secret(&file["kek_hex"]);
    let key = secret(&file["key_hex"]);
    let subject = unhex(&file["subject_hex"]);
    let bytes = unhex(&file["wrapped_hex"]);

    let wrapped = WrappedKey::from_bytes(&bytes).unwrap();
    assert_eq!(wrapped.to_bytes().as_slice(), bytes.as_slice());
    assert_eq!(unwrap_key(&wrapped, &kek, &subject).unwrap(), key);
    assert_eq!(
        unwrap_key(&wrapped, &kek, b"another subject"),
        Err(CryptoError::Authentication)
    );
    let again = wrap_key(&key, &kek, &subject, &seed(&file["rng_seed"]));
    assert_eq!(hex(&again.to_bytes()), hex(&bytes), "regenerated");
}

#[test]
fn passphrase_goldens() {
    let file = parse(include_str!("../../../fixtures/crypto/passphrase-v1.json"));
    let key = secret(&file["key_hex"]);
    let passphrase = file["passphrase_utf8"].as_str().unwrap().as_bytes();
    let subject = unhex(&file["subject_hex"]);
    let blobs = file["blobs"].as_array().unwrap();
    assert_eq!(blobs.len(), 2, "small and default parameters");
    for golden in blobs {
        let name = golden["name"].as_str().unwrap();
        let params = Argon2Params {
            memory_kib: as_u32(&golden["memory_kib"]),
            iterations: as_u32(&golden["iterations"]),
            parallelism: as_u32(&golden["parallelism"]),
        };
        let bytes = unhex(&golden["blob_hex"]);
        let blob = PassphraseWrapped::from_bytes(&bytes).unwrap();
        assert_eq!(blob.params, params, "{name}");
        assert_eq!(blob.to_bytes(), bytes, "{name}");

        if params == Argon2Params::DEFAULT {
            // The production path: default parameters pass the production minimum.
            // One Argon2id run at 64 MiB; the others below use the small blob.
            assert_eq!(
                unwrap_with_passphrase(&blob, passphrase, &subject, &Argon2Params::MINIMUM)
                    .unwrap(),
                key,
                "{name}"
            );
        } else {
            assert_eq!(
                unwrap_with_passphrase(&blob, passphrase, &subject, &params).unwrap(),
                key,
                "{name}"
            );
            assert_eq!(
                unwrap_with_passphrase(&blob, b"wrong passphrase", &subject, &params),
                Err(CryptoError::Authentication),
                "{name}"
            );
            assert_eq!(
                unwrap_with_passphrase(&blob, passphrase, &subject, &Argon2Params::MINIMUM),
                Err(CryptoError::WeakParameters),
                "{name}: below the production minimum"
            );
            let again = wrap_with_passphrase(
                &key,
                passphrase,
                params,
                &subject,
                &seed(&golden["rng_seed"]),
            )
            .unwrap();
            assert_eq!(hex(&again.to_bytes()), hex(&bytes), "{name}: regenerated");
        }
    }
}

#[test]
fn signature_goldens() {
    let file = parse(include_str!("../../../fixtures/crypto/sign-v1.json"));
    let signing = SigningKey::from_seed(&secret(&file["seed_hex"]));
    let public = VerifyingKey::from_bytes(&unhex(&file["public_hex"])).unwrap();
    assert_eq!(signing.verifying_key(), public);
    for golden in file["signatures"].as_array().unwrap() {
        let context = golden["context"].as_str().unwrap();
        let message = unhex(&golden["message_hex"]);
        let signature = Signature::from_bytes(&unhex(&golden["signature_hex"])).unwrap();
        verify(&public, context, &message, &signature).unwrap();
        assert_eq!(sign(&signing, context, &message), signature, "regenerated");
        assert_eq!(
            verify(&public, "another-context", &message, &signature),
            Err(CryptoError::Authentication)
        );
    }
}

#[test]
fn sealed_box_golden() {
    let file = parse(include_str!("../../../fixtures/crypto/sealed-box-v1.json"));
    let recipient = AgreementSecret::from_seed(&secret(&file["recipient_seed_hex"]));
    assert_eq!(
        hex(&recipient.public().to_bytes()),
        file["recipient_public_hex"].as_str().unwrap()
    );
    let context = file["context"].as_str().unwrap();
    let plaintext = unhex(&file["plaintext_hex"]);
    let sealed = unhex(&file["sealed_hex"]);
    assert_eq!(
        open_sealed(&recipient, context, &sealed)
            .unwrap()
            .as_slice(),
        plaintext.as_slice()
    );
    let again = seal_to(
        &recipient.public(),
        context,
        &plaintext,
        &seed(&file["rng_seed"]),
    );
    assert_eq!(hex(&again), hex(&sealed), "regenerated");
}

#[test]
fn envelope_goldens_cover_every_kind() {
    let file = parse(include_str!("../../../fixtures/crypto/envelope-v1.json"));
    let key = secret(&file["key_hex"]);
    let signer = SigningKey::from_seed(&secret(&file["signer_seed_hex"]));
    let verifier = VerifyingKey::from_bytes(&unhex(&file["signer_public_hex"])).unwrap();
    assert_eq!(signer.verifying_key(), verifier);

    let goldens = file["envelopes"].as_array().unwrap();
    let kinds: Vec<u64> = goldens
        .iter()
        .map(|g| g["kind_byte"].as_u64().unwrap())
        .collect();
    assert_eq!(kinds, (1..=11).collect::<Vec<u64>>(), "one golden per kind");

    for golden in goldens {
        let kind_byte = u8::try_from(golden["kind_byte"].as_u64().unwrap()).unwrap();
        let kind = EnvelopeKind::from_byte(kind_byte).unwrap();
        assert_eq!(
            kind.context(),
            format!("envelope/{}", golden["kind"].as_str().unwrap())
        );
        let signed = golden["signed"].as_bool().unwrap();
        let header = unhex(&golden["header_hex"]);
        let plaintext = unhex(&golden["plaintext_hex"]);
        let bytes = unhex(&golden["envelope_hex"]);

        let decoded = Envelope::decode(&bytes).unwrap();
        assert_eq!(decoded.kind, kind);
        assert_eq!(decoded.header, header);
        assert_eq!(decoded.signature.is_some(), signed);
        assert_eq!(decoded.encode(), bytes, "{kind:?}: canonical encoding");

        let (opened, text) = open_envelope(&bytes, &key, signed.then_some(&verifier)).unwrap();
        assert_eq!(opened, decoded);
        assert_eq!(text.as_slice(), plaintext.as_slice(), "{kind:?}");
        if !signed {
            assert_eq!(
                open_envelope(&bytes, &key, Some(&verifier)),
                Err(CryptoError::Authentication),
                "{kind:?}: a verifier requires a signature"
            );
        }

        let again = seal_envelope(
            kind,
            &header,
            &key,
            &plaintext,
            signed.then_some(&signer),
            &seed(&golden["rng_seed"]),
        );
        assert_eq!(hex(&again), hex(&bytes), "{kind:?}: regenerated");
    }
}

#[test]
fn padding_goldens() {
    let file = parse(include_str!("../../../fixtures/crypto/padding-v1.json"));
    for golden in file["cases"].as_array().unwrap() {
        let plaintext = unhex(&golden["plaintext_hex"]);
        let padded = unhex(&golden["padded_hex"]);
        assert_eq!(padded.len(), bucket_size(plaintext.len() + 4));
        assert_eq!(unpad(&padded).unwrap(), plaintext.as_slice());
        let again = pad_to_bucket(&plaintext, &seed(&golden["rng_seed"]));
        assert_eq!(hex(&again), hex(&padded), "regenerated");
    }
}

#[test]
fn three_chunk_stream_golden() {
    let file = parse(include_str!("../../../fixtures/crypto/stream-v1.json"));
    let key = secret(&file["key_hex"]);
    let prefix = unhex(&file["aad_prefix_hex"]);
    let chunks = file["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 3);

    let mut opener = StreamOpener::new(key.clone(), prefix.clone());
    for (i, chunk) in chunks.iter().enumerate() {
        let is_final = chunk["is_final"].as_bool().unwrap();
        assert_eq!(is_final, i == 2);
        let opened = opener
            .open_chunk(&unhex(&chunk["sealed_hex"]), is_final)
            .unwrap();
        assert_eq!(opened.as_slice(), unhex(&chunk["plaintext_hex"]).as_slice());
    }
    opener.finish().unwrap();

    let rng = seed(&file["rng_seed"]);
    let mut sealer = StreamSealer::new(key, prefix);
    for chunk in chunks {
        let again = sealer
            .seal_chunk(
                &unhex(&chunk["plaintext_hex"]),
                chunk["is_final"].as_bool().unwrap(),
                &rng,
            )
            .unwrap();
        assert_eq!(
            hex(&again),
            chunk["sealed_hex"].as_str().unwrap(),
            "regenerated"
        );
    }
}

#[test]
fn kit_goldens() {
    let file = parse(include_str!("../../../fixtures/crypto/kit-v1.json"));
    let kits = file["kits"].as_array().unwrap();
    assert!(!kits.is_empty());
    for golden in kits {
        let key = secret(&golden["key_hex"]);
        let words: Vec<&str> = golden["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        let code = golden["code"].as_str().unwrap();
        assert_eq!(kit::words(&key).as_slice(), words.as_slice());
        assert_eq!(kit::code(&key), code);
        assert_eq!(kit::from_words(&words).unwrap(), key);
        assert_eq!(kit::from_code(code).unwrap(), key);
        assert_eq!(kit::parse(&words.join(" ")).unwrap(), key);
        assert_eq!(kit::parse(code).unwrap(), key);
    }
}
