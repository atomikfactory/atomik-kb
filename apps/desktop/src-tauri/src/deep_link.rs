//! Deep links (ADR-0040, ADR-0046): what the shell does with the links the operating system hands to the
//! app: `atomik-kb://page/<pageId>[#<blockId>]`, which "Copy link to page" and "Copy link to block" write,
//! and `atomik-kb://invite?server=<url>#<secret>`, the invitation link of a shared space.
//!
//! **Where links come from.** `tauri-plugin-deep-link` registers the scheme with the OS (the NSIS installer
//! writes `HKCU\Software\Classes\atomik-kb`, the `.deb` should get a `x-scheme-handler/atomik-kb`
//! entry in its `.desktop` file (not yet inspected), both from `plugins.deep-link` in `tauri.conf.json`). On Windows and Linux the OS then
//! starts a process with the URL as its only argument; `tauri-plugin-single-instance` (registered first, with
//! its `deep-link` feature) hands that process's arguments to the running app and the process exits, and the
//! deep-link plugin raises its `deep-link://new-url` event, which [`crate::run`] listens to. A link the app was
//! *started* with is in the plugin's `get_current`. Both end in [`offer`]. The webview is granted none of
//! the plugins' permissions: the only way in for it is the shell's events channel (`shell_events`), through the
//! app's own `shell_events_subscribe` command.
//!
//! **What is accepted.** [`parse`] and [`parse_invite`] are strict and pure.
//!
//! - A page link: the scheme (any case), `page/`, a canonical lowercase UUID and optionally `#` and another.
//!   No query, no extra path or trailing slash, no whitespace, no other text, at most [`MAX_LEN`] bytes.
//! - An invitation link: the scheme (any case), `invite?server=`, the server address percent-escaped (only
//!   the characters `encodeURIComponent` leaves, and well-formed `%XX` escapes; it decodes to an
//!   `http://` or `https://` address with a host, without whitespace or control characters, at most 512
//!   bytes), `#`, and the secret: exactly 52 letters and digits (Crockford base32, no `U`, no dashes), at
//!   most [`INVITE_MAX_LEN`] bytes. Nothing else may follow or precede, no second parameter. The core
//!   checks the rest (the address, the canonical form of the secret) when the UI hands the link to
//!   `InviteLinkOpen`.
//!
//! Anything else, including `atomik-kb://pair?...` (device pairing, ADR-0022, is not a deep link), is
//! ignored. Nothing here logs a URL, an id or a secret: "deep link ignored", "deep link opened" and "invite
//! link opened" only; an [`InviteLink`] hides its text from `Debug`.
//!
//! **Delivery.** [`offer`] hands an accepted link to `ShellEvents`, which sends it to the webview's subscribers
//! (the UI opens the page in the active tab, ADR-0034, and scrolls to the block; it opens the invitation's preview).
//! With nobody subscribed (the app is starting, or its page is reloading) the last link of each kind is held and
//! goes to the next subscriber at once. The shell does not look at the app lock: the UI keeps the link until a
//! workspace is open (after an unlock when locked), where it looks the page up through the core like any other
//! link, or hands the invitation to the core; so a locked app opens nothing and reveals nothing until the
//! passphrase has been given.

use std::fmt;
use std::sync::Arc;

use kb_types::PageId;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_deep_link::DeepLinkExt;

use crate::shell_events::ShellEvents;

/// The URL scheme registered with the OS (without `://`); `plugins.deep-link.desktop.schemes` in
/// `tauri.conf.json` says the same.
pub const SCHEME: &str = "atomik-kb";

/// The longest page link accepted, in bytes. The longest valid one has a page and a block id: 90.
pub const MAX_LEN: usize = 128;

/// The longest invitation link accepted, in bytes: a server address of 512 bytes with every character
/// escaped, and the secret.
pub const INVITE_MAX_LEN: usize = 1024;

/// What follows the scheme, case-sensitive.
const HOST_AND_SLASH: &str = "page/";

/// What follows the scheme of an invitation link, case-sensitive.
const INVITE_HOST_AND_QUERY: &str = "invite?server=";

/// Letters and digits of an invite's secret (256 bits in Crockford base32, no padding).
const INVITE_SECRET_LEN: usize = 52;

/// The longest server address an invitation link may name once unescaped.
const INVITE_SERVER_MAX_LEN: usize = 512;

/// A page, and optionally one of its blocks, as a link names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepLink {
    /// The page.
    pub page_id: PageId,
    /// The block to scroll to: canonical lowercase UUID text, as the page document stores it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
}

