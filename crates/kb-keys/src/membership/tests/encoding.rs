//! The golden encodings of the header (every kind) and of the private part, assembled by hand
//! field by field, and the strictness of their decoders.

use super::*;

const AT: i64 = 0x0102_0304_0506_0708;

/// The public key of RFC 8032 section 7.1, test 1: a valid Ed25519 point whose bytes are known
/// without running any of our code.
const RFC8032_KEY: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

fn header(body: EntryBody) -> MembershipHeader {
    MembershipHeader {
        feed: FeedId::from_bytes([0xf1; 16]),
        seq: 7,
        prev: [0x07; 32],
        account: account(0xa1),
        device: device(0xd1),
        at: AT,
        meta_epoch: 3,
        body,
    }
}

/// `u8 2 | feed | u64 seq | prev | account | device | kind`.
fn prefix(kind: u8) -> Vec<u8> {
    let mut out = vec![2u8];
    out.extend_from_slice(&[0xf1; 16]);
    out.extend_from_slice(&7u64.to_le_bytes());
    out.extend_from_slice(&[0x07; 32]);
    out.extend_from_slice(&[0xa1; 16]);
    out.extend_from_slice(&[0xd1; 16]);
    out.push(kind);
    out
}

/// `i64 at | u32 meta_epoch`.
fn suffix() -> Vec<u8> {
    let mut out = AT.to_le_bytes().to_vec();
    out.extend_from_slice(&3u32.to_le_bytes());
    out
}

fn boxes() -> Vec<KeyBox> {
    vec![
        KeyBox {
            device: device(0xb1),
            bytes: vec![1, 2, 3],
        },
        KeyBox {
            device: device(0xb2),
            bytes: Vec::new(),
        },
    ]
}

fn boxes_bytes() -> Vec<u8> {
    let mut out = 2u32.to_le_bytes().to_vec();
    out.extend_from_slice(&[0xb1; 16]);
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&[1, 2, 3]);
    out.extend_from_slice(&[0xb2; 16]);
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

fn heads() -> Vec<MemberHead> {
    vec![MemberHead {
        account: account(0xa1),
        head: ChainHead {
            seq: 4,
            hash: [0x44; 32],
        },
    }]
}

fn heads_bytes() -> Vec<u8> {
    let mut out = 1u32.to_le_bytes().to_vec();
    out.extend_from_slice(&[0xa1; 16]);
    out.extend_from_slice(&4u64.to_le_bytes());
    out.extend_from_slice(&[0x44; 32]);
    out
}

fn rekey() -> RekeyFields {
    RekeyFields {
        new_epoch: 6,
        sk_commitment: [0xc1; 32],
        prev_wrapped: WrappedKey::from_bytes(&[0x77; 72]).unwrap(),
        boxes: boxes(),
        member_heads: heads(),
    }
}

fn rekey_bytes() -> Vec<u8> {
    let mut out = 6u32.to_le_bytes().to_vec();
    out.extend_from_slice(&[0xc1; 32]);
    out.extend_from_slice(&[0x77; 72]);
    out.extend_from_slice(&boxes_bytes());
    out.extend_from_slice(&heads_bytes());
    out
}

fn invite_body(full: bool) -> (EntryBody, Vec<u8>) {
    let mut fields = vec![0x11u8; 16];
    fields.extend_from_slice(&RFC8032_KEY);
    fields.push(3);
    fields.extend_from_slice(&0x1112_1314_1516_1718_i64.to_le_bytes());
    let body = EntryBody::Invite {
        invite_id: invite_id_of(0x11),
        invite_pub: VerifyingKey::from_bytes(&RFC8032_KEY).unwrap(),
        role: MemberRole::Editor,
        expires_at: 0x1112_1314_1516_1718,
        bound_account: full.then(|| account(0xb0)),
        verified: full,
        enc_bundle: full.then(|| vec![9, 8, 7]),
    };
    if full {
        fields.push(1);
        fields.extend_from_slice(&[0xb0; 16]);
        fields.push(1);
        fields.push(1);
        fields.extend_from_slice(&3u32.to_le_bytes());
        fields.extend_from_slice(&[9, 8, 7]);
    } else {
        fields.extend_from_slice(&[0, 0, 0]);
    }
    (body, fields)
}

