//! `Request::Search` (ADR-0040, item 9): a page matches when every word is somewhere in it, fuzzy
//! title hits after the full-text ones, and the `edited_after` filter.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_model::EDIT_STAMP_INTERVAL_MS;
use kb_types::{Request, Response, SearchHit, SearchScope, SpaceId};

use crate::Core;
use crate::test_support::{Fixture, create_page, create_workspace, spaces, write_text};

/// The filters of a search; everything else takes the core's default.
#[derive(Default)]
struct Ask<'a> {
    query: &'a str,
    space_id: Option<SpaceId>,
    scope: Option<SearchScope>,
    limit: Option<u32>,
    edited_after: Option<i64>,
}

fn run(core: &mut Core, ask: Ask<'_>) -> Vec<SearchHit> {
    match core
        .dispatch(Request::Search {
            query: ask.query.into(),
            space_id: ask.space_id,
            scope: ask.scope,
            limit: ask.limit,
            edited_after: ask.edited_after,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

fn search(core: &mut Core, query: &str) -> Vec<SearchHit> {
    run(
        core,
        Ask {
            query,
            ..Ask::default()
        },
    )
}

fn titles(hits: &[SearchHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.title.as_str()).collect()
}

/// The highlighted parts of a hit's snippet, cut by UTF-16 offsets.
fn highlighted(hit: &SearchHit) -> Vec<String> {
    let units: Vec<u16> = hit.snippet.encode_utf16().collect();
    hit.highlights
        .iter()
        .map(|range| String::from_utf16(&units[range.start as usize..range.end as usize]).unwrap())
        .collect()
}

fn home(core: &mut Core) -> SpaceId {
    create_workspace(core, "Home", Some("Personal"));
    spaces(core)[0].id
}

#[test]
fn a_page_matches_when_every_word_is_somewhere_in_it() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = home(&mut core);
    let roadmap = create_page(&mut core, space, None, "Roadmap");
    write_text(&mut core, roadmap.id, "Quarter planning notes", 1);
    let other = create_page(&mut core, space, None, "Other");
    write_text(&mut core, other.id, "the roadmap for the quarter", 2);

    // The page whose block holds both words first, then the page whose title and block hold one each.
    let hits = search(&mut core, "roadmap quarter");
    assert_eq!(titles(&hits), ["Other", "Roadmap"], "{hits:?}");
    assert!(hits[0].block_id.is_some());
    assert_eq!(highlighted(&hits[0]), ["roadmap", "quarter"]);
    assert_eq!(hits[1].page_id, roadmap.id, "one hit for the spread page");

    // The row of the page that holds the most of the words is its hit, with those words highlighted.
    let hits = search(&mut core, "roadmap quarter planning");
    assert_eq!(titles(&hits), ["Roadmap"], "{hits:?}");
    assert!(hits[0].block_id.is_some());
    assert_eq!(hits[0].snippet, "Quarter planning notes");
    assert_eq!(highlighted(&hits[0]), ["Quarter", "planning"]);

    // Every word is needed.
    assert!(search(&mut core, "roadmap quarter zebra").is_empty());
    // Prefixes, as everywhere.
    assert_eq!(search(&mut core, "road quart").len(), 2);

    // A titles-only search has one row per page: the words of a title.
    let by_title = run(
        &mut core,
        Ask {
            query: "roadmap quarter",
            scope: Some(SearchScope::Titles),
            ..Ask::default()
        },
    );
    assert!(by_title.is_empty(), "{by_title:?}");

    // A trashed page is not found, spread words or not.
    core.dispatch(Request::PageTrash {
        page_id: roadmap.id,
    })
    .unwrap();
    assert_eq!(titles(&search(&mut core, "roadmap quarter")), ["Other"]);
}

#[test]
fn the_space_filter_applies_to_the_spread_words_too() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let personal = home(&mut core);
    let work = match core
        .dispatch(Request::SpaceCreate {
            name: "Work".into(),
            icon: None,
        })
        .unwrap()
    {
        Response::Space(space) => space.id,
        other => panic!("unexpected {other:?}"),
    };
    for (space, title, seed) in [(personal, "Budget", 1), (work, "Budget again", 2)] {
        let page = create_page(&mut core, space, None, title);
        write_text(&mut core, page.id, "quarterly review", seed);
    }
    let everywhere = search(&mut core, "budget review");
    assert_eq!(everywhere.len(), 2, "{everywhere:?}");
    let only_work = run(
        &mut core,
        Ask {
            query: "budget review",
            space_id: Some(work),
            ..Ask::default()
        },
    );
    assert_eq!(titles(&only_work), ["Budget again"]);
}

#[test]
fn fuzzy_titles_come_after_the_full_text_hits_and_never_repeat_a_page() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = home(&mut core);
    let plan = create_page(&mut core, space, None, "Plan");
    let zone = create_page(&mut core, space, None, "Zone");
    write_text(&mut core, zone.id, "plan b for the weekend", 1);
    let pelican = create_page(&mut core, space, None, "Pelican");
    create_page(&mut core, space, None, "Unrelated");

    // "Plan" (its title) and "Zone" (a block) have the word; "Pelican" has the letters p, l, a, n in order.
    let hits = search(&mut core, "plan");
    assert_eq!(titles(&hits), ["Plan", "Zone", "Pelican"], "{hits:?}");
    assert_eq!(hits[0].page_id, plan.id);
    assert_eq!(hits[1].page_id, zone.id);
    assert_eq!(hits[2].page_id, pelican.id);
    assert_eq!(hits[2].block_id, None, "a fuzzy hit is on the title");
    assert_eq!(hits[2].snippet, "Pelican");
    assert_eq!(highlighted(&hits[2]), ["P", "l", "an"]);

    // The limit counts the fuzzy hits last: they only fill what the others leave.
    let two = run(
        &mut core,
        Ask {
            query: "plan",
            limit: Some(2),
            ..Ask::default()
        },
    );
    assert_eq!(titles(&two), ["Plan", "Zone"]);

    // The titles scope (the `[[` picker) is fuzzy too.
    let picker = run(
        &mut core,
        Ask {
            query: "plan",
            scope: Some(SearchScope::Titles),
            limit: Some(8),
            ..Ask::default()
        },
    );
    assert_eq!(titles(&picker), ["Plan", "Pelican"], "{picker:?}");

    // A typo of a word of four characters or more, and a title found by its initials.
    let typo = search(&mut core, "pelcian");
    assert_eq!(titles(&typo), ["Pelican"], "{typo:?}");
    assert_eq!(titles(&search(&mut core, "unrltd")), ["Unrelated"]);

    // One letter is not enough to look for; no letters, no search.
    assert!(search(&mut core, "e").is_empty());
    assert!(search(&mut core, "...").is_empty());

    // A trashed page is never offered.
    core.dispatch(Request::PageTrash {
        page_id: pelican.id,
    })
    .unwrap();
    assert!(search(&mut core, "pelcian").is_empty());
}