/// The link `raw` is, when it is exactly `atomik-kb://page/<uuid>` or `atomik-kb://page/<uuid>#<uuid>`
/// (scheme in any case, both ids canonical lowercase UUIDs), and `None` for anything else.
#[must_use]
pub fn parse(raw: &str) -> Option<DeepLink> {
    if raw.len() > MAX_LEN {
        return None;
    }
    // `split_at_checked` is `None` for a text too short or cut inside a character.
    let (scheme, rest) = raw.split_at_checked(SCHEME.len())?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let rest = rest.strip_prefix("://")?.strip_prefix(HOST_AND_SLASH)?;
    let (page, block) = match rest.split_once('#') {
        Some((page, block)) => (page, Some(block)),
        None => (rest, None),
    };
    if !is_canonical_uuid(page) || !block.is_none_or(is_canonical_uuid) {
        return None;
    }
    Some(DeepLink {
        page_id: page.parse().ok()?,
        block_id: block.map(str::to_owned),
    })
}

/// `8-4-4-4-12` lowercase hexadecimal digits, nothing else (not the braced, URN or hyphenless spellings
/// `PageId::from_str` also reads, and not capitals: the app only ever writes lowercase).
fn is_canonical_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
        })
}

/// An invitation link the shell accepted: the text of `atomik-kb://invite?server=<url>#<secret>`, as it
/// came. It carries the invite's secret, so it is never logged: `Debug` hides it, and it serialises as the
/// plain text the UI hands to the core (`InviteLinkOpen`).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InviteLink(String);

impl InviteLink {
    /// The link's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InviteLink(..)")
    }
}

/// The invitation link `raw` is, when it is exactly `atomik-kb://invite?server=<escaped address>#<secret>`
/// (scheme in any case; see the module documentation for what the address and the secret may be), and `None`
/// for anything else.
#[must_use]
pub fn parse_invite(raw: &str) -> Option<InviteLink> {
    if raw.len() > INVITE_MAX_LEN {
        return None;
    }
    // `split_at_checked` is `None` for a text too short or cut inside a character.
    let (scheme, rest) = raw.split_at_checked(SCHEME.len())?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let rest = rest
        .strip_prefix("://")?
        .strip_prefix(INVITE_HOST_AND_QUERY)?;
    let (server, secret) = rest.split_once('#')?;
    if !is_invite_secret(secret) || unescape_server(server).is_none() {
        return None;
    }
    Some(InviteLink(raw.to_owned()))
}

/// 52 letters and digits, the alphabet of an invite's secret (Crockford base32 reads every letter but `U`).
fn is_invite_secret(text: &str) -> bool {
    text.len() == INVITE_SECRET_LEN
        && text.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_alphabetic() && !matches!(byte, b'u' | b'U'))
        })
}

/// The server address the `server` parameter stands for: only unreserved characters (and the few that
/// `encodeURIComponent` leaves) and well-formed `%XX` escapes, UTF-8 once unescaped, an `http://` or
/// `https://` address with a host and without whitespace, control characters or `#`.
fn unescape_server(param: &str) -> Option<String> {
    let bytes = param.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes.get(index)? {
            b'%' => {
                let pair = param.get(index + 1..index + 3)?;
                if !pair.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return None;
                }
                out.push(u8::from_str_radix(pair, 16).ok()?);
                index += 3;
            }
            byte if byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.' | b'_' | b'~' | b'!' | b'*' | b'\'' | b'(' | b')'
                ) =>
            {
                out.push(*byte);
                index += 1;
            }
            _ => return None,
        }
    }
    let server = String::from_utf8(out).ok()?;
    let host = server
        .strip_prefix("https://")
        .or_else(|| server.strip_prefix("http://"))?;
    let plain = server
        .chars()
        .all(|c| !c.is_control() && !c.is_whitespace() && c != '#');
    (plain
        && server.len() <= INVITE_SERVER_MAX_LEN
        && !host.is_empty()
        && !host.starts_with(['/', '?', ':', '@']))
    .then_some(server)
}

/// Takes a URL the OS gave the app: an accepted one (see [`parse`] and [`parse_invite`]) is sent to the
/// webview's subscribers, or held when there is none that can be reached (the last link of each kind wins);
/// anything else is ignored. Answers whether it was accepted. Never logs the URL.
pub fn offer(events: &ShellEvents, raw: &str) -> bool {
    if let Some(link) = parse(raw) {
        tracing::info!("deep link opened");
        events.deep_link(link);
        return true;
    }
    if let Some(link) = parse_invite(raw) {
        tracing::info!("invite link opened");
        events.invite_link(link);
        return true;
    }
    tracing::info!("deep link ignored");
    false
}

/// Environment variable that keeps "one running app" on in a debug build that has `KB_DATA_DIR` (`1`): the
/// real-app pass of the deep links (`scripts/smoke/drive-s-gaps.ps1`) starts a second launch beside a running
/// app on its own data directory and expects it to be forwarded to the first. Release builds ignore it (they
/// ignore `KB_DATA_DIR` too, so they always take part).
pub const SINGLE_INSTANCE_VAR: &str = "KB_SINGLE_INSTANCE";