/// Every kind with its hand-assembled fields.
fn goldens() -> Vec<(EntryBody, u8, Vec<u8>)> {
    let mut create = 5u32.to_le_bytes().to_vec();
    create.extend_from_slice(&[0xc0; 32]);
    create.extend_from_slice(&boxes_bytes());
    create.extend_from_slice(&heads_bytes());

    let mut join = vec![0x11u8; 16];
    join.extend_from_slice(&[0x5a; 64]);
    join.extend_from_slice(&[0xa1; 16]);
    join.extend_from_slice(&9u64.to_le_bytes());
    join.extend_from_slice(&[0x99; 32]);

    let mut role_set = vec![0xb1u8; 16];
    role_set.push(2);

    let mut remove = vec![0xb1u8; 16];
    remove.extend_from_slice(&rekey_bytes());

    let mut grant = vec![0xb1u8; 16];
    grant.extend_from_slice(&6u32.to_le_bytes());
    grant.extend_from_slice(&boxes_bytes());

    vec![
        (
            EntryBody::Create {
                epoch: 5,
                sk_commitment: [0xc0; 32],
                boxes: boxes(),
                member_heads: heads(),
            },
            1,
            create,
        ),
        {
            let (body, fields) = invite_body(true);
            (body, 2, fields)
        },
        (
            EntryBody::InviteRevoke {
                invite_id: invite_id_of(0x11),
            },
            3,
            vec![0x11; 16],
        ),
        (
            EntryBody::Join {
                invite_id: invite_id_of(0x11),
                proof: Signature::from_bytes(&[0x5a; 64]).unwrap(),
                account: account(0xa1),
                log: ChainHead {
                    seq: 9,
                    hash: [0x99; 32],
                },
            },
            4,
            join,
        ),
        (
            EntryBody::RoleSet {
                target: account(0xb1),
                role: MemberRole::Commenter,
            },
            5,
            role_set,
        ),
        (
            EntryBody::Remove {
                target: account(0xb1),
                rekey: rekey(),
            },
            6,
            remove,
        ),
        (EntryBody::Leave, 7, Vec::new()),
        (EntryBody::Rekey(rekey()), 8, rekey_bytes()),
        (
            EntryBody::Grant {
                member: account(0xb1),
                epoch: 6,
                boxes: boxes(),
            },
            9,
            grant,
        ),
        (EntryBody::Profile, 10, Vec::new()),
        (EntryBody::Settings, 11, Vec::new()),
        (EntryBody::Close, 12, Vec::new()),
    ]
}

fn expected(kind: u8, fields: &[u8]) -> Vec<u8> {
    let mut out = prefix(kind);
    out.extend_from_slice(fields);
    out.extend_from_slice(&suffix());
    out
}

fn is_corrupted<T: core::fmt::Debug>(result: Result<T, KeysError>) -> bool {
    matches!(result, Err(KeysError::Corrupted(_)))
}

#[test]
fn every_kind_has_the_documented_header_layout() {
    let all = goldens();
    assert_eq!(all.len(), 12);
    for (body, kind_byte, fields) in all {
        let kind = body.kind();
        assert_eq!(kind.to_byte(), kind_byte);
        let bytes = expected(kind_byte, &fields);
        let h = header(body);
        assert_eq!(h.encode(), bytes, "{kind:?}");
        assert_eq!(MembershipHeader::decode(&bytes).unwrap(), h, "{kind:?}");
    }
}

#[test]
fn the_unbound_link_invite_has_the_short_layout() {
    let (body, fields) = invite_body(false);
    let bytes = expected(2, &fields);
    assert_eq!(header(body.clone()).encode(), bytes);
    assert_eq!(MembershipHeader::decode(&bytes).unwrap().body, body);
}

