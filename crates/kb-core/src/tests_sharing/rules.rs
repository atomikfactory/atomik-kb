//! Milestone 13, slice 1, the single-user rules in shared spaces (ADR-0047) and the settings, the AI
//! switch, the profile, the invites' rules, and what a lost compare-and-swap does to the request
//! side. Helpers: `requests`; teams: `scenarios`.

use kb_storage::{AiIndexQueries, DocQueries};
use kb_types::ai::{
    AI_SETTINGS_SETTING, AiChunkVector, AiIndexInfo, AiIndexWork, AiScope, AiSource,
};
use kb_types::{
    AttachmentInfo, InviteKind, InviteSessionState, InviteStatus, MembersInfo, Request, Response,
    SpaceShareState,
};
use serde_json::{Value, json};

use super::requests::*;
use super::*;

// ----- helpers ----------------------------------------------------------------------------------

fn server_and_team(role: MemberRole) -> Team {
    team(role)
}

use super::scenarios::{Team, team};

fn personal_space(dev: &mut Dev) -> SpaceId {
    crate::test_support::spaces(&mut dev.core)
        .into_iter()
        .find(|info| info.share_state == SpaceShareState::Personal)
        .expect("a personal space")
        .id
}

fn settings_of(dev: &mut Dev, space: SpaceId) -> kb_types::sharing::SpaceSettings {
    members(dev, space).settings
}

