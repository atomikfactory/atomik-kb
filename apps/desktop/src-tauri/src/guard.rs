//! What the webview may leave the app for: the navigation guard of the main window and the
//! scheme filter of `open_external`.
//!
//! A link inside a page must never navigate the webview away from the app (the new page
//! would run inside the app's window, next to the IPC bridge), and a link the user clicks
//! must only ever hand the system a web or mail address, never something that launches a
//! program. Both rules live here as plain functions so a table of URLs can test them.

use kb_types::ai::AI_ENDPOINT_SETTING;
use kb_types::interop::{BACKUP_DRIVE_SETTING, BACKUP_FOLDER_SETTING, BACKUP_LAST_SETTING};
use kb_types::{ApiError, Request};
use tauri::webview::{NewWindowFeatures, NewWindowResponse};
use tauri::{Runtime, Url};

/// Host of the Vite dev server (`devUrl` in `tauri.conf.json`), debug builds only.
const DEV_HOST: &str = "localhost";

/// Port of the Vite dev server; `strictPort` in `vite.config.ts` keeps it fixed.
const DEV_PORT: u16 = 1420;

/// Schemes `open_external` hands to the operating system: web links and mail addresses.
/// Nothing that can launch a program (`file:`, `ms-msdt:`, `search-ms:`, ...) and nothing
/// that runs script (`javascript:`).
const EXTERNAL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Whether the main webview may navigate to `url`.
///
/// Allowed: the app's own origin (`tauri://localhost`, or `http(s)://tauri.localhost` where
/// the platform serves custom protocols that way, as WebView2 does), `about:blank`, and in
/// `dev` (debug builds) the Vite dev server `http://localhost:1420` with its
/// `ws://localhost:1420` socket. Everything else is refused, including any URL that
/// carries credentials, so `http://localhost:1420@evil.example/` cannot pass for a local
/// address.
pub fn navigation_allowed(url: &Url, dev: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    is_app_origin(url) || url.as_str() == "about:blank" || (dev && is_dev_server(url))
}

/// The app's own assets, on whichever origin the platform serves them from.
fn is_app_origin(url: &Url) -> bool {
    let host = url.host_str();
    match url.scheme() {
        "tauri" => host == Some("localhost") && url.port().is_none(),
        "http" | "https" => host == Some("tauri.localhost") && url.port().is_none(),
        _ => false,
    }
}

/// The Vite dev server and its hot-reload socket.
fn is_dev_server(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "ws")
        && url.host_str() == Some(DEV_HOST)
        && url.port() == Some(DEV_PORT)
}

/// `on_navigation` handler of the main window: [`navigation_allowed`] for this build, and a
/// warning for whatever it refuses.
pub fn allow_navigation(url: &Url) -> bool {
    let allowed = navigation_allowed(url, cfg!(debug_assertions));
    if !allowed {
        warn_blocked("navigation", url);
    }
    allowed
}

/// `on_new_window` handler of the main window: the app never opens another window, so
/// every request (`window.open`, `target="_blank"`) is refused. Links the user clicks go
/// through `open_external` instead.
pub fn deny_new_window<R: Runtime>(url: Url, _features: NewWindowFeatures) -> NewWindowResponse<R> {
    warn_blocked("new window", &url);
    NewWindowResponse::Deny
}

/// Logs the scheme and host of a refused request, nothing else: a path or query can hold
/// page content.
fn warn_blocked(request: &str, url: &Url) {
    tracing::warn!(
        request,
        scheme = url.scheme(),
        host = url.host_str().unwrap_or(""),
        "blocked a webview request to leave the app"
    );
}

/// Parses a link the UI wants opened outside the app. Only `http`, `https` and `mailto`
/// pass; the returned URL is the normalised form, which is what must be handed to the
/// system (never the raw text). Anything else is `InvalidInput` for the field `url`; the
/// message never repeats the link, and the warning logged for a refusal names the scheme
/// at most.
pub fn external_url(raw: &str) -> Result<Url, ApiError> {
    let Ok(url) = Url::parse(raw) else {
        tracing::warn!("refused to open a link that is not a valid URL");
        return Err(ApiError::invalid("url", "the link is not a valid URL"));
    };
    if !EXTERNAL_SCHEMES.contains(&url.scheme()) {
        tracing::warn!(
            scheme = url.scheme(),
            "refused to open a link with this scheme"
        );
        return Err(ApiError::invalid(
            "url",
            "only http, https and mailto links can be opened",
        ));
    }
    if url.scheme() == "mailto" && mailto_attaches_a_file(&url) {
        // Some mail clients honour `attach=` and read a local file into the draft.
        tracing::warn!("refused to open a mail link that attaches a file");
        return Err(ApiError::invalid(
            "url",
            "mail links that attach files cannot be opened",
        ));
    }
    Ok(url)
}

