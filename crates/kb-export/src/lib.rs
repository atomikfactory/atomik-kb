//! Export of pages to files (ADR-0036, `docs/architecture/import-export.md`, "Export").
//!
//! Pure functions over portable page JSON and page metadata the core gathers: the layout of an
//! export (file and folder names, links between them), a page as Markdown with YAML front matter
//! or as a standalone HTML document, whiteboards as `.excalidraw` files, the `atomik-export.json`
//! sidecar and the CSV of a view. No I/O, no state; compiles for `wasm32-unknown-unknown`.
//! The core (`kb-core`, `export` module) runs the session, the shell writes the archive.

#![forbid(unsafe_code)]

pub mod csv;
pub mod excalidraw;
pub mod html;
pub mod layout;
pub mod markdown;
pub mod sidecar;
pub mod value;

mod json;
mod time;

#[cfg(test)]
mod file_flavour_tests;
#[cfg(test)]
mod person_tests;
#[cfg(test)]
mod test_support;

pub use kb_model::ExportLinks;
pub use layout::{Layout, LayoutAttachment, LayoutPage, layout, relative_href, safe_file_name};
pub use value::{ExportProperty, ExportValue, LinkedFile, LinkedPage, PageMeta};
