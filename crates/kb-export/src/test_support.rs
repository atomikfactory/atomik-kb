//! Test doubles shared by the tests of this crate.

use std::collections::HashMap;

use kb_types::{AttachmentId, BlockId, OptionId, PageId, PropertyId};

use crate::ExportLinks;

/// An `ExportLinks` that knows what a test puts in it and nothing else.
#[derive(Default)]
pub(crate) struct TestLinks {
    /// Page: title, and its href when it is in the export.
    pub pages: HashMap<PageId, (String, Option<String>)>,
    /// Attachment: name, and its href when it is in the archive.
    pub attachments: HashMap<AttachmentId, (String, Option<String>)>,
    /// Whiteboard block: href of its `.excalidraw` file.
    pub whiteboards: HashMap<BlockId, String>,
    /// Property names.
    pub properties: HashMap<PropertyId, String>,
    /// Option names.
    pub options: HashMap<(PropertyId, OptionId), String>,
}

impl ExportLinks for TestLinks {
    fn page_href(&self, page: PageId) -> Option<String> {
        self.pages.get(&page)?.1.clone()
    }

    fn page_title(&self, page: PageId) -> Option<String> {
        Some(self.pages.get(&page)?.0.clone())
    }

    fn attachment_href(&self, attachment: AttachmentId) -> Option<String> {
        self.attachments.get(&attachment)?.1.clone()
    }

    fn attachment_name(&self, attachment: AttachmentId) -> Option<String> {
        Some(self.attachments.get(&attachment)?.0.clone())
    }

    fn whiteboard_href(&self, block: BlockId) -> Option<String> {
        self.whiteboards.get(&block).cloned()
    }

    fn property_name(&self, property: PropertyId) -> Option<String> {
        self.properties.get(&property).cloned()
    }

    fn option_name(&self, property: PropertyId, option: OptionId) -> Option<String> {
        self.options.get(&(property, option)).cloned()
    }
}

/// A page id from one byte.
pub(crate) fn page(n: u8) -> PageId {
    PageId::from_random_bytes([n; 16])
}

/// An attachment id from one byte.
pub(crate) fn attachment(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([n; 16])
}

/// A block id from one byte.
pub(crate) fn block(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

impl TestLinks {
    /// The export most tests use. Pages: 1 "Alpha" is in it (`Alpha.md`), 2 "Beta" is not.
    /// Attachments: 1 `diagram.png`, 2 `clip.mp4`, 3 `song.mp3`, 4 `report.pdf` (all in the
    /// archive, under `dir/`), 5 `gone.bin` (named, not in the archive). Block 1 is a whiteboard
    /// (`dir/Whiteboard.excalidraw`).
    pub(crate) fn sample() -> Self {
        let mut links = TestLinks::default();
        links
            .pages
            .insert(page(1), ("Alpha".to_owned(), Some("Alpha.md".to_owned())));
        links.pages.insert(page(2), ("Beta".to_owned(), None));
        for (n, name, href) in [
            (1, "diagram.png", "dir/diagram.png"),
            (2, "clip.mp4", "dir/clip.mp4"),
            (3, "song.mp3", "dir/song.mp3"),
            (4, "report.pdf", "dir/report%20x.pdf"),
        ] {
            links
                .attachments
                .insert(attachment(n), (name.to_owned(), Some(href.to_owned())));
        }
        links
            .attachments
            .insert(attachment(5), ("gone.bin".to_owned(), None));
        links
            .whiteboards
            .insert(block(1), "dir/Whiteboard.excalidraw".to_owned());
        links
    }
}