/// Whether `value` (the content of [`SINGLE_INSTANCE_VAR`]) asks to keep "one running app" on beside
/// `KB_DATA_DIR`: debug builds only, and only the exact value `1`.
#[must_use]
pub fn keep_single_instance(value: Option<std::ffi::OsString>, debug_build: bool) -> bool {
    debug_build && value.is_some_and(|value| value == "1")
}

/// Whether this launch takes part in "one running app". A debug build with `KB_DATA_DIR` is a second
/// device on its own data directory (the sync demo, `scripts/smoke/launch-b.ps1`): it must start beside
/// the first, so it does not register the single-instance plugin, unless `keep_single_instance` (the
/// [`SINGLE_INSTANCE_VAR`] override) asks for it. Everything else does.
#[must_use]
pub fn single_instance_applies(
    data_dir_override: Option<&std::path::Path>,
    keep_single_instance: bool,
) -> bool {
    data_dir_override.is_none() || keep_single_instance
}

/// What the running app does when the single-instance plugin forwards a second launch (the deep-link plugin has
/// already raised the event for a link among its arguments): the main window comes to the front.
pub fn on_second_launch<R: Runtime>(app: &AppHandle<R>) {
    tracing::info!("a second launch was forwarded to the running app");
    crate::window::show_main_window(app);
}

/// Feeds the links of the OS into the managed [`ShellEvents`]: the ones that arrive while the app runs (the
/// deep-link plugin's `deep-link://new-url` event, raised for a second launch's arguments and, on macOS, by the
/// OS) and the one the app was started with (`get_current`). Call it once the plugin is set up (in `setup`).
/// Nothing is logged but "deep link opened" or "deep link ignored".
pub fn listen<R: Runtime>(app: &tauri::App<R>) {
    let events = Arc::clone(app.state::<Arc<ShellEvents>>().inner());
    let running = Arc::clone(&events);
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            offer(&running, url.as_str());
        }
    });
    match app.deep_link().get_current() {
        Ok(Some(urls)) => {
            for url in urls {
                offer(&events, url.as_str());
            }
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "the link the app was started with could not be read"),
    }
}

/// Environment variable that makes a debug build register the scheme with the OS (`1`).
#[cfg(debug_assertions)]
pub const REGISTER_VAR: &str = "KB_REGISTER_DEEP_LINK";