/// `mailto:` query parameters that make a mail client attach a local file.
fn mailto_attaches_a_file(url: &Url) -> bool {
    url.query_pairs()
        .any(|(key, _)| matches!(key.to_ascii_lowercase().as_str(), "attach" | "attachment"))
}

/// Settings only the shell writes: the folder of scheduled backups, which the shell takes from its own
/// folder dialog, and the outcome of the last scheduled backup (ADR-0036); and the Ollama endpoint with
/// the person's consent to send text to it, which the shell stores after its native dialog (ADR-0037);
/// and the Google Drive target, which the shell writes after its consent dialog and sign-in (ADR-0059).
/// The webview can read them (`SettingsGet`) but never write them.
const SHELL_OWNED_SETTINGS: [&str; 4] = [
    BACKUP_FOLDER_SETTING,
    BACKUP_LAST_SETTING,
    AI_ENDPOINT_SETTING,
    BACKUP_DRIVE_SETTING,
];

/// Requests only the shell makes (ADR-0037): the indexer's work and its vectors (the webview has no
/// network and no business writing vectors), a message of the MCP server (which carries a client's
/// token) and the grants of an MCP client (which the webview changes through `mcp_client_grants_set`,
/// after a native confirmation).
fn is_shell_only(request: &Request) -> bool {
    matches!(
        request,
        Request::AiIndexNext { .. }
            | Request::AiIndexStore { .. }
            | Request::McpCall { .. }
            | Request::McpClientGrantsSet { .. }
    )
}

