//! `PageCreateFromTemplate` in a shared space (ADR-0056, ADR-0047): the role it needs, and a
//! template that is shared with the space like any page. Helpers: `requests`; teams: `scenarios`.

use kb_types::{PageInfo, PropertyDef, PropertyKind, PropertyValue, Request, Response};

use super::requests::*;
use super::scenarios::{Team, forbidden_role, team};
use super::*;

fn set_template(dev: &mut Dev, page_id: PageId) {
    call(
        dev,
        Request::PageSetTemplate {
            page_id,
            template: true,
        },
    )
    .unwrap();
}

fn create_from(
    dev: &mut Dev,
    template_id: PageId,
    values: Vec<(kb_types::PropertyId, PropertyValue)>,
) -> Result<PageInfo, ApiError> {
    match call(
        dev,
        Request::PageCreateFromTemplate {
            template_id,
            parent_id: None,
            index: None,
            values,
        },
    )? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn page_of(dev: &mut Dev, page_id: PageId) -> PageInfo {
    match call(dev, Request::PageGet { page_id }).unwrap() {
        Response::Page(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_viewer_cannot_make_a_page_from_a_template() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = team(MemberRole::Viewer);
    let template = create_page(&mut alice.core, space, None, "Retro").id;
    set_template(&mut alice, template);
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    assert!(
        page_of(&mut bob, template).template,
        "the template reached the viewer like any page"
    );
    let before = outline_of(&mut bob, space);

    let err = fails(
        &mut bob,
        Request::PageCreateFromTemplate {
            template_id: template,
            parent_id: None,
            index: None,
            values: Vec::new(),
        },
    );
    forbidden_role(&err, "editor");
    assert_eq!(outline_of(&mut bob, space), before, "nothing was written");
}

#[test]
fn an_editor_makes_a_page_from_a_template_of_the_space_with_me_being_the_editor() {
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = team(MemberRole::Editor);
    let owner_property = match call(
        &mut alice,
        Request::PropertyCreate {
            space_id: space,
            name: "Owner".into(),
            kind: PropertyKind::Person,
            config: None,
        },
    )
    .unwrap()
    {
        Response::Property(PropertyDef { id, .. }) => id,
        other => panic!("unexpected {other:?}"),
    };
    let template = create_page(&mut alice.core, space, None, "Retro").id;
    let sub = create_page(&mut alice.core, space, Some(template), "Actions").id;
    set_template(&mut alice, template);
    call(
        &mut alice,
        Request::PageSetTemplatePlaceholder {
            page_id: sub,
            property_id: owner_property,
            placeholder: Some(kb_types::TemplatePlaceholder::Me),
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let bob_account = bob.core.me().unwrap();
    let alice_account = alice.core.me().unwrap();
    assert_ne!(bob_account, alice_account);

    let made = create_from(&mut bob, template, Vec::new()).unwrap();
    let copies = {
        let view = crate::test_support::tree(&mut bob.core, space);
        let node = view
            .roots
            .iter()
            .find(|node| node.page.id == made.id)
            .expect("the new page is a root");
        let mut ids = vec![node.page.id];
        ids.extend(node.children.iter().map(|child| child.page.id));
        ids
    };
    assert_eq!(copies.len(), 2);
    assert!(!made.template);
    Dev::settle(&mut [&mut alice, &mut bob], 4);

    // Both devices converge on the new pages; the person is the one who made them, not the owner.
    for dev in [&mut alice, &mut bob] {
        assert_eq!(
            outline_of(dev, space)
                .lines()
                .filter(|line| line.contains("Retro") || line.contains("Actions"))
                .count(),
            4,
            "{}: the template and its copy",
            dev.name
        );
        let info = page_of(dev, copies[1]);
        assert!(!info.template, "{}", dev.name);
        assert_eq!(
            info.properties
                .iter()
                .find(|(id, _)| *id == owner_property)
                .map(|(_, value)| value.clone()),
            Some(PropertyValue::Person(vec![bob_account])),
            "{}",
            dev.name
        );
        assert!(page_of(dev, template).template, "{}", dev.name);
    }
}