#[test]
fn kind_bytes_are_one_to_twelve_in_table_order() {
    let names = [
        EntryKind::Create,
        EntryKind::Invite,
        EntryKind::InviteRevoke,
        EntryKind::Join,
        EntryKind::RoleSet,
        EntryKind::Remove,
        EntryKind::Leave,
        EntryKind::Rekey,
        EntryKind::Grant,
        EntryKind::Profile,
        EntryKind::Settings,
        EntryKind::Close,
    ];
    assert_eq!(EntryKind::ALL, names);
    for (i, kind) in names.into_iter().enumerate() {
        let byte = u8::try_from(i + 1).unwrap();
        assert_eq!(kind.to_byte(), byte);
        assert_eq!(EntryKind::from_byte(byte), Some(kind));
    }
    for byte in [0u8, 13, 14, 0x80, 0xff] {
        assert_eq!(EntryKind::from_byte(byte), None);
    }
    assert_eq!(MEMBERSHIP_VERSION, 2);
}

#[test]
fn headers_decode_only_canonically() {
    for (body, kind_byte, fields) in goldens() {
        let bytes = expected(kind_byte, &fields);
        let kind = body.kind();
        for len in 0..bytes.len() {
            assert!(
                is_corrupted(MembershipHeader::decode(&bytes[..len])),
                "{kind:?} cut at {len}"
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(is_corrupted(MembershipHeader::decode(&longer)), "{kind:?}");
        // Version and kind.
        for version in [0u8, 1, 3, 0xff] {
            let mut other = bytes.clone();
            other[0] = version;
            assert!(is_corrupted(MembershipHeader::decode(&other)));
        }
        for byte in [0u8, 13, 0xff] {
            let mut other = bytes.clone();
            other[1 + 16 + 8 + 32 + 16 + 16] = byte;
            assert!(is_corrupted(MembershipHeader::decode(&other)));
        }
        // Whatever decodes is the canonical encoding of what it decoded.
        for i in 0..bytes.len() {
            let mut other = bytes.clone();
            other[i] ^= 0x01;
            if let Ok(decoded) = MembershipHeader::decode(&other) {
                assert_eq!(decoded.encode(), other, "{kind:?} flipped at {i}");
            }
        }
    }
}

#[test]
fn roles_and_booleans_are_checked() {
    // RoleSet: the role byte is the last byte of the fields.
    let role_at = prefix(5).len() + 16;
    let (body, kind, fields) = goldens().into_iter().find(|g| g.1 == 5).unwrap();
    let bytes = expected(kind, &fields);
    assert_eq!(MembershipHeader::decode(&bytes).unwrap().body, body);
    for (role, ok) in [
        (0u8, false),
        (1, true),
        (2, true),
        (3, true),
        (4, true),
        (5, false),
    ] {
        let mut other = bytes.clone();
        other[role_at] = role;
        assert_eq!(MembershipHeader::decode(&other).is_ok(), ok, "role {role}");
    }
    // Invite: role, then the three flags.
    let (_, fields) = invite_body(true);
    let bytes = expected(2, &fields);
    let role_at = prefix(2).len() + 16 + 32;
    let bound_at = role_at + 1 + 8;
    let verified_at = bound_at + 1 + 16;
    let bundle_at = verified_at + 1;
    for (at, bad) in [
        (role_at, 9u8),
        (bound_at, 2),
        (verified_at, 2),
        (bundle_at, 2),
    ] {
        let mut other = bytes.clone();
        other[at] = bad;
        assert!(is_corrupted(MembershipHeader::decode(&other)), "byte {at}");
    }
}

#[test]
fn the_limits_of_a_header_are_enforced_at_decode() {
    // A Create with more boxes or more heads than an entry may carry.
    let create = goldens().into_iter().find(|g| g.1 == 1).unwrap();
    let boxes_at = prefix(1).len() + 4 + 32;
    let bytes = expected(1, &create.2);
    let mut too_many = bytes.clone();
    too_many[boxes_at..boxes_at + 4].copy_from_slice(&1025u32.to_le_bytes());
    assert!(is_corrupted(MembershipHeader::decode(&too_many)));
    too_many[boxes_at..boxes_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(is_corrupted(MembershipHeader::decode(&too_many)));

    let heads_at = bytes.len() - suffix().len() - heads_bytes().len();
    let mut many_heads = bytes.clone();
    many_heads[heads_at..heads_at + 4].copy_from_slice(&51u32.to_le_bytes());
    assert!(is_corrupted(MembershipHeader::decode(&many_heads)));

    // 1024 boxes are fine (the others of the entry are not): build them for real.
    let body = EntryBody::Grant {
        member: account(1),
        epoch: 1,
        boxes: (0..1024u16)
            .map(|i| {
                let mut id = [0u8; 16];
                id[..2].copy_from_slice(&i.to_le_bytes());
                KeyBox {
                    device: DeviceId::from_bytes(id),
                    bytes: vec![0; 4],
                }
            })
            .collect(),
    };
    let encoded = header(body.clone()).encode();
    assert_eq!(MembershipHeader::decode(&encoded).unwrap().body, body);
    let EntryBody::Grant {
        member,
        epoch,
        mut boxes,
    } = body
    else {
        panic!("a grant")
    };
    boxes.push(KeyBox {
        device: device(0xff),
        bytes: Vec::new(),
    });
    let over = header(EntryBody::Grant {
        member,
        epoch,
        boxes,
    })
    .encode();
    assert!(is_corrupted(MembershipHeader::decode(&over)));

    // An invite bundle at the limit and over it.
    let limit = kb_types::sharing::INVITE_BUNDLE_LOG_MAX_BYTES;
    for (len, ok) in [(limit, true), (limit + 1, false)] {
        let (
            EntryBody::Invite {
                invite_id,
                invite_pub,
                role,
                expires_at,
                ..
            },
            _,
        ) = invite_body(false)
        else {
            panic!("an invite")
        };
        let encoded = header(EntryBody::Invite {
            invite_id,
            invite_pub,
            role,
            expires_at,
            bound_account: None,
            verified: false,
            enc_bundle: Some(vec![0; len]),
        })
        .encode();
        assert_eq!(MembershipHeader::decode(&encoded).is_ok(), ok, "{len}");
    }
}

#[test]
fn an_invite_key_must_be_a_valid_point() {
    let (_, fields) = invite_body(true);
    let mut bytes = expected(2, &fields);
    let key_at = prefix(2).len() + 16;
    // The identity point and an invalid encoding.
    for bad in [
        [0u8; 32],
        {
            let mut b = [0u8; 32];
            b[0] = 1;
            b
        },
        [0xff; 32],
    ] {
        bytes[key_at..key_at + 32].copy_from_slice(&bad);
        assert!(is_corrupted(MembershipHeader::decode(&bytes)));
    }
}

// ----- the private part ----------------------------------------------------------------------

fn private_goldens() -> Vec<(Private, EntryKind, Vec<u8>)> {
    let mut create = vec![1u8];
    create.extend_from_slice(&5u32.to_le_bytes());
    create.extend_from_slice(b"Space");
    create.extend_from_slice(&30u32.to_le_bytes());
    create.push(1);
    create.extend_from_slice(&3u32.to_le_bytes());
    create.extend_from_slice(b"Ann");

    let mut join = vec![2u8];
    join.extend_from_slice(&3u32.to_le_bytes());
    join.extend_from_slice(b"Bob");

    let mut profile = vec![3u8];
    profile.extend_from_slice(&2u32.to_le_bytes());
    profile.extend_from_slice(b"Cy");

    let mut settings = vec![4u8];
    settings.extend_from_slice(&0u32.to_le_bytes());
    settings.push(0);

    vec![
        (
            Private::Create {
                space_name: "Space".to_owned(),
                settings: SpaceSettings {
                    trash_retention_days: 30,
                    editors_may_link: true,
                },
                profile_name: "Ann".to_owned(),
            },
            EntryKind::Create,
            create,
        ),
        (
            Private::Join {
                profile_name: "Bob".to_owned(),
            },
            EntryKind::Join,
            join,
        ),
        (
            Private::Profile {
                display_name: "Cy".to_owned(),
            },
            EntryKind::Profile,
            profile,
        ),
        (
            Private::Settings {
                settings: SpaceSettings {
                    trash_retention_days: 0,
                    editors_may_link: false,
                },
            },
            EntryKind::Settings,
            settings,
        ),
        (Private::None, EntryKind::Leave, Vec::new()),
    ]
}

#[test]
fn the_private_part_has_the_documented_layout() {
    for (private, kind, bytes) in private_goldens() {
        assert_eq!(codec::encode_private(&private), bytes, "{kind:?}");
        assert_eq!(codec::decode_private(kind, &bytes).unwrap(), private);
    }
    // Every kind without private fields takes the empty plaintext, and only that.
    for kind in [
        EntryKind::Invite,
        EntryKind::InviteRevoke,
        EntryKind::RoleSet,
        EntryKind::Remove,
        EntryKind::Leave,
        EntryKind::Rekey,
        EntryKind::Grant,
        EntryKind::Close,
    ] {
        assert_eq!(codec::decode_private(kind, &[]).unwrap(), Private::None);
        assert!(is_corrupted(codec::decode_private(kind, &[1])));
    }
}

#[test]
fn the_private_part_decodes_strictly() {
    for (_, kind, bytes) in private_goldens() {
        if bytes.is_empty() {
            continue;
        }
        for len in 0..bytes.len() {
            assert!(
                is_corrupted(codec::decode_private(kind, &bytes[..len])),
                "{kind:?} cut at {len}"
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(is_corrupted(codec::decode_private(kind, &longer)));
        // Another kind's tag.
        for tag in [0u8, 1, 2, 3, 4, 5] {
            let mut other = bytes.clone();
            other[0] = tag;
            assert_eq!(
                codec::decode_private(kind, &other).is_ok(),
                tag == bytes[0],
                "{kind:?} tag {tag}"
            );
        }
    }
    // The tag of one kind under another kind.
    let (_, _, join) = &private_goldens()[1];
    assert!(is_corrupted(codec::decode_private(
        EntryKind::Profile,
        join
    )));
    assert!(is_corrupted(codec::decode_private(EntryKind::Create, join)));
}

#[test]
fn profile_names_are_one_to_one_hundred_characters() {
    let max = kb_types::sharing::PROFILE_NAME_MAX_CHARS;
    let name_of = |name: &str| {
        codec::encode_private(&Private::Join {
            profile_name: name.to_owned(),
        })
    };
    assert!(is_corrupted(codec::decode_private(
        EntryKind::Join,
        &name_of("")
    )));
    assert!(codec::decode_private(EntryKind::Join, &name_of("x")).is_ok());
    // Characters, not bytes: a hundred four-byte characters fit.
    assert!(codec::decode_private(EntryKind::Join, &name_of(&"\u{1F4F1}".repeat(max))).is_ok());
    assert!(is_corrupted(codec::decode_private(
        EntryKind::Join,
        &name_of(&"\u{1F4F1}".repeat(max + 1))
    )));
    assert!(is_corrupted(codec::decode_private(
        EntryKind::Join,
        &name_of(&"x".repeat(max + 1))
    )));
    // Not UTF-8.
    let mut bad = name_of("abc");
    let last = bad.len() - 1;
    bad[last] = 0xff;
    assert!(is_corrupted(codec::decode_private(EntryKind::Join, &bad)));
    // A boolean that is not 0 or 1 in the settings.
    let mut settings = codec::encode_private(&Private::Settings {
        settings: SpaceSettings::default(),
    });
    let last = settings.len() - 1;
    settings[last] = 2;
    assert!(is_corrupted(codec::decode_private(
        EntryKind::Settings,
        &settings
    )));
}