/// Checks a request that came from the webview before it reaches the core: a `SettingsSet` of a
/// key the shell owns (see [`SHELL_OWNED_SETTINGS`]) is `InvalidInput` for the field `key`, and a
/// request only the shell makes (see [`is_shell_only`]) is `InvalidInput` for the field `type`. The
/// shell's own writes go through its handle on the core and never through here. A compromised
/// page could otherwise point scheduled backups, and the deletion of the old ones, at any folder
/// the person can write to, send the text of the person's pages to another computer, or give itself
/// access to the workspace through an MCP client without the person's confirmation.
///
/// # Errors
///
/// `InvalidInput` for the field `key` or `type`.
pub fn webview_request_allowed(request: &Request) -> Result<(), ApiError> {
    if let Request::SettingsSet { key, .. } = request
        && SHELL_OWNED_SETTINGS.contains(&key.as_str())
    {
        tracing::warn!("refused a webview write to a setting the shell owns");
        return Err(ApiError::invalid(
            "key",
            "this setting is managed by the app and cannot be changed here",
        ));
    }
    if is_shell_only(request) {
        tracing::warn!(
            kind = request.kind(),
            "refused a webview request only the shell makes"
        );
        return Err(ApiError::invalid(
            "type",
            "this request is made by the app itself and cannot be sent from here",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use kb_types::ErrorCode;

    use super::*;
    use crate::test_support::captured_logs;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    fn allowed(raw: &str, dev: bool) -> bool {
        navigation_allowed(&url(raw), dev)
    }

    /// What the app is served from on every platform, plus the blank page.
    const OWN_ORIGINS: [&str; 12] = [
        "tauri://localhost",
        "tauri://localhost/",
        "tauri://localhost/index.html",
        "tauri://localhost/pages/42?x=1#y",
        "http://tauri.localhost",
        "http://tauri.localhost/",
        "http://tauri.localhost/index.html?x=1#y",
        "https://tauri.localhost/",
        "https://tauri.localhost/index.html",
        "HTTP://TAURI.LOCALHOST/",
        "about:blank",
        "http://tauri.localhost:80/",
    ];

    #[test]
    fn the_apps_own_origin_is_allowed_in_every_build() {
        for raw in OWN_ORIGINS {
            assert!(allowed(raw, false), "release: {raw}");
            assert!(allowed(raw, true), "debug: {raw}");
        }
    }

    #[test]
    fn everything_else_is_denied_in_every_build() {
        for raw in [
            // other sites
            "https://example.com/",
            "http://example.com/",
            "https://example.com/tauri.localhost",
            // look-alikes of the app's origin
            "tauri://example.com/",
            "tauri://localhost:8080/",
            "tauri://localhost.evil.example/",
            "tauri://evil.example/localhost",
            "http://tauri.localhost:8080/",
            "https://tauri.localhost:8443/",
            "http://tauri.localhost.evil.example/",
            "http://evil.example/tauri.localhost",
            "ws://tauri.localhost/",
            // credentials, whichever way round the trick goes
            "http://tauri.localhost@evil.example/",
            "http://evil.example@tauri.localhost/",
            "http://user:pass@tauri.localhost/",
            "tauri://user@localhost/",
            // other schemes
            "file:///C:/Users/x/page.html",
            "data:text/html,hi",
            "javascript:alert(1)",
            "blob:http://tauri.localhost/00000000-0000-0000-0000-000000000000",
            "ipc://localhost/dispatch",
            "asset://localhost/x",
            "ftp://tauri.localhost/",
            "mailto:ada@example.com",
            "chrome://settings",
            "edge://settings",
            "view-source:http://tauri.localhost/",
            // only the blank page itself
            "about:srcdoc",
            "about:blank#frag",
            "about:blank?x=1",
        ] {
            assert!(!allowed(raw, false), "release: {raw}");
            assert!(!allowed(raw, true), "debug: {raw}");
        }
    }

    #[test]
    fn the_dev_server_is_allowed_in_debug_builds_only() {
        for raw in [
            "http://localhost:1420",
            "http://localhost:1420/",
            "http://localhost:1420/dev.html?seed=demo#x",
            "ws://localhost:1420/",
            "ws://localhost:1420/?token=abc",
        ] {
            assert!(allowed(raw, true), "debug: {raw}");
            assert!(!allowed(raw, false), "release: {raw}");
        }
    }

    #[test]
    fn only_the_exact_dev_server_origin_is_allowed_in_debug_builds() {
        for raw in [
            "http://localhost:1421/",
            "http://localhost/",
            "http://localhost:80/",
            "https://localhost:1420/",
            "wss://localhost:1420/",
            "ws://localhost/",
            "http://127.0.0.1:1420/",
            "http://[::1]:1420/",
            "http://localhost.evil.example:1420/",
            "http://evil.example:1420/",
            "http://localhost:1420@evil.example/",
            "http://evil.example@localhost:1420/",
            "ws://localhost:1420@evil.example/",
            "tauri://localhost:1420/",
        ] {
            assert!(!allowed(raw, true), "debug: {raw}");
            assert!(!allowed(raw, false), "release: {raw}");
        }
    }

    #[test]
    fn a_blocked_navigation_is_logged_with_its_scheme_and_host_only() {
        let logs = captured_logs(|| {
            let blocked = url("https://evil.example/private/page?token=SECRET#frag");
            assert!(!allow_navigation(&blocked));
        });
        assert!(logs.contains("WARN"), "{logs}");
        assert!(logs.contains("https"), "{logs}");
        assert!(logs.contains("evil.example"), "{logs}");
        for content in ["private", "SECRET", "token", "frag"] {
            assert!(!logs.contains(content), "{content} leaked: {logs}");
        }
    }

    #[test]
    fn an_allowed_navigation_is_not_logged() {
        let logs = captured_logs(|| {
            assert!(allow_navigation(&url("tauri://localhost/")));
        });
        assert!(!logs.contains("blocked"), "{logs}");
    }

    // --- open_external ---

    #[test]
    fn web_and_mail_links_pass_in_their_normalised_form() {
        for (raw, expected) in [
            ("https://example.com", "https://example.com/"),
            (
                "http://example.com/a b?q=1#frag",
                "http://example.com/a%20b?q=1#frag",
            ),
            ("HTTPS://EXAMPLE.COM/Path", "https://example.com/Path"),
            ("  https://example.com/  ", "https://example.com/"),
            (
                "https://user@example.com:8443/x",
                "https://user@example.com:8443/x",
            ),
            ("https://[::1]:8080/", "https://[::1]:8080/"),
            (
                "https://m\u{fc}nchen.example/",
                "https://xn--mnchen-3ya.example/",
            ),
            ("mailto:ada@example.com", "mailto:ada@example.com"),
            (
                "MAILTO:ada@example.com?subject=Hello%20there",
                "mailto:ada@example.com?subject=Hello%20there",
            ),
        ] {
            assert_eq!(external_url(raw).unwrap().as_str(), expected, "{raw}");
        }
    }

    #[test]
    fn mail_links_that_attach_files_are_refused() {
        for raw in [
            "mailto:ada@example.com?attach=C:\\secrets.txt",
            "mailto:ada@example.com?subject=x&Attach=/etc/passwd",
            "mailto:ada@example.com?attachment=~/.ssh/id_ed25519",
        ] {
            let error = external_url(raw).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{raw:?}");
            assert!(!error.message.contains("secrets"), "{}", error.message);
        }
        assert!(external_url("mailto:ada@example.com?subject=attach%20this&body=hi").is_ok());
    }

    #[test]
    fn links_that_could_launch_something_are_refused() {
        for raw in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:alert(1)",
            "java\tscript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///C:/Windows/System32/calc.exe",
            "file://server/share/doc.docx",
            "ftp://example.com/",
            "tel:+15551234567",
            "sms:+15551234567",
            "ssh://example.com/",
            "tauri://localhost/",
            "ipc://localhost/dispatch",
            "asset://localhost/x",
            "blob:https://example.com/00000000-0000-0000-0000-000000000000",
            "vbscript:msgbox(1)",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "about:blank",
            "chrome://settings",
            "C:\\Windows\\System32\\calc.exe",
            "\\\\server\\share\\payload.exe",
        ] {
            let error = external_url(raw).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{raw:?}");
            assert_eq!(
                error.details,
                Some(serde_json::json!({ "field": "url" })),
                "{raw:?}"
            );
            assert!(
                !error.message.contains(raw.trim()),
                "{raw:?}: {}",
                error.message
            );
        }
    }

    #[test]
    fn text_that_is_not_a_url_is_refused() {
        for raw in [
            "",
            "   ",
            "example.com",
            "/relative/path",
            "https://",
            "http://exa mple.com/",
            "https://[::1",
            "://example.com",
            "https://exa\u{0}mple.com/",
        ] {
            let error = external_url(raw).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{raw:?}");
        }
    }

    #[test]
    fn a_refused_link_is_logged_with_its_scheme_at_most() {
        let logs = captured_logs(|| {
            assert!(external_url("file:///C:/Users/ada/secret-plans.docx?x=1#frag7").is_err());
            assert!(external_url("this is secret-plans, not a URL").is_err());
        });
        assert!(logs.contains("file"), "{logs}");
        assert!(logs.contains("WARN"), "{logs}");
        for content in ["ada", "secret", "plans", "docx", "frag7"] {
            assert!(!logs.contains(content), "{content} leaked: {logs}");
        }
    }

    #[test]
    fn an_accepted_link_is_not_logged() {
        let logs = captured_logs(|| {
            assert!(external_url("https://example.com/private").is_ok());
        });
        assert!(logs.is_empty(), "{logs}");
    }

    fn set(key: &str) -> Request {
        Request::SettingsSet {
            key: key.to_owned(),
            value: serde_json::json!("C:\\Elsewhere"),
        }
    }

    #[test]
    fn the_webview_cannot_write_the_settings_the_shell_owns() {
        for key in [BACKUP_FOLDER_SETTING, BACKUP_LAST_SETTING] {
            let error = webview_request_allowed(&set(key)).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{key}");
            assert_eq!(
                error
                    .details
                    .as_ref()
                    .and_then(|d| d.get("field")?.as_str()),
                Some("key"),
                "{key}"
            );
            assert!(!error.message.contains("Elsewhere"), "{}", error.message);
        }
    }

    #[test]
    fn the_webview_cannot_write_the_ai_endpoint() {
        let error = webview_request_allowed(&set(AI_ENDPOINT_SETTING)).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(
            error
                .details
                .as_ref()
                .and_then(|d| d.get("field")?.as_str()),
            Some("key")
        );
        // The value (a remote URL with consent) is never echoed.
        assert!(!error.message.contains("Elsewhere"), "{}", error.message);
        // Reading it is allowed: the settings screen shows the endpoint.
        let read = Request::SettingsGet {
            key: AI_ENDPOINT_SETTING.to_owned(),
        };
        assert!(webview_request_allowed(&read).is_ok());
        // The settings the webview does write.
        for key in ["app.ai.settings", "app.mcp.settings", "app.ai.endpoint2"] {
            assert!(webview_request_allowed(&set(key)).is_ok(), "{key}");
        }
    }

    #[test]
    fn the_webview_cannot_send_the_requests_only_the_shell_makes() {
        use kb_types::ai::{AiChunkVector, McpGrant};
        let page_id = kb_types::PageId::from_bytes([1; 16]);
        let space_id = kb_types::SpaceId::from_bytes([2; 16]);
        for request in [
            Request::AiIndexNext { max_chunks: 16 },
            Request::AiIndexStore {
                page_id,
                source_hash: "00".repeat(32),
                model: "nomic-embed-text".into(),
                vectors: vec![AiChunkVector {
                    seq: 0,
                    vector: vec![0.5; 16],
                }],
            },
            Request::McpCall {
                token: "kbm_SECRETTOKEN".into(),
                message: "{}".into(),
            },
            Request::McpClientGrantsSet {
                client_id: "c".repeat(32),
                grants: vec![McpGrant {
                    space_id,
                    read: true,
                    write: true,
                }],
            },
        ] {
            let kind = request.kind();
            let error = webview_request_allowed(&request).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{kind}");
            assert_eq!(
                error
                    .details
                    .as_ref()
                    .and_then(|d| d.get("field")?.as_str()),
                Some("type"),
                "{kind}"
            );
            assert!(!error.message.contains("SECRET"), "{}", error.message);
        }
    }

    #[test]
    fn a_refused_shell_only_request_is_logged_by_kind_and_nothing_else() {
        let logs = captured_logs(|| {
            let call = Request::McpCall {
                token: "kbm_SECRETTOKEN".into(),
                message: "PAGETEXT".into(),
            };
            assert!(webview_request_allowed(&call).is_err());
        });
        assert!(logs.contains("McpCall"), "{logs}");
        for secret in ["SECRETTOKEN", "PAGETEXT"] {
            assert!(!logs.contains(secret), "{secret} leaked: {logs}");
        }
    }

    #[test]
    fn the_requests_the_webview_makes_for_ai_and_mcp_pass() {
        use kb_types::ai::AiScope;
        for request in [
            Request::AiIndexStatus,
            Request::AiIndexClear,
            Request::AiRetrieve {
                query: "q".into(),
                vector: None,
                scope: AiScope::All,
                limit: None,
            },
            Request::McpClientList,
            Request::McpClientCreate {
                name: "Claude".into(),
            },
            Request::McpClientDelete {
                client_id: "c".repeat(32),
            },
            Request::McpAuditList {
                client_id: None,
                before: None,
                limit: None,
            },
            Request::McpAuditClear,
        ] {
            let kind = request.kind();
            assert!(webview_request_allowed(&request).is_ok(), "{kind}");
        }
    }

    #[test]
    fn every_other_request_passes_the_webview_check() {
        for key in [
            "app.backup.schedule",
            "app.backup.folder.extra",
            "app.backup",
            "ui.theme",
            "app.backup.last2",
        ] {
            assert!(webview_request_allowed(&set(key)).is_ok(), "{key}");
        }
        // Reading them is allowed: the settings screen shows the folder and the last result.
        for key in [BACKUP_FOLDER_SETTING, BACKUP_LAST_SETTING] {
            let read = Request::SettingsGet {
                key: key.to_owned(),
            };
            assert!(webview_request_allowed(&read).is_ok(), "{key}");
        }
        assert!(webview_request_allowed(&Request::AppStatus).is_ok());
    }
}