/// Debug builds only, and only with `KB_REGISTER_DEEP_LINK=1`: registers the `atomik-kb` scheme for this
/// executable (`DeepLink::register_all`: on Windows `HKCU\Software\Classes\atomik-kb`, on Linux a
/// `<exe>-handler.desktop` in the user's applications folder). It is opt-in because it takes the scheme from an
/// installed app until that is installed again; the installers register it for release builds, and a debug app
/// can be handed a link without any registration by starting it with the URL as its argument.
#[cfg(debug_assertions)]
pub fn register_for_development<R: Runtime>(app: &tauri::App<R>) {
    if !std::env::var_os(REGISTER_VAR).is_some_and(|value| value == "1") {
        return;
    }
    match app.deep_link().register_all() {
        Ok(()) => tracing::info!("the atomik-kb scheme is registered for this debug build"),
        Err(error) => tracing::warn!(%error, "the atomik-kb scheme could not be registered"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::shell_events::ShellEvent;
    use crate::test_support::captured_logs;

    const PAGE: &str = "12345678-9abc-4def-8123-456789abcdef";
    const BLOCK: &str = "0a1b2c3d-4e5f-4a6b-8c7d-8e9f0a1b2c3d";

    fn link(page: &str, block: Option<&str>) -> DeepLink {
        DeepLink {
            page_id: page.parse().unwrap(),
            block_id: block.map(str::to_owned),
        }
    }

    // ----- the parser ----------------------------------------------------------------------------------------

    #[test]
    fn a_page_link_is_accepted() {
        assert_eq!(
            parse(&format!("atomik-kb://page/{PAGE}")),
            Some(link(PAGE, None))
        );
    }

    #[test]
    fn a_page_link_with_a_block_is_accepted() {
        assert_eq!(
            parse(&format!("atomik-kb://page/{PAGE}#{BLOCK}")),
            Some(link(PAGE, Some(BLOCK)))
        );
    }

    #[test]
    fn the_scheme_is_matched_in_any_case_and_nothing_else_is() {
        for scheme in ["ATOMIK-KB", "Atomik-Kb", "atomik-KB"] {
            assert_eq!(
                parse(&format!("{scheme}://page/{PAGE}#{BLOCK}")),
                Some(link(PAGE, Some(BLOCK))),
                "{scheme}"
            );
        }
        // The host and the ids are exact.
        assert_eq!(parse(&format!("atomik-kb://PAGE/{PAGE}")), None);
        assert_eq!(parse(&format!("atomik-kb://Page/{PAGE}")), None);
    }

    #[test]
    fn the_ids_must_be_canonical_lowercase_uuids() {
        let upper = PAGE.to_uppercase();
        let mixed = "12345678-9ABC-4def-8123-456789abcdef";
        let hyphenless = PAGE.replace('-', "");
        let braced = format!("{{{PAGE}}}");
        let urn = format!("urn:uuid:{PAGE}");
        let short = &PAGE[..35];
        let long = format!("{PAGE}0");
        let wrong_hyphens = "1234567-89abc-4def-8123-456789abcdef";
        let not_hex = "1234567g-9abc-4def-8123-456789abcdef";
        let space = format!(" {}", &PAGE[1..]);
        let non_ascii = "1234567é-9abc-4def-8123-456789abcdef";
        for bad in [
            upper.as_str(),
            mixed,
            hyphenless.as_str(),
            braced.as_str(),
            urn.as_str(),
            short,
            long.as_str(),
            wrong_hyphens,
            not_hex,
            space.as_str(),
            non_ascii,
            "",
            "page",
        ] {
            assert_eq!(
                parse(&format!("atomik-kb://page/{bad}")),
                None,
                "page {bad}"
            );
            assert_eq!(
                parse(&format!("atomik-kb://page/{PAGE}#{bad}")),
                None,
                "block {bad}"
            );
        }
    }

    #[test]
    fn a_link_with_anything_more_or_less_is_ignored() {
        let long_tail = "a".repeat(40);
        for bad in [
            // A query, an extra path, a trailing slash, a second fragment, an empty block.
            format!("atomik-kb://page/{PAGE}?x=1"),
            format!("atomik-kb://page/{PAGE}?x=1#{BLOCK}"),
            format!("atomik-kb://page/{PAGE}#{BLOCK}?x=1"),
            format!("atomik-kb://page/{PAGE}/"),
            format!("atomik-kb://page/{PAGE}/extra"),
            format!("atomik-kb://page/{PAGE}/{BLOCK}"),
            format!("atomik-kb://page/{PAGE}#{BLOCK}#{BLOCK}"),
            format!("atomik-kb://page/{PAGE}#"),
            format!("atomik-kb://page/{PAGE}#{BLOCK}/"),
            // Whitespace and control characters around or inside.
            format!(" atomik-kb://page/{PAGE}"),
            format!("atomik-kb://page/{PAGE} "),
            format!("atomik-kb://page/{PAGE}\n"),
            format!("atomik-kb://page/{PAGE}\t#{BLOCK}"),
            format!("atomik-kb://page/{PAGE}\0"),
            // Other hosts, schemes and shapes.
            format!("atomik-kb://{PAGE}"),
            format!("atomik-kb:/page/{PAGE}"),
            format!("atomik-kb:page/{PAGE}"),
            format!("atomik-kb:///page/{PAGE}"),
            format!("atomik-kb://page//{PAGE}"),
            format!("atomik-kb://page/../{PAGE}"),
            format!("atomik-kb://user@page/{PAGE}"),
            format!("atomik-kb://page:80/{PAGE}"),
            format!("atomik-kb://other/{PAGE}"),
            format!("atomik-kb://page/%2e%2e/{PAGE}"),
            format!("atomik-kb://page/{}", PAGE.replace('-', "%2d")),
            format!("http://page/{PAGE}"),
            format!("atomik-kb+x://page/{PAGE}"),
            format!("xatomik-kb://page/{PAGE}"),
            format!("see atomik-kb://page/{PAGE}"),
            format!("atomik-kb://page/{PAGE} atomik-kb://page/{PAGE}"),
            format!("atomik-kb://page/{PAGE}#{long_tail}"),
            // The scheme alone, and nothing.
            "atomik-kb://".to_owned(),
            "atomik-kb://page/".to_owned(),
            "atomik-kb".to_owned(),
            String::new(),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_pairing_link_is_not_a_deep_link_of_this_item() {
        for pair in [
            "atomik-kb://pair",
            "atomik-kb://pair?server=https%3A%2F%2Fexample.com&code=ABCD",
            "atomik-kb://pair/?server=x&code=y",
        ] {
            assert_eq!(parse(pair), None, "{pair}");
        }
    }

    #[test]
    fn a_very_long_link_is_ignored_without_being_parsed() {
        // A link of the longest valid length is read; padding it past the cap, far past it, and to a
        // megabyte is not.
        let at_cap = format!("atomik-kb://page/{PAGE}#{BLOCK}");
        assert!(at_cap.len() <= MAX_LEN);
        assert!(parse(&at_cap).is_some());
        assert_eq!(parse(&format!("{at_cap}{}", "a".repeat(MAX_LEN))), None);
        assert_eq!(parse(&"a".repeat(MAX_LEN + 1)), None);
        assert_eq!(parse(&"atomik-kb://page/".repeat(70_000)), None);
        assert_eq!(
            parse(&format!("atomik-kb://page/{PAGE}{}", "0".repeat(1 << 20))),
            None
        );
    }

    #[test]
    fn text_cut_inside_a_character_never_panics() {
        // The scheme's length lands inside a multi-byte character.
        for text in [
            "atomik-k\u{e9}://page/x",
            "\u{1f600}\u{1f600}\u{1f600}",
            "\u{e9}",
            "atomik-k",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }

    #[test]
    fn the_longest_valid_link_fits_the_cap() {
        let longest = format!("atomik-kb://page/{PAGE}#{BLOCK}");
        assert_eq!(longest.len(), 90);
        assert!(longest.len() <= MAX_LEN);
    }

    #[test]
    fn the_link_serialises_as_the_webview_reads_it() {
        let both = serde_json::to_value(link(PAGE, Some(BLOCK))).unwrap();
        assert_eq!(
            both,
            serde_json::json!({ "pageId": PAGE, "blockId": BLOCK })
        );
        let page_only = serde_json::to_value(link(PAGE, None)).unwrap();
        assert_eq!(page_only, serde_json::json!({ "pageId": PAGE }));
    }

    // ----- offering a link ---------------------------------------------------------------------------------------

    const OTHER: &str = "87654321-cba9-4fed-8321-fedcba987654";

    fn url(page: &str) -> String {
        format!("atomik-kb://page/{page}")
    }

    /// A subscriber that records what it was sent.
    fn recording(events: &ShellEvents) -> Arc<Mutex<Vec<ShellEvent>>> {
        let seen: Arc<Mutex<Vec<ShellEvent>>> = Arc::default();
        let sink = Arc::clone(&seen);
        events.subscribe(Box::new(move |event| {
            sink.lock().unwrap().push(event.clone());
            true
        }));
        seen
    }

    #[test]
    fn an_accepted_link_reaches_the_subscriber_as_a_deep_link_message() {
        let events = ShellEvents::new();
        let seen = recording(&events);
        assert!(offer(&events, &url(PAGE)));
        assert!(offer(&events, &format!("{}#{BLOCK}", url(OTHER))));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                ShellEvent::DeepLink(link(PAGE, None)),
                ShellEvent::DeepLink(link(OTHER, Some(BLOCK)))
            ]
        );
        assert!(!events.has_pending());
    }

    #[test]
    fn a_link_before_anyone_listens_is_held_for_the_first_subscriber() {
        // The cold start: the OS hands the link over while the page is still loading.
        let events = ShellEvents::new();
        assert!(offer(&events, &url(PAGE)));
        assert!(events.has_pending());
        let seen = recording(&events);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![ShellEvent::DeepLink(link(PAGE, None))]
        );
        assert!(!events.has_pending());
    }

    #[test]
    fn a_link_that_is_ignored_neither_replaces_nor_clears_the_held_one() {
        let events = ShellEvents::new();
        assert!(offer(&events, &url(PAGE)));
        assert!(!offer(&events, "atomik-kb://pair?server=x&code=y"));
        assert!(!offer(&events, &format!("{}?x=1", url(OTHER))));
        let seen = recording(&events);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![ShellEvent::DeepLink(link(PAGE, None))]
        );
    }

    #[test]
    fn the_log_says_what_happened_and_never_the_link() {
        let links = ShellEvents::new();
        let logs = captured_logs(|| {
            offer(&links, &format!("atomik-kb://page/{PAGE}#{BLOCK}"));
            offer(&links, &format!("atomik-kb://page/{OTHER}?secret=1"));
            offer(
                &links,
                "atomik-kb://pair?server=https%3A%2F%2Fprivate.example&code=SECRETCODE",
            );
        });
        assert!(logs.contains("deep link opened"), "{logs}");
        assert!(logs.contains("deep link ignored"), "{logs}");
        for secret in [
            PAGE,
            BLOCK,
            OTHER,
            "secret",
            "private.example",
            "SECRETCODE",
            "atomik-kb://",
        ] {
            assert!(!logs.contains(secret), "{secret} in {logs}");
        }
    }

    #[test]
    fn a_second_device_on_its_own_data_directory_starts_beside_the_first() {
        let data = std::path::Path::new("C:\\scratch\\b-data");
        assert!(single_instance_applies(None, false));
        assert!(!single_instance_applies(Some(data), false));
    }

    #[test]
    fn the_override_keeps_one_running_app_beside_a_data_directory() {
        let data = std::path::Path::new("C:\\scratch\\b-data");
        // With the override a launch on its own data directory takes part, so a second launch is forwarded.
        assert!(single_instance_applies(Some(data), true));
        // Without a data directory everything takes part, with or without the override.
        assert!(single_instance_applies(None, true));
    }

    #[test]
    fn the_override_is_the_value_one_in_debug_builds_only() {
        let value = |text: &str| Some(std::ffi::OsString::from(text));
        assert!(keep_single_instance(value("1"), true));
        // Release builds ignore it, as they ignore `KB_DATA_DIR`.
        assert!(!keep_single_instance(value("1"), false));
        // Nothing set, and anything but `1`, keeps the second device beside the first.
        assert!(!keep_single_instance(None, true));
        for other in ["", "0", "true", "yes", "11", " 1", "1 "] {
            assert!(!keep_single_instance(value(other), true), "{other:?}");
        }
    }

    // ----- invitation links ----------------------------------------------------------------------------------

    /// A well-formed secret: 52 letters and digits.
    const SECRET: &str = "8H2XG7MQ4TNP5WRZ3KVB9D6YCEJA1F0S8H2XG7MQ4TNP5WRZ3KAB";
    /// `http://127.0.0.1:8787` the way the core escapes it.
    const SERVER: &str = "http%3A%2F%2F127.0.0.1%3A8787";

    fn invite(server: &str, secret: &str) -> String {
        format!("atomik-kb://invite?server={server}#{secret}")
    }

    fn accepted(raw: &str) -> bool {
        parse_invite(raw).is_some()
    }

    #[test]
    fn an_invitation_link_is_accepted_as_written() {
        assert_eq!(SECRET.len(), INVITE_SECRET_LEN);
        let raw = invite(SERVER, SECRET);
        let link = parse_invite(&raw).unwrap();
        // The text is kept as it came: the core reads it again.
        assert_eq!(link.as_str(), raw);
        // An https address with a port and a path prefix, escaped like `encodeURIComponent` does
        // (it leaves `!*'()`), and like the core does.
        for server in [
            "https%3A%2F%2Fsync.example.com",
            "https%3A%2F%2Fsync.example.com%3A8443%2Fkb",
            "http%3A%2F%2F%5B%3A%3A1%5D%3A9000",
            "https%3A%2F%2Fa-b_c.example.org%2F%7Euser",
            "https%3A%2F%2Fexample.com%2F(a)!*'",
        ] {
            assert!(accepted(&invite(server, SECRET)), "{server}");
        }
    }

    #[test]
    fn the_scheme_of_an_invitation_is_matched_in_any_case_and_nothing_else_is() {
        for scheme in ["ATOMIK-KB", "Atomik-Kb", "atomik-KB"] {
            assert!(accepted(&format!(
                "{scheme}://invite?server={SERVER}#{SECRET}"
            )));
        }
        for bad in [
            format!("atomik-kb://INVITE?server={SERVER}#{SECRET}"),
            format!("atomik-kb://Invite?server={SERVER}#{SECRET}"),
            format!("atomik-kb://invite?SERVER={SERVER}#{SECRET}"),
            format!("atomik-kb://invite?Server={SERVER}#{SECRET}"),
            format!("atomik-kb://invite/?server={SERVER}#{SECRET}"),
            format!("atomik-kb://invite/server={SERVER}#{SECRET}"),
            format!("atomik-kb:/invite?server={SERVER}#{SECRET}"),
            format!("atomik-kb:invite?server={SERVER}#{SECRET}"),
            format!("atomik-kb:///invite?server={SERVER}#{SECRET}"),
            format!("atomik-kb://user@invite?server={SERVER}#{SECRET}"),
            format!("atomik-kb://invite:80?server={SERVER}#{SECRET}"),
            format!("atomik-kb+x://invite?server={SERVER}#{SECRET}"),
            format!("xatomik-kb://invite?server={SERVER}#{SECRET}"),
            format!("http://invite?server={SERVER}#{SECRET}"),
            format!("see atomik-kb://invite?server={SERVER}#{SECRET}"),
        ] {
            assert!(!accepted(&bad), "{bad}");
        }
    }

    #[test]
    fn the_secret_is_exactly_52_letters_and_digits_without_u() {
        // Lower case is the same alphabet (the core reads it case-insensitively).
        assert!(accepted(&invite(SERVER, &SECRET.to_ascii_lowercase())));
        let short = &SECRET[..51];
        let long = format!("{SECRET}0");
        let with_u = SECRET.replace('X', "U");
        let with_dash = format!("{}-{}", &SECRET[..26], &SECRET[26..]);
        let with_space = format!("{} {}", &SECRET[..26], &SECRET[26..]);
        let with_punctuation = SECRET.replace('X', "!");
        let non_ascii = SECRET.replace('X', "\u{e9}");
        let two = format!("{SECRET}#{SECRET}");
        for bad in [
            "",
            short,
            long.as_str(),
            with_u.as_str(),
            with_dash.as_str(),
            with_space.as_str(),
            with_punctuation.as_str(),
            non_ascii.as_str(),
            two.as_str(),
            "secret",
        ] {
            assert!(!accepted(&invite(SERVER, bad)), "{bad:?}");
        }
        // No fragment, an empty one.
        assert!(!accepted(&format!("atomik-kb://invite?server={SERVER}")));
        assert!(!accepted(&format!("atomik-kb://invite?server={SERVER}#")));
        assert!(!accepted(&format!("atomik-kb://invite#{SECRET}")));
        assert!(!accepted(&format!("atomik-kb://invite?#{SECRET}")));
    }

    #[test]
    fn the_server_is_one_escaped_http_address_and_nothing_more() {
        for bad in [
            // Nothing, another parameter, a second one, a different name.
            "",
            "x%3D1",
            &format!("{SERVER}&x=1"),
            &format!("{SERVER}&server={SERVER}"),
            &format!("server={SERVER}"),
            // Not escaped: the address is not made of the characters the escaping leaves.
            "http://127.0.0.1:8787",
            "127.0.0.1:8787",
            "http%3A//127.0.0.1",
            // A bad escape.
            "http%3A%2F%2F127.0.0.1%",
            "http%3A%2F%2F127.0.0.1%2",
            "http%3A%2F%2F127.0.0.1%zz",
            "http%3A%2F%2F127.0.0.1%g0",
            "http%3A%2F%2F127.0.0.1%\u{e9}",
            // Not UTF-8 once unescaped.
            "http%3A%2F%2Fa%FF",
            // Another scheme, no host, a host that is not one.
            "ftp%3A%2F%2Fexample.com",
            "file%3A%2F%2F%2Fetc%2Fpasswd",
            "javascript%3Aalert(1)",
            "HTTP%3A%2F%2Fexample.com",
            "http%3A%2F%2F",
            "https%3A%2F%2F",
            "http%3A%2F%2F%2Fexample.com",
            "http%3A%2F%2F%3A80",
            "http%3A%2F%2F%40example.com",
            "http%3A%2F%2F%3Fexample.com",
            // Whitespace, control characters and `#` (a second fragment).
            "http%3A%2F%2Fexample.com%20",
            "http%3A%2F%2Fexample.com%0A",
            "http%3A%2F%2Fexample.com%09",
            "http%3A%2F%2Fexa%00mple.com",
            "http%3A%2F%2Fexample.com%23x",
            "http%3A%2F%2Fexample.com%C2%A0",
        ] {
            assert!(!accepted(&invite(bad, SECRET)), "{bad:?}");
        }
        // A space in the link itself, outside any escape.
        assert!(!accepted(&format!(
            "atomik-kb://invite?server={SERVER} #{SECRET}"
        )));
        assert!(!accepted(&format!(
            " atomik-kb://invite?server={SERVER}#{SECRET}"
        )));
        assert!(!accepted(&format!(
            "atomik-kb://invite?server={SERVER}#{SECRET} "
        )));
        assert!(!accepted(&format!(
            "atomik-kb://invite?server={SERVER}#{SECRET}\n"
        )));
    }

    #[test]
    fn a_very_long_invitation_is_ignored_without_being_parsed() {
        // A server address of 512 bytes is the longest, every character escaped it is 3 times that
        // and past the cap of the link.
        let long_host = "a".repeat(INVITE_SERVER_MAX_LEN - "http://".len());
        let at_cap = format!("http%3A%2F%2F{long_host}");
        assert!(accepted(&invite(&at_cap, SECRET)));
        let past_cap = format!("http%3A%2F%2F{long_host}a");
        assert!(!accepted(&invite(&past_cap, SECRET)), "513 bytes unescaped");
        let escaped_everything = format!("%68%74%74%70%3A%2F%2F{}", "%61".repeat(340));
        assert!(
            invite(&escaped_everything, SECRET).len() > INVITE_MAX_LEN,
            "the cap of the whole link"
        );
        assert!(!accepted(&invite(&escaped_everything, SECRET)));
        assert!(!accepted(&"atomik-kb://invite?server=".repeat(70_000)));
        assert!(!accepted(&format!(
            "atomik-kb://invite?server={SERVER}#{SECRET}{}",
            "0".repeat(1 << 20)
        )));
    }

    #[test]
    fn invitation_text_cut_inside_a_character_never_panics() {
        for text in [
            "atomik-k\u{e9}://invite?server=x",
            "\u{1f600}\u{1f600}\u{1f600}",
            "atomik-kb://invite?server=\u{e9}#\u{e9}",
            "atomik-kb://invite?server=%\u{1f600}#00",
            "atomik-kb://invite?server=http%3A%2F%2F\u{e9}#\u{e9}",
        ] {
            assert!(!accepted(text), "{text}");
        }
    }

    #[test]
    fn a_page_link_is_not_an_invitation_and_an_invitation_is_not_a_page_link() {
        assert!(!accepted(&format!("atomik-kb://page/{PAGE}")));
        assert!(parse(&invite(SERVER, SECRET)).is_none());
        assert!(!accepted(
            "atomik-kb://pair?server=https%3A%2F%2Fexample.com&code=ABCD"
        ));
    }

    #[test]
    fn an_invitation_reaches_the_subscriber_as_an_invite_link_message() {
        let events = ShellEvents::new();
        let seen = recording(&events);
        let raw = invite(SERVER, SECRET);
        assert!(offer(&events, &raw));
        assert!(offer(&events, &url(PAGE)));
        let messages = seen.lock().unwrap().clone();
        assert_eq!(messages.len(), 2);
        match &messages[0] {
            ShellEvent::InviteLink { link } => assert_eq!(link.as_str(), raw),
            other => panic!("{other:?}"),
        }
        assert_eq!(messages[1], ShellEvent::DeepLink(link(PAGE, None)));
        assert!(!events.has_pending());
    }

    #[test]
    fn an_invitation_before_anyone_listens_is_held_and_the_last_one_wins() {
        // The cold start: the OS hands over the link while the page is still loading.
        let events = ShellEvents::new();
        let first = invite(SERVER, SECRET);
        let second = invite("https%3A%2F%2Fsync.example.com", SECRET);
        assert!(offer(&events, &first));
        assert!(offer(&events, &second));
        assert!(events.has_pending());
        let seen = recording(&events);
        let messages = seen.lock().unwrap().clone();
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            ShellEvent::InviteLink { link } => assert_eq!(link.as_str(), second),
            other => panic!("{other:?}"),
        }
        assert!(!events.has_pending());
    }

    #[test]
    fn a_page_link_and_an_invitation_are_held_side_by_side() {
        let events = ShellEvents::new();
        assert!(offer(&events, &url(PAGE)));
        assert!(offer(&events, &invite(SERVER, SECRET)));
        assert!(offer(&events, &url(OTHER)));
        let seen = recording(&events);
        let messages = seen.lock().unwrap().clone();
        // The newer page link replaced the older one and stands where it came; the invitation stays.
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(matches!(messages[0], ShellEvent::InviteLink { .. }));
        assert_eq!(messages[1], ShellEvent::DeepLink(link(OTHER, None)));
    }

    #[test]
    fn a_malformed_invitation_is_ignored_and_does_not_clear_the_held_one() {
        let events = ShellEvents::new();
        let held = invite(SERVER, SECRET);
        assert!(offer(&events, &held));
        assert!(!offer(&events, &invite(SERVER, "short")));
        assert!(!offer(&events, &invite("http://127.0.0.1", SECRET)));
        let seen = recording(&events);
        let messages = seen.lock().unwrap().clone();
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            ShellEvent::InviteLink { link } => assert_eq!(link.as_str(), held),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_log_says_what_happened_and_never_an_invitation() {
        let events = ShellEvents::new();
        let server = "https%3A%2F%2Fprivate.example";
        let logs = captured_logs(|| {
            offer(&events, &invite(server, SECRET));
            offer(&events, &invite(server, "short"));
        });
        assert!(logs.contains("invite link opened"), "{logs}");
        assert!(logs.contains("deep link ignored"), "{logs}");
        for secret in [
            SECRET,
            "private.example",
            "private",
            "atomik-kb://invite",
            "server=",
        ] {
            assert!(!logs.contains(secret), "{secret} in {logs}");
        }
    }

    #[test]
    fn an_invitation_hides_its_text_from_debug() {
        let link = parse_invite(&invite(SERVER, SECRET)).unwrap();
        let shown = format!(
            "{link:?} {:?}",
            ShellEvent::InviteLink { link: link.clone() }
        );
        assert!(!shown.contains(SECRET), "{shown}");
        assert!(!shown.contains("127.0.0.1"), "{shown}");
        assert_eq!(
            serde_json::to_value(&link).unwrap(),
            serde_json::Value::String(link.as_str().to_owned())
        );
    }

    #[test]
    fn the_scheme_is_the_one_the_config_registers() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["plugins"]["deep-link"]["desktop"]["schemes"],
            serde_json::json!([SCHEME])
        );
    }
}