fn members_response(response: Response) -> MembersInfo {
    match response {
        Response::Members(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_settings(
    dev: &mut Dev,
    space: SpaceId,
    days: Option<u32>,
    link: Option<bool>,
) -> Result<MembersInfo, ApiError> {
    call(
        dev,
        Request::SpaceSettingsSet {
            space_id: space,
            trash_retention_days: days,
            editors_may_link: link,
        },
    )
    .map(members_response)
}

fn image_block(id: u8, attachment: kb_types::AttachmentId) -> Value {
    json!({ "type": "image", "attrs": {
        "id": kb_types::BlockId::from_random_bytes([id; 16]).to_string(),
        "attachmentId": attachment.to_string(),
        "alt": ""
    }})
}

/// Appends portable blocks to a page's document, like an import or the MCP server does.
fn append_blocks(dev: &mut Dev, page_id: PageId, blocks: Value) {
    let core = &mut dev.core;
    let appended = core
        .open_page(page_id)
        .unwrap()
        .model
        .append_portable(&json!({ "type": "doc", "content": blocks }))
        .unwrap();
    assert!(appended > 0);
    core.persist_page(
        page_id,
        None,
        kb_storage::UpdateOrigin::Local,
        kb_types::ChangeSource::Core,
    )
    .unwrap();
    core.page_doc_close(page_id).unwrap();
}

fn upload(dev: &mut Dev, space: SpaceId, name: &str, bytes: &[u8]) -> AttachmentInfo {
    dev.upload_attachment(space, name, bytes)
}

// ----- favourites -------------------------------------------------------------------------------

#[test]
fn favourites_are_each_member_s_own_and_live_in_the_account_document() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        notes,
        ..
    } = server_and_team(MemberRole::Editor);
    // Alice's pin moved to her account document at the share.
    assert_eq!(tree(&mut alice.core, space).favorites, vec![plan]);
    assert_eq!(
        alice
            .core
            .docs
            .account
            .as_ref()
            .unwrap()
            .model
            .favorites(space),
        vec![plan]
    );
    // Bob has none, and pins the other page; the lists never mix.
    assert!(tree(&mut bob.core, space).favorites.is_empty());
    call(
        &mut bob,
        Request::PageFavoriteSet {
            page_id: notes,
            favorite: true,
        },
    )
    .unwrap();
    call(
        &mut bob,
        Request::PageFavoriteSet {
            page_id: plan,
            favorite: true,
        },
    )
    .unwrap();
    call(
        &mut bob,
        Request::PageFavoriteMove {
            page_id: plan,
            before: Some(notes),
        },
    )
    .unwrap();
    assert_eq!(tree(&mut bob.core, space).favorites, vec![plan, notes]);
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(tree(&mut alice.core, space).favorites, vec![plan]);
    assert_eq!(tree(&mut bob.core, space).favorites, vec![plan, notes]);
    // A page in the trash cannot be pinned, and a trashed pin is hidden until the page is back.
    call(&mut alice, Request::PageTrash { page_id: notes }).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(tree(&mut bob.core, space).favorites, vec![plan], "hidden");
    invalid_field(
        &fails(
            &mut bob,
            Request::PageFavoriteSet {
                page_id: notes,
                favorite: true,
            },
        ),
        "pageId",
    );
    call(&mut alice, Request::PageRestore { page_id: notes }).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(tree(&mut bob.core, space).favorites, vec![plan, notes]);
}

fn invalid_field(error: &ApiError, field: &str) {
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(error.details, Some(json!({ "field": field })), "{error:?}");
}

// ----- trash, purge and the settings ------------------------------------------------------------

#[test]
fn the_trash_expires_by_the_spaces_setting_on_owner_devices_only() {
    let Team {
        mut alice,
        mut bob,
        space,
        notes,
        ..
    } = server_and_team(MemberRole::Editor);
    // The settings: a field left out stays, and the limits.
    assert_eq!(settings_of(&mut alice, space).trash_retention_days, 30);
    let panel = set_settings(&mut alice, space, Some(2), None).unwrap();
    assert_eq!(panel.settings.trash_retention_days, 2);
    let panel = set_settings(&mut alice, space, None, Some(true)).unwrap();
    assert_eq!(
        (
            panel.settings.trash_retention_days,
            panel.settings.editors_may_link
        ),
        (2, true)
    );
    invalid_field(
        &set_settings(&mut alice, space, Some(4000), None).unwrap_err(),
        "trashRetentionDays",
    );
    forbidden_role_of(&set_settings(&mut bob, space, Some(9), None).unwrap_err());
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(settings_of(&mut bob, space).trash_retention_days, 2);

    // A trashed page expires by the space's two days on Alice's device, not by the device's thirty.
    call(&mut alice, Request::PageTrash { page_id: notes }).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    alice.clock.advance(DAY_MS);
    assert_eq!(alice.core.trash_expire().unwrap(), 0);
    alice.clock.advance(2 * DAY_MS + 1);
    // An Editor's device never purges a shared space, whatever its own setting says.
    bob.core
        .dispatch(Request::SettingsSet {
            key: crate::TRASH_RETENTION_SETTING.into(),
            value: json!(1),
        })
        .unwrap();
    bob.clock.advance(10 * DAY_MS);
    assert_eq!(bob.core.trash_expire().unwrap(), 0);
    forbidden_role_of(&fails(&mut bob, Request::PagePurge { page_id: notes }));
    assert_eq!(
        alice.core.trash_expire().unwrap(),
        2,
        "the page just trashed and the one the space came with"
    );
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(
        bob.core.space_of_page(notes).is_err(),
        "the purge reached the other member"
    );
    // The purge was an Owner's: nobody's entry was refused.
    for dev in [&mut alice, &mut bob] {
        assert_eq!(members(dev, space).refused_total, 0, "{}", dev.name);
    }
    // A personal space follows the device setting as before.
    let personal = personal_space(&mut bob);
    let diary = create_page(&mut bob.core, personal, None, "Diary").id;
    call(&mut bob, Request::PageTrash { page_id: diary }).unwrap();
    bob.clock.advance(2 * DAY_MS);
    assert_eq!(bob.core.trash_expire().unwrap(), 1);
    // Zero keeps for ever.
    set_settings(&mut alice, space, Some(0), None).unwrap();
    let plan_child = create_page(&mut alice.core, space, None, "Scratch").id;
    call(
        &mut alice,
        Request::PageTrash {
            page_id: plan_child,
        },
    )
    .unwrap();
    alice.clock.advance(400 * DAY_MS);
    assert_eq!(alice.core.trash_expire().unwrap(), 0);
}

fn forbidden_role_of(error: &ApiError) {
    assert_eq!(error.code, ErrorCode::Forbidden, "{error:?}");
}

#[test]
fn only_an_owner_deletes_files_and_collects_them() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    let bytes = vec![5u8; 40_000];
    let file = upload(&mut bob, space, "bob.bin", &bytes);
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert_eq!(alice.read_attachment(file.attachment_id).unwrap(), bytes);
    // An Editor cannot delete it; an Owner can, and the chunks go from the server too.
    forbidden_role_of(&fails(
        &mut bob,
        Request::AttachmentDelete {
            attachment_id: file.attachment_id,
        },
    ));
    let feed = feed_of(&mut alice, space);
    let chunk = alice
        .core
        .db
        .chunk_sync_by_attachment(file.attachment_id)
        .unwrap()[0]
        .hash;
    assert!(alice.remote.get_chunk_sync(feed, chunk).unwrap().is_some());
    call(
        &mut alice,
        Request::AttachmentDelete {
            attachment_id: file.attachment_id,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(alice.remote.get_chunk_sync(feed, chunk).unwrap().is_none());
    // The garbage collection of an Editor's device leaves a shared space's files alone.
    let kept = upload(&mut alice, space, "kept.bin", &bytes);
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    bob.clock.advance(400 * DAY_MS);
    let summary = bob.core.attachment_gc(bob.clock.now_ms()).unwrap();
    assert_eq!(summary.purged, 0);
    assert!(bob.read_attachment(kept.attachment_id).is_ok());
}

// ----- templates ---------------------------------------------------------------------------------

fn page_of(dev: &mut Dev, page_id: PageId) -> kb_types::PageInfo {
    match call(dev, Request::PageGet { page_id }).unwrap() {
        Response::Page(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

/// A shared space with a page Alice owns and a Date property "Due", settled on both devices.
fn shared_page_with_a_date(team: &mut Team) -> (PageId, kb_types::PropertyId) {
    let Team {
        alice, bob, space, ..
    } = team;
    let page = create_page(&mut alice.core, *space, None, "Meeting").id;
    let due = match call(
        alice,
        Request::PropertyCreate {
            space_id: *space,
            name: "Due".into(),
            kind: kb_types::PropertyKind::Date,
            config: None,
        },
    )
    .unwrap()
    {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    };
    call(
        alice,
        Request::PageSetTemplate {
            page_id: page,
            template: true,
        },
    )
    .unwrap();
    Dev::settle(&mut [alice, bob], 4);
    (page, due.id)
}

#[test]
fn a_viewer_cannot_make_a_template_or_set_a_placeholder() {
    let mut team = server_and_team(MemberRole::Viewer);
    let (page, due) = shared_page_with_a_date(&mut team);
    let Team {
        mut alice, mut bob, ..
    } = team;
    // The flag reached the Viewer, who sees a template and the pages inside it.
    assert!(page_of(&mut bob, page).template);
    for request in [
        Request::PageSetTemplate {
            page_id: page,
            template: false,
        },
        Request::PageSetTemplatePlaceholder {
            page_id: page,
            property_id: due,
            placeholder: Some(kb_types::TemplatePlaceholder::Today),
        },
    ] {
        let refused = fails(&mut bob, request);
        assert_eq!(refused.code, ErrorCode::Forbidden);
        assert_eq!(
            refused.details,
            Some(json!({ "reason": "role", "required": "editor" }))
        );
    }
    assert!(page_of(&mut bob, page).template);
    // The Owner's device did both, and the Viewer reads them after the sync.
    call(
        &mut alice,
        Request::PageSetTemplatePlaceholder {
            page_id: page,
            property_id: due,
            placeholder: Some(kb_types::TemplatePlaceholder::Now),
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    assert_eq!(
        page_of(&mut bob, page).template_placeholders,
        vec![(due, kb_types::TemplatePlaceholder::Now)]
    );
}

#[test]
fn an_editor_makes_templates_and_sets_placeholders_and_the_owner_reads_them() {
    let mut team = server_and_team(MemberRole::Editor);
    let (page, due) = shared_page_with_a_date(&mut team);
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = team;
    let other = create_page(&mut bob.core, space, None, "Notes").id;
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    call(
        &mut bob,
        Request::PageSetTemplate {
            page_id: page,
            template: false,
        },
    )
    .unwrap();
    call(
        &mut bob,
        Request::PageSetTemplate {
            page_id: other,
            template: true,
        },
    )
    .unwrap();
    call(
        &mut bob,
        Request::PageSetTemplatePlaceholder {
            page_id: other,
            property_id: due,
            placeholder: Some(kb_types::TemplatePlaceholder::Today),
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 4);
    assert!(!page_of(&mut alice, page).template);
    let notes = page_of(&mut alice, other);
    assert!(notes.template);
    assert_eq!(
        notes.template_placeholders,
        vec![(due, kb_types::TemplatePlaceholder::Today)]
    );
    // The page index rows of the Owner's device follow what arrived through sync.
    assert!(
        alice
            .core
            .db
            .pages_index_get(other)
            .unwrap()
            .unwrap()
            .in_template
    );
    assert!(
        !alice
            .core
            .db
            .pages_index_get(page)
            .unwrap()
            .unwrap()
            .in_template
    );
}

// ----- moving pages between spaces --------------------------------------------------------------

#[test]
fn pages_move_into_a_shared_space_as_an_editor_and_out_of_it_as_an_owner() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    let personal = personal_space(&mut bob);
    let diary = create_page(&mut bob.core, personal, None, "Diary").id;
    bob.write(diary, "bob diary line");
    let bytes = vec![9u8; 30_000];
    let own = upload(&mut bob, personal, "own.bin", &bytes);
    append_blocks(&mut bob, diary, json!([image_block(1, own.attachment_id)]));
    bob.cycle().unwrap();
    // The plan only reads.
    let plan = match call(
        &mut bob,
        Request::PageMoveToSpacePlan {
            page_id: diary,
            space_id: space,
            parent_id: None,
        },
    )
    .unwrap()
    {
        Response::SpaceMovePlanned(summary) => summary,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!((plan.attachments_moved, plan.attachments_left), (1, 0));
    // Into the shared space an Editor may move.
    call(
        &mut bob,
        Request::PageMoveToSpace {
            page_id: diary,
            space_id: space,
            parent_id: None,
            index: None,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 4);
    assert!(outline_of(&mut alice, space).contains("Diary"));
    assert!(alice.texts(diary).contains(&"bob diary line".to_owned()));
    assert_eq!(
        alice.read_attachment(own.attachment_id).unwrap(),
        bytes,
        "the file the page owns reached the members"
    );
    // Out of it an Editor may not; an Owner may.
    forbidden_role_of(&fails(
        &mut bob,
        Request::PageMoveToSpace {
            page_id: diary,
            space_id: personal,
            parent_id: None,
            index: None,
        },
    ));
    let alice_personal = personal_space(&mut alice);
    call(
        &mut alice,
        Request::PageMoveToSpace {
            page_id: diary,
            space_id: alice_personal,
            parent_id: None,
            index: None,
        },
    )
    .unwrap();
    assert_eq!(
        alice.read_attachment(own.attachment_id).unwrap(),
        bytes,
        "and takes its file with it"
    );
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    assert!(!outline_of(&mut bob, space).contains("Diary"));
}

#[test]
fn a_viewer_cannot_move_pages_into_a_shared_space_but_may_ask_for_the_plan() {
    let Team { mut bob, space, .. } = server_and_team(MemberRole::Viewer);
    let personal = personal_space(&mut bob);
    let diary = create_page(&mut bob.core, personal, None, "Diary").id;
    let moved = fails(
        &mut bob,
        Request::PageMoveToSpace {
            page_id: diary,
            space_id: space,
            parent_id: None,
            index: None,
        },
    );
    assert_eq!(moved.code, ErrorCode::Forbidden);
    assert_eq!(
        moved.details,
        Some(json!({ "reason": "role", "required": "editor" }))
    );
    call(
        &mut bob,
        Request::PageMoveToSpacePlan {
            page_id: diary,
            space_id: space,
            parent_id: None,
        },
    )
    .unwrap();
}

// ----- AI and MCP -------------------------------------------------------------------------------

fn ai_status(dev: &mut Dev) -> AiIndexInfo {
    match call(dev, Request::AiIndexStatus).unwrap() {
        Response::AiIndex(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn ai_work(dev: &mut Dev) -> AiIndexWork {
    match call(dev, Request::AiIndexNext { max_chunks: 32 }).unwrap() {
        Response::AiIndexWork(work) => work,
        other => panic!("unexpected {other:?}"),
    }
}

fn ai_retrieve(dev: &mut Dev, scope: AiScope) -> Vec<AiSource> {
    match call(
        dev,
        Request::AiRetrieve {
            query: "plan".into(),
            vector: None,
            scope,
            limit: None,
        },
    )
    .unwrap()
    {
        Response::AiPassages(passages) => passages,
        other => panic!("unexpected {other:?}"),
    }
}

fn store_work(dev: &mut Dev, work: &AiIndexWork) {
    let model = work.model.clone().unwrap();
    for page in &work.pages {
        let vectors = page
            .chunks
            .iter()
            .filter(|chunk| chunk.needs_vector)
            .map(|chunk| AiChunkVector {
                seq: chunk.seq,
                vector: vec![1.0; 16],
            })
            .collect();
        call(
            dev,
            Request::AiIndexStore {
                page_id: page.page_id,
                source_hash: page.source_hash.clone(),
                model: model.clone(),
                vectors,
            },
        )
        .unwrap();
    }
}

#[test]
fn ai_and_mcp_stay_away_from_a_shared_space_until_its_member_opts_in() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = server_and_team(MemberRole::Editor);
    // Off by default for a shared space, on for a personal one.
    assert!(!space_info(&mut bob, space).ai_enabled);
    assert!(!space_info(&mut alice, space).ai_enabled);
    let personal = personal_space(&mut bob);
    assert!(space_info(&mut bob, personal).ai_enabled);
    call(
        &mut bob,
        Request::SettingsSet {
            key: AI_SETTINGS_SETTING.into(),
            value: json!({ "enabled": true, "embeddingModel": "m", "semanticIndex": true }),
        },
    )
    .unwrap();
    let scope = AiScope::Space { space_id: space };
    assert!(ai_retrieve(&mut bob, scope).is_empty());
    assert!(ai_retrieve(&mut bob, AiScope::All).is_empty());
    assert_eq!(
        ai_status(&mut bob).pages_total,
        0,
        "nothing waits to be indexed"
    );
    assert!(ai_work(&mut bob).pages.is_empty());
    bob.core.drain_events();
    assert!(ai_work(&mut bob).pages.is_empty());
    assert!(
        bob.core
            .drain_events()
            .iter()
            .all(|e| *e != Event::AiIndexChanged),
        "the pages that are not indexed are no change (the indexer would loop on it)"
    );
    // A client cannot be granted the space.
    let created = match call(
        &mut bob,
        Request::McpClientCreate {
            name: "Tool".into(),
        },
    )
    .unwrap()
    {
        Response::McpClientCreated(created) => created,
        other => panic!("unexpected {other:?}"),
    };
    let grant = kb_types::ai::McpGrant {
        space_id: space,
        read: true,
        write: false,
    };
    invalid_field(
        &fails(
            &mut bob,
            Request::McpClientGrantsSet {
                client_id: created.client.id.clone(),
                grants: vec![grant],
            },
        ),
        "grants",
    );
    // Opting in: retrieval, the index and the grants work, for this account only.
    let on = match call(
        &mut bob,
        Request::SpaceAiSet {
            space_id: space,
            enabled: true,
        },
    )
    .unwrap()
    {
        Response::Space(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert!(on.ai_enabled);
    assert!(
        !space_info(&mut alice, space).ai_enabled,
        "Alice did not opt in"
    );
    assert!(!ai_retrieve(&mut bob, scope).is_empty());
    assert!(ai_status(&mut bob).pages_total >= 2);
    let work = ai_work(&mut bob);
    assert!(!work.pages.is_empty());
    store_work(&mut bob, &work);
    assert!(bob.core.db.ai_page_get(plan).unwrap().is_some());
    match call(
        &mut bob,
        Request::McpClientGrantsSet {
            client_id: created.client.id.clone(),
            grants: vec![grant],
        },
    )
    .unwrap()
    {
        Response::McpClient(info) => assert_eq!(info.grants.len(), 1),
        other => panic!("unexpected {other:?}"),
    }
    // Opting out again revokes the grants and clears the index rows of the space.
    call(
        &mut bob,
        Request::SpaceAiSet {
            space_id: space,
            enabled: false,
        },
    )
    .unwrap();
    match call(&mut bob, Request::McpClientList).unwrap() {
        Response::McpClients(list) => assert!(list[0].grants.is_empty()),
        other => panic!("unexpected {other:?}"),
    }
    assert!(bob.core.db.ai_page_get(plan).unwrap().is_none());
    assert!(ai_retrieve(&mut bob, scope).is_empty());
    assert_eq!(ai_status(&mut bob).pages_total, 0);
    // Any member may set it; so is a personal space.
    let off = match call(
        &mut bob,
        Request::SpaceAiSet {
            space_id: personal,
            enabled: false,
        },
    )
    .unwrap()
    {
        Response::Space(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert!(!off.ai_enabled);
}

// ----- profile ----------------------------------------------------------------------------------

#[test]
fn the_profile_name_is_validated_and_every_shared_space_shows_the_new_one() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    match call(&mut alice, Request::ProfileGet).unwrap() {
        Response::Profile(info) => assert_eq!(info.display_name.as_deref(), Some("Alice")),
        other => panic!("unexpected {other:?}"),
    }
    for bad in ["", "   ", &"x".repeat(101), "bad\nname"] {
        invalid_field(
            &fails(
                &mut alice,
                Request::ProfileSet {
                    display_name: bad.to_owned(),
                },
            ),
            "displayName",
        );
    }
    let info = set_profile(&mut alice, "  Alice R.  ");
    assert_eq!(info.display_name.as_deref(), Some("Alice R."));
    // The panel shows it at once, and Bob's after a cycle.
    assert_eq!(members(&mut alice, space).members[0].name, "Alice R.");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(
        members(&mut bob, space)
            .members
            .iter()
            .any(|member| member.name == "Alice R.")
    );
    assert!(bob.events().contains(&Event::PeopleChanged));
    // A name two members share is marked, up to case.
    set_profile(&mut bob, "alice r.");
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let panel = members(&mut alice, space);
    assert!(panel.members.iter().all(|member| member.name_shared));
}

#[test]
fn a_space_needs_sync_and_a_name_before_it_is_shared() {
    let server = server();
    let (mut dev, _) = Dev::first("solo", 5, &server);
    let space = create_space(&mut dev.core, "Mine").id;
    // No server configured.
    invalid_field(
        &fails(&mut dev, Request::SpaceShare { space_id: space }),
        "serverUrl",
    );
    dev.configure();
    // No profile name.
    invalid_field(
        &fails(&mut dev, Request::SpaceShare { space_id: space }),
        "displayName",
    );
    assert!(!plan(&mut dev, space).has_profile_name);
    set_profile(&mut dev, "Solo");
    assert!(plan(&mut dev, space).has_profile_name);
    // A personal space has no members, and cannot be left.
    invalid_field(
        &fails(&mut dev, Request::SpaceMembers { space_id: space }),
        "spaceId",
    );
    invalid_field(
        &fails(&mut dev, Request::SpaceLeave { space_id: space }),
        "spaceId",
    );
    // An unknown space is NotFound.
    let unknown = SpaceId::from_bytes([3; 16]);
    assert_eq!(
        fails(&mut dev, Request::SpaceShare { space_id: unknown }).code,
        ErrorCode::NotFound
    );
    assert_eq!(
        fails(&mut dev, Request::SpaceSharePlan { space_id: unknown }).code,
        ErrorCode::NotFound
    );
}

// ----- link invites: the rules ------------------------------------------------------------------

#[test]
fn a_link_grants_no_owner_lives_at_most_thirty_days_and_there_are_at_most_twenty() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    invalid_field(
        &fails(
            &mut alice,
            Request::InviteLinkCreate {
                space_id: space,
                role: MemberRole::Owner,
                expires_in_days: None,
            },
        ),
        "role",
    );
    invalid_field(
        &fails(
            &mut alice,
            Request::InviteCodeStart {
                space_id: space,
                role: MemberRole::Owner,
            },
        ),
        "role",
    );
    for days in [0, 31] {
        invalid_field(
            &fails(
                &mut alice,
                Request::InviteLinkCreate {
                    space_id: space,
                    role: MemberRole::Viewer,
                    expires_in_days: Some(days),
                },
            ),
            "expiresInDays",
        );
    }
    let thirty = match call(
        &mut alice,
        Request::InviteLinkCreate {
            space_id: space,
            role: MemberRole::Viewer,
            expires_in_days: Some(30),
        },
    )
    .unwrap()
    {
        Response::InviteLink(link) => link,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(thirty.expires_at, alice.clock.now_ms() + 30 * DAY_MS);
    for _ in 1..20 {
        link_invite(&mut alice, space, MemberRole::Viewer);
    }
    invalid_field(
        &fails(
            &mut alice,
            Request::InviteLinkCreate {
                space_id: space,
                role: MemberRole::Viewer,
                expires_in_days: None,
            },
        ),
        "invites",
    );
    let open = members(&mut alice, space)
        .invites
        .iter()
        .filter(|invite| invite.status == InviteStatus::Open)
        .count();
    assert_eq!(open, 20);
    // An Editor creates and revokes nothing.
    forbidden_role_of(&fails(
        &mut bob,
        Request::InviteRevoke {
            space_id: space,
            invite_id: thirty.invite_id,
        },
    ));
    // Revoking: once; an unknown invite is InvalidInput.
    let panel = members_response(
        call(
            &mut alice,
            Request::InviteRevoke {
                space_id: space,
                invite_id: thirty.invite_id,
            },
        )
        .unwrap(),
    );
    assert_eq!(
        panel
            .invites
            .iter()
            .find(|invite| invite.invite_id == thirty.invite_id)
            .unwrap()
            .status,
        InviteStatus::Revoked
    );
    invalid_field(
        &fails(
            &mut alice,
            Request::InviteRevoke {
                space_id: space,
                invite_id: thirty.invite_id,
            },
        ),
        "inviteId",
    );
    invalid_field(
        &fails(
            &mut alice,
            Request::InviteRevoke {
                space_id: space,
                invite_id: kb_types::InviteId::from_bytes([4; 16]),
            },
        ),
        "inviteId",
    );
    // A revoked invite is dead on the server too.
    alice.cycle().unwrap();
    let revoked_link = thirty;
    let mut eve = newcomer("eve", 6, &alice.remote.clone(), "Eve");
    call(
        &mut eve,
        Request::InviteLinkOpen {
            link: revoked_link.link,
        },
    )
    .unwrap();
    let epoch = eve.remote.server_epoch();
    let request = eve.core.invite_open_request(epoch).unwrap().unwrap();
    assert!(matches!(
        eve.remote
            .invite_open_sync(request.invite_id, &request.signature),
        Err(kb_sync::SyncError::NotFound)
    ));
}

#[test]
fn a_link_expires_by_the_clock_and_the_open_invites_of_a_demoted_owner_are_void() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    let short = match call(
        &mut alice,
        Request::InviteLinkCreate {
            space_id: space,
            role: MemberRole::Viewer,
            expires_in_days: Some(1),
        },
    )
    .unwrap()
    {
        Response::InviteLink(link) => link,
        other => panic!("unexpected {other:?}"),
    };
    alice.clock.advance(DAY_MS);
    let listed = members(&mut alice, space).invites;
    assert_eq!(
        listed
            .iter()
            .find(|invite| invite.invite_id == short.invite_id)
            .unwrap()
            .status,
        InviteStatus::Expired
    );
    invalid_field(
        &fails(
            &mut alice,
            Request::InviteRevoke {
                space_id: space,
                invite_id: short.invite_id,
            },
        ),
        "inviteId",
    );
    // Bob becomes an Owner, then demotes Alice: her open invites are void.
    let (alice_account, bob_account) = (alice.account(), bob.account());
    let live = link_invite(&mut alice, space, MemberRole::Viewer);
    call(
        &mut alice,
        Request::MemberRoleSet {
            space_id: space,
            account_id: bob_account,
            role: MemberRole::Owner,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    call(
        &mut bob,
        Request::MemberRoleSet {
            space_id: space,
            account_id: alice_account,
            role: MemberRole::Editor,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let panel = members(&mut bob, space);
    assert_eq!(
        panel
            .invites
            .iter()
            .find(|invite| invite.invite_id == live.invite_id)
            .unwrap()
            .status,
        InviteStatus::Revoked
    );
    let mut eve = newcomer("eve", 6, &bob.remote.clone(), "Eve");
    call(&mut eve, Request::InviteLinkOpen { link: live.link }).unwrap();
    let epoch = eve.remote.server_epoch();
    let request = eve.core.invite_open_request(epoch).unwrap().unwrap();
    assert!(matches!(
        eve.remote
            .invite_open_sync(request.invite_id, &request.signature),
        Err(kb_sync::SyncError::NotFound)
    ));
}

// ----- code invites: the failures ---------------------------------------------------------------

/// A code session up to the digits: both sides in `Confirming`.
fn code_to_digits(inviter: &mut Dev, invitee: &mut Dev, space: SpaceId) -> ([u8; 6], [Cursor; 2]) {
    call(
        inviter,
        Request::InviteCodeStart {
            space_id: space,
            role: MemberRole::Editor,
        },
    )
    .unwrap();
    let (_, offer) = inviter.core.invite_outbox().unwrap().remove(0);
    let (mailbox, expires) = inviter.remote.pair_create_sync(&offer).unwrap();
    inviter
        .core
        .invite_mailbox_opened(mailbox, expires)
        .unwrap();
    inviter.core.invite_posted(0).unwrap();
    let code = invite_info(inviter).code.unwrap();
    call(
        invitee,
        Request::InviteCodeJoin {
            server_url: SERVER_URL.into(),
            code,
        },
    )
    .unwrap();
    invitee
        .core
        .invite_mailbox_opened(mailbox, expires)
        .unwrap();
    let mut cursors = [Cursor::default(), Cursor::default()];
    cursors[0].0 = 1;
    pump(inviter, invitee, mailbox, &mut cursors);
    (mailbox, cursors)
}

#[test]
fn a_code_session_can_be_cancelled_and_the_other_side_hears_of_it() {
    let Team {
        mut alice, space, ..
    } = server_and_team(MemberRole::Editor);
    let server = alice.remote.clone();
    let mut carol = newcomer("carol", 3, &server, "Carol");
    let (mailbox, mut cursors) = code_to_digits(&mut alice, &mut carol, space);
    assert_eq!(
        invite_info(&mut alice).state,
        InviteSessionState::Confirming
    );
    // The invitee cancels: an abort goes through the mailbox and the inviter ends cancelled.
    match call(&mut carol, Request::InviteCancel).unwrap() {
        Response::Invite(info) => assert_eq!(info.state, InviteSessionState::Cancelled),
        other => panic!("unexpected {other:?}"),
    }
    pump(&mut alice, &mut carol, mailbox, &mut cursors);
    assert_eq!(invite_info(&mut alice).state, InviteSessionState::Cancelled);
    // Cancelling again changes nothing; a new session replaces a finished one.
    call(&mut alice, Request::InviteCancel).unwrap();
    assert_eq!(invite_info(&mut alice).state, InviteSessionState::Cancelled);
    call(
        &mut alice,
        Request::InviteCodeStart {
            space_id: space,
            role: MemberRole::Viewer,
        },
    )
    .unwrap();
    assert_eq!(invite_info(&mut alice).state, InviteSessionState::Starting);
    // A session that is running is replaced too.
    call(
        &mut alice,
        Request::InviteCodeStart {
            space_id: space,
            role: MemberRole::Commenter,
        },
    )
    .unwrap();
    assert_eq!(invite_info(&mut alice).role, Some(MemberRole::Commenter));
    // Nothing to confirm yet, or at all.
    invalid_field(&fails(&mut carol, Request::InviteConfirm), "invite");
}

#[test]
fn a_code_invite_ends_failed_when_the_inviter_stopped_being_an_owner_before_confirming() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    let server = alice.remote.clone();
    let (alice_account, bob_account) = (alice.account(), bob.account());
    call(
        &mut alice,
        Request::MemberRoleSet {
            space_id: space,
            account_id: bob_account,
            role: MemberRole::Owner,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let mut carol = newcomer("carol", 3, &server, "Carol");
    let (mailbox, mut cursors) = code_to_digits(&mut alice, &mut carol, space);
    call(&mut carol, Request::InviteConfirm).unwrap();
    pump(&mut alice, &mut carol, mailbox, &mut cursors);
    assert_eq!(
        invite_info(&mut alice).state,
        InviteSessionState::PeerConfirming
    );
    // Bob demotes Alice before she confirms.
    call(
        &mut bob,
        Request::MemberRoleSet {
            space_id: space,
            account_id: alice_account,
            role: MemberRole::Editor,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    // The inviter confirms: the role gate refuses, the session ends failed with a reason, and no
    // `Invite` was written.
    let info = match call(&mut alice, Request::InviteConfirm).unwrap() {
        Response::Invite(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(info.state, InviteSessionState::Failed);
    assert!(info.error.unwrap().contains("no longer invite"));
    assert!(alice.core.invite_membership_pending().unwrap().is_none());
}

#[test]
fn the_checks_of_the_invitee_requests_name_the_field() {
    let server = server();
    let mut carol = newcomer("carol", 3, &server, "Carol");
    invalid_field(
        &fails(
            &mut carol,
            Request::InviteCodeJoin {
                server_url: "ftp://nope".into(),
                code: kb_keys::pairing::PairingCode { mailbox_id: [1; 6] }
                    .display("https://nope.example.test"),
            },
        ),
        "serverUrl",
    );
    invalid_field(
        &fails(
            &mut carol,
            Request::InviteCodeJoin {
                server_url: SERVER_URL.into(),
                code: "short".into(),
            },
        ),
        "code",
    );
    for bad in [
        "",
        "https://example.com",
        "atomik-kb://page/1",
        "atomik-kb://invite?server=x#abc",
    ] {
        invalid_field(
            &fails(&mut carol, Request::InviteLinkOpen { link: bad.into() }),
            "link",
        );
    }
    // No workspace: nothing to join with.
    let fx = Fixture::new();
    let (core, _) = open_core(&fx, 77, 1_000_000);
    let mut bare = Dev {
        name: "bare",
        _fx: fx,
        core,
        remote: server.clone(),
        authed: false,
        edits: 0,
        last_report: None,
        log: Vec::new(),
        clock: Arc::new(FixedClock::new(0)),
    };
    invalid_field(
        &fails(
            &mut bare,
            Request::InviteCodeJoin {
                server_url: SERVER_URL.into(),
                code: kb_keys::pairing::PairingCode { mailbox_id: [1; 6] }.display(SERVER_URL),
            },
        ),
        "workspace",
    );
    // The status of a core with no session.
    match call(&mut carol, Request::InviteStatus).unwrap() {
        Response::Invite(info) => {
            assert_eq!(info.state, InviteSessionState::Idle);
            assert!(!info.verified);
        }
        other => panic!("unexpected {other:?}"),
    }
    let _ = InviteKind::Code;
}

// ----- the compare-and-swap lost -----------------------------------------------------------------

#[test]
fn what_a_lost_compare_and_swap_drops_is_written_again_on_the_new_log() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    let bob_account = bob.account();
    // Bob becomes an Owner; both Owners act before either hears of the other.
    call(
        &mut alice,
        Request::MemberRoleSet {
            space_id: space,
            account_id: bob_account,
            role: MemberRole::Owner,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let link = match call(
        &mut alice,
        Request::InviteLinkCreate {
            space_id: space,
            role: MemberRole::Viewer,
            expires_in_days: None,
        },
    )
    .unwrap()
    {
        Response::InviteLink(link) => link,
        other => panic!("unexpected {other:?}"),
    };
    set_settings(&mut bob, space, Some(9), None).unwrap();
    bob.cycle().unwrap();
    // Alice's `Invite` was minted on a log that Bob's `Settings` has outgrown: her compare-and-swap
    // loses, the core re-reads the log and writes the invite again.
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let panel = members(&mut alice, space);
    assert_eq!(panel.settings.trash_retention_days, 9);
    let invite = panel
        .invites
        .iter()
        .find(|invite| invite.invite_id == link.invite_id)
        .expect("the invite is in the log");
    assert_eq!(
        (invite.kind, invite.status),
        (InviteKind::Link, InviteStatus::Open)
    );
    // And it works: a newcomer opens it on the server.
    let mut eve = newcomer("eve", 6, &alice.remote.clone(), "Eve");
    let preview = open_link(&mut eve, &link.link);
    assert_eq!(preview.state, InviteSessionState::Previewing);
}

#[test]
fn a_file_the_moved_page_shares_with_a_page_that_stays_is_copied_under_a_fresh_key() {
    use kb_storage::AttachmentQueries;
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    let personal = personal_space(&mut bob);
    let moving = create_page(&mut bob.core, personal, None, "Moving").id;
    let staying = create_page(&mut bob.core, personal, None, "Staying").id;
    let bytes = vec![3u8; 30_000];
    let shared_file = upload(&mut bob, personal, "both.bin", &bytes);
    append_blocks(
        &mut bob,
        moving,
        json!([image_block(1, shared_file.attachment_id)]),
    );
    append_blocks(
        &mut bob,
        staying,
        json!([image_block(2, shared_file.attachment_id)]),
    );
    bob.cycle().unwrap();
    let summary = match call(
        &mut bob,
        Request::PageMoveToSpace {
            page_id: moving,
            space_id: space,
            parent_id: None,
            index: None,
        },
    )
    .unwrap()
    {
        Response::PagesMovedToSpace(summary) => summary,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        (summary.attachments_moved, summary.attachments_left),
        (0, 1),
        "the file stays where the other page uses it"
    );
    // The moved page names a copy that lives in the shared space; the staying page the original.
    let moved_refs = bob.core.db.attachment_ids_referenced_by(moving).unwrap();
    let staying_refs = bob.core.db.attachment_ids_referenced_by(staying).unwrap();
    assert_eq!(staying_refs, vec![shared_file.attachment_id]);
    assert_eq!(moved_refs.len(), 1);
    let copy = moved_refs[0];
    assert_ne!(copy, shared_file.attachment_id, "a new attachment");
    assert_eq!(
        bob.core.db.attachment_get(copy).unwrap().unwrap().space_id,
        space
    );
    assert_eq!(
        bob.core
            .db
            .attachment_get(shared_file.attachment_id)
            .unwrap()
            .unwrap()
            .space_id,
        personal
    );
    assert_eq!(bob.read_attachment(copy).unwrap(), bytes);
    assert_eq!(
        bob.read_attachment(shared_file.attachment_id).unwrap(),
        bytes
    );
    // A fresh key: a record of its own under the shared space, none for the original there.
    let subject = |id| kb_keys::KeySubject::Attachment { attachment_id: id };
    assert!(
        bob.core
            .security
            .keyring
            .record(space, &subject(copy))
            .is_some()
    );
    assert!(
        bob.core
            .security
            .keyring
            .record(space, &subject(shared_file.attachment_id))
            .is_none()
    );
    // The members read the copy; they never get the original.
    Dev::settle(&mut [&mut bob, &mut alice], 4);
    assert_eq!(alice.read_attachment(copy).unwrap(), bytes);
    assert!(
        alice
            .core
            .db
            .attachment_get(shared_file.attachment_id)
            .unwrap()
            .is_none()
    );
    // Purging the copy (an Owner may) leaves the original alone.
    call(
        &mut alice,
        Request::AttachmentDelete {
            attachment_id: copy,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(
        bob.read_attachment(shared_file.attachment_id).unwrap(),
        bytes
    );
}

// ----- parity with the in-memory mirror ----------------------------------------------------------

fn keys_of(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

fn is_safety_number(text: &str) -> bool {
    let groups: Vec<&str> = text.split(' ').collect();
    groups.len() == 6
        && groups
            .iter()
            .all(|group| group.len() == 5 && group.bytes().all(|byte| byte.is_ascii_digit()))
}

/// What `packages/core-client/src/memory-sharing.test.ts` expects of the mirror (the tests named in
/// the comments), checked against the core: the wire shapes, the link strings, the idle session, the
/// errors' fields and the invite statuses.
#[test]
fn the_answers_have_the_shapes_and_texts_the_mirror_documents() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = server_and_team(MemberRole::Editor);
    // "the contract's wire shapes": a member row has no safetyNumber for this account and no viaKitAt.
    let panel = members(&mut alice, space);
    let me = panel.members.iter().find(|m| m.is_me).unwrap();
    assert_eq!(
        keys_of(&serde_json::to_value(me).unwrap()),
        [
            "accountId",
            "deviceCount",
            "isMe",
            "joinedAt",
            "name",
            "nameShared",
            "refusedChanges",
            "role",
            "verified"
        ]
    );
    // "lists the members in join order with names, roles, safety numbers and the verified mark".
    let names: Vec<(&str, MemberRole, bool)> = panel
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.role, m.is_me))
        .collect();
    assert_eq!(
        names,
        vec![
            ("Alice", MemberRole::Owner, true),
            ("Bob", MemberRole::Editor, false)
        ]
    );
    let joined: Vec<i64> = panel.members.iter().map(|m| m.joined_at).collect();
    assert!(
        joined.windows(2).all(|pair| pair[0] <= pair[1]),
        "join order"
    );
    let bobs = member_of(&panel, "Bob");
    assert!(is_safety_number(bobs.safety_number.as_deref().unwrap()));
    assert!(bobs.verified);
    // The listing: a personal space carries no sharing field, a shared one its role and members and
    // AI off ("lists a personal space without any sharing field, and a shared one with its role,
    // members and AI off").
    let personal = personal_space(&mut alice);
    let personal_keys = keys_of(&serde_json::to_value(space_info(&mut alice, personal)).unwrap());
    assert!(
        [
            "shareState",
            "role",
            "archiveReason",
            "memberCount",
            "aiEnabled"
        ]
        .iter()
        .all(|key| !personal_keys.iter().any(|k| k == key)),
        "{personal_keys:?}"
    );
    let shared = serde_json::to_value(space_info(&mut alice, space)).unwrap();
    assert_eq!(shared["shareState"], "Shared");
    assert_eq!(shared["role"], "owner");
    assert_eq!(shared["memberCount"], 2);
    assert_eq!(shared["aiEnabled"], false);
    assert!(shared.get("archiveReason").is_none());
    // "makes a link that grants a role below Owner, shows the secret once and lists the invite".
    let link = link_invite(&mut alice, space, MemberRole::Editor);
    assert_eq!(
        keys_of(&serde_json::to_value(&link).unwrap()),
        ["expiresAt", "inviteId", "link", "role", "webLink"]
    );
    assert_eq!(link.expires_at, alice.clock.now_ms() + 7 * DAY_MS);
    let (head, secret) = link.link.split_once('#').unwrap();
    assert_eq!(
        head,
        "atomik-kb://invite?server=https%3A%2F%2Fsync.example.test"
    );
    assert!(
        secret.len() == 52
            && secret
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
    );
    let listed = members(&mut alice, space)
        .invites
        .into_iter()
        .find(|invite| invite.invite_id == link.invite_id)
        .unwrap();
    assert_eq!(
        keys_of(&serde_json::to_value(&listed).unwrap()),
        [
            "createdAt",
            "createdBy",
            "expiresAt",
            "inviteId",
            "kind",
            "role",
            "status"
        ]
    );
    assert_eq!(listed.created_by, alice.account());
    assert_eq!(listed.kind, InviteKind::Link);
    assert_eq!(listed.status, InviteStatus::Open);
    // "tells that a link of this account's own space leads nowhere new".
    let own = open_link(&mut alice, &link.link);
    assert_eq!(own.state, InviteSessionState::Failed);
    assert_eq!(
        own.error.as_deref(),
        Some("you are already a member of this space")
    );
    // The idle session ("answers every sharing request with its documented type").
    let mut zed = newcomer("zed", 9, &alice.remote.clone(), "Zed");
    let idle = invite_info(&mut zed);
    assert_eq!(
        keys_of(&serde_json::to_value(&idle).unwrap()),
        ["flow", "side", "state", "verified"]
    );
    assert_eq!(idle.state, InviteSessionState::Idle);
    // "closes a space as an Owner": closedAt is absent until the Close is in the log.
    assert!(
        serde_json::to_value(members(&mut alice, space))
            .unwrap()
            .get("closedAt")
            .is_none()
    );
    // "refuses every write of a viewer with Forbidden": the wire form of the error.
    let error = fails(&mut bob, Request::SpaceClose { space_id: space });
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["code"], "Forbidden");
    assert_eq!(
        wire["details"],
        json!({ "reason": "role", "required": "owner" })
    );
}

// ----- adversarial invites ---------------------------------------------------------------------------

/// The invitee's side up to the server's answer to the open request, which the caller may tamper
/// with before the core sees it.
fn opened_for(member: &mut Dev, link: &str) -> kb_sync::v2::InviteOpened {
    call(
        member,
        Request::InviteLinkOpen {
            link: link.to_owned(),
        },
    )
    .unwrap();
    let epoch = member.remote.server_epoch();
    let request = member.core.invite_open_request(epoch).unwrap().unwrap();
    member
        .remote
        .invite_open_sync(request.invite_id, &request.signature)
        .unwrap()
}

fn assert_failed_unverified(member: &mut Dev) {
    let info = invite_info(member);
    assert_eq!(info.state, InviteSessionState::Failed, "{info:?}");
    assert_eq!(
        info.error.as_deref(),
        Some("this invite could not be verified")
    );
    // Nothing was registered, and the key was never taken.
    assert!(member.core.invite_join().unwrap().is_none());
    assert!(
        member
            .core
            .db
            .sync_feed_list()
            .unwrap()
            .iter()
            .all(|row| row.kind == kb_types::sync::FeedKind::Personal)
    );
}

#[test]
fn a_server_that_tampers_with_the_invites_log_ends_the_session_failed() {
    let Team {
        mut alice,
        space,
        feed,
        ..
    } = server_and_team(MemberRole::Editor);
    let server = alice.remote.clone();
    let link = link_invite(&mut alice, space, MemberRole::Editor);

    // 1. The log without the Invite: nothing says an Owner invited anyone.
    let mut carol = newcomer("carol", 3, &server, "Carol");
    let mut opened = opened_for(&mut carol, &link.link);
    opened.log.pop();
    verify_opened(&mut carol, &opened);
    assert_failed_unverified(&mut carol);

    // 2. One byte of an entry changed: the signature (or the encoding) breaks.
    let mut dave = newcomer("dave", 4, &server, "Dave");
    let mut opened = opened_for(&mut dave, &link.link);
    let last = opened.log.last_mut().unwrap();
    let at = last.bytes.len() / 2;
    last.bytes[at] ^= 0x40;
    verify_opened(&mut dave, &opened);
    assert_failed_unverified(&mut dave);

    // 3. An invite whose sealed key is not the space's: the commitment of the epoch refuses it.
    let secret = InviteSecret::generate(&SeededRng::new(4242));
    let wrong = kb_keys::space_keys::SpaceKey::generate(&SeededRng::new(77));
    let bundle = kb_keys::invite::LinkBundle {
        epoch: 1,
        space_key: wrong,
        space_name: "Project".into(),
        inviter_name: "Alice".into(),
    };
    let enc = kb_keys::invite::seal_link_bundle(&bundle, &secret, &SeededRng::new(5));
    alice
        .core
        .membership_mint(
            feed,
            EntryBody::Invite {
                invite_id: invite_id(&secret),
                invite_pub: invite_signing_key(&secret).verifying_key(),
                role: MemberRole::Editor,
                expires_at: alice.clock.now_ms() + DAY_MS,
                bound_account: None,
                verified: false,
                enc_bundle: Some(enc),
            },
            Private::None,
            None,
        )
        .unwrap();
    alice.cycle().unwrap();
    let forged_link = format!(
        "atomik-kb://invite?server=https%3A%2F%2Fsync.example.test#{}",
        secret.to_base32()
    );
    let mut erin = newcomer("erin", 5, &server, "Erin");
    let opened = opened_for(&mut erin, &forged_link);
    verify_opened(&mut erin, &opened);
    assert_failed_unverified(&mut erin);

    // The honest link still works for somebody else.
    let mut frank = newcomer("frank", 6, &server, "Frank");
    let preview = open_link(&mut frank, &link.link);
    assert_eq!(preview.state, InviteSessionState::Previewing);
}

// ----- reminders ---------------------------------------------------------------------------------

/// A paragraph with a date mention that reminds at 09:00 of `date`.
fn reminder_block(id: u8, date: &str) -> Value {
    json!({ "type": "paragraph",
        "attrs": { "id": kb_types::BlockId::from_random_bytes([id; 16]).to_string() },
        "content": [
            { "type": "text", "text": "call " },
            { "type": "dateMention", "attrs": {
                "id": kb_types::BlockId::from_random_bytes([id.wrapping_add(50); 16]).to_string(),
                "date": date, "time": "09:00", "reminder": 0 } }
        ] })
}

#[test]
fn a_reminder_of_a_shared_space_fires_on_an_owners_devices_only() {
    let Team {
        mut alice,
        mut bob,
        plan,
        ..
    } = server_and_team(MemberRole::Editor);
    // Bob, an Editor, sets a reminder in the shared page and another in his personal space.
    append_blocks(&mut bob, plan, json!([reminder_block(1, "1970-01-03")]));
    let personal = personal_space(&mut bob);
    let diary = create_page(&mut bob.core, personal, None, "Diary").id;
    append_blocks(&mut bob, diary, json!([reminder_block(2, "1970-01-03")]));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    for dev in [&mut alice, &mut bob] {
        dev.clock.advance(4 * DAY_MS);
    }
    // Alice's device is an Owner's: the shared reminder fires there. Bob's fires only the one he set
    // in his own space (ADR-0047: a mention carries no `reminderFor` yet, the Owners' fallback).
    assert_eq!(alice.core.reminders_tick().unwrap(), 1);
    let fired: Vec<Event> = alice
        .core
        .drain_events()
        .into_iter()
        .filter(|event| matches!(event, Event::ReminderDue(_)))
        .collect();
    assert_eq!(fired.len(), 1);
    assert_eq!(bob.core.reminders_tick().unwrap(), 1);
    let fired: Vec<Event> = bob
        .core
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ReminderDue(reminder) => Some(Event::ReminderDue(reminder)),
            _ => None,
        })
        .collect();
    match &fired[0] {
        Event::ReminderDue(reminder) => assert_eq!(reminder.page_id, diary),
        other => panic!("unexpected {other:?}"),
    }
}