#[test]
fn fuzzy_titles_follow_the_space_filter() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let personal = home(&mut core);
    let work = match core
        .dispatch(Request::SpaceCreate {
            name: "Work".into(),
            icon: None,
        })
        .unwrap()
    {
        Response::Space(space) => space.id,
        other => panic!("unexpected {other:?}"),
    };
    create_page(&mut core, personal, None, "Recipes");
    create_page(&mut core, work, None, "Rice cooking plan");
    let both = search(&mut core, "rcp");
    // The letters at word starts score more: "Rice cooking plan" before "Recipes".
    assert_eq!(titles(&both), ["Rice cooking plan", "Recipes"], "{both:?}");
    let ask = |space_id| Ask {
        query: "rcp",
        space_id,
        ..Ask::default()
    };
    assert_eq!(titles(&run(&mut core, ask(Some(personal)))), ["Recipes"]);
    assert_eq!(
        titles(&run(&mut core, ask(Some(work)))),
        ["Rice cooking plan"]
    );
}

#[test]
fn edited_after_keeps_the_pages_last_edited_at_or_after_it() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = home(&mut core);
    let old = create_page(&mut core, space, None, "Old plan");
    drop(core);
    let mut core = fx.open(2, 5_000);
    create_page(&mut core, space, None, "New plan");
    create_page(&mut core, space, None, "Pelican");

    let after = |core: &mut Core, query, edited_after| {
        titles(&run(
            core,
            Ask {
                query,
                edited_after,
                ..Ask::default()
            },
        ))
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    };
    let mut all = after(&mut core, "plan", None);
    assert_eq!(all.pop().as_deref(), Some("Pelican"), "the fuzzy hit last");
    all.sort();
    assert_eq!(all, ["New plan", "Old plan"]);
    // The fuzzy pass follows the filter too, and the limit is inclusive.
    assert_eq!(
        after(&mut core, "plan", Some(3_000)),
        ["New plan", "Pelican"]
    );
    assert_eq!(
        after(&mut core, "plan", Some(5_000)),
        ["New plan", "Pelican"]
    );
    assert!(after(&mut core, "plan", Some(5_001)).is_empty());

    // Editing a page moves it into the range (the last edit is stamped once a minute at most).
    drop(core);
    let later = 5_000 + EDIT_STAMP_INTERVAL_MS + 5;
    let mut core = fx.open(3, later);
    write_text(&mut core, old.id, "a new line", 1);
    assert_eq!(after(&mut core, "plan", Some(later)), ["Old plan"]);
    // Words spread over the title and a block follow the filter too.
    assert_eq!(after(&mut core, "old line", Some(later)), ["Old plan"]);
    assert!(after(&mut core, "old line", Some(later + 1)).is_empty());
}
