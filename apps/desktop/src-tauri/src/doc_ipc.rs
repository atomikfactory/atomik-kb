//! The raw half of the IPC contract: page documents travel as bytes, never inside JSON
//! (ADR-0015). A raw command gets its page id from the `x-kb-page` request header and its
//! payload from the request body; this module turns those two into plain values, so the
//! commands stay thin and the rules are testable without a webview.

use std::borrow::Cow;

use kb_types::{ApiError, PageId};
use serde_json::Value;
use tauri::http::HeaderMap;
use tauri::ipc::InvokeBody;

/// Request header that carries the page id, as UUID text.
pub const PAGE_HEADER: &str = "x-kb-page";

/// The page a raw command is about.
///
/// A missing, non-ASCII or malformed header is `InvalidInput` for the field `x-kb-page`.
/// The message never repeats the header value.
pub fn page_id(headers: &HeaderMap) -> Result<PageId, ApiError> {
    let value = headers
        .get(PAGE_HEADER)
        .ok_or_else(|| ApiError::invalid(PAGE_HEADER, "the page id header is missing"))?;
    value
        .to_str()
        .ok()
        .and_then(|text| text.parse::<PageId>().ok())
        .ok_or_else(|| ApiError::invalid(PAGE_HEADER, "the page id must be UUID text"))
}

/// The bytes a raw request carries; empty means "no body".
///
/// The webview sends a `Uint8Array` as `application/octet-stream`, which Tauri hands over
/// as [`InvokeBody::Raw`] (borrowed here, not copied). "Nothing to send" is an empty raw
/// body, or JSON `{}` (an `invoke` without a payload) or `null`. When the custom protocol
/// is unavailable Tauri falls back to `postMessage`, which turns a `Uint8Array` into a
/// JSON array of numbers; that is accepted as well. Anything else is `InvalidInput`.
pub fn payload(body: &InvokeBody) -> Result<Cow<'_, [u8]>, ApiError> {
    match body {
        InvokeBody::Raw(bytes) => Ok(Cow::Borrowed(bytes.as_slice())),
        InvokeBody::Json(Value::Null) => Ok(Cow::Borrowed(&[])),
        InvokeBody::Json(Value::Object(fields)) if fields.is_empty() => Ok(Cow::Borrowed(&[])),
        InvokeBody::Json(Value::Array(numbers)) => numbers
            .iter()
            .map(|number| number.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect::<Option<Vec<u8>>>()
            .map(Cow::Owned)
            .ok_or_else(not_bytes),
        InvokeBody::Json(_) => Err(not_bytes()),
    }
}

/// The update a `doc_import` carries. An empty body is `InvalidInput`: there is nothing to
/// import.
pub fn update(body: &InvokeBody) -> Result<Cow<'_, [u8]>, ApiError> {
    let bytes = payload(body)?;
    if bytes.is_empty() {
        return Err(ApiError::invalid("body", "an update must not be empty"));
    }
    Ok(bytes)
}

/// The encoded version vector a `doc_export` carries; an empty body means "everything"
/// and is `None`.
pub fn since(body: &InvokeBody) -> Result<Option<Cow<'_, [u8]>>, ApiError> {
    let bytes = payload(body)?;
    Ok((!bytes.is_empty()).then_some(bytes))
}

fn not_bytes() -> ApiError {
    ApiError::invalid("body", "the body must be raw bytes")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use kb_types::ErrorCode;
    use serde_json::json;
    use tauri::http::{HeaderName, HeaderValue};

    use super::*;

    const PAGE: &str = "12345678-9abc-4def-8123-456789abcdef";

    fn page() -> PageId {
        PAGE.parse().unwrap()
    }

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(PAGE_HEADER, HeaderValue::from_str(value).unwrap());
        headers
    }

    /// The `field` an error names in its details.
    fn field(error: &ApiError) -> Option<&str> {
        error.details.as_ref()?.get("field")?.as_str()
    }

    /// Every way the webview can say "there is nothing to send".
    fn empty_bodies() -> Vec<InvokeBody> {
        vec![
            InvokeBody::Raw(Vec::new()),
            InvokeBody::Json(Value::Null),
            InvokeBody::Json(json!({})),
            InvokeBody::Json(json!([])),
            InvokeBody::default(),
        ]
    }

    // --- page id header ---

    #[test]
    fn the_header_name_is_the_documented_one() {
        assert_eq!(PAGE_HEADER, "x-kb-page");
    }

    #[test]
    fn the_page_id_is_read_from_the_header() {
        assert_eq!(page_id(&headers(PAGE)).unwrap(), page());
        assert_eq!(page_id(&headers(&PAGE.to_uppercase())).unwrap(), page());
    }

    #[test]
    fn header_names_are_matched_case_insensitively() {
        let mut map = HeaderMap::new();
        map.insert(
            HeaderName::from_bytes(b"X-KB-Page").unwrap(),
            HeaderValue::from_static(PAGE),
        );
        assert_eq!(page_id(&map).unwrap(), page());
    }

    #[test]
    fn a_missing_header_is_invalid_input_naming_the_header() {
        let error = page_id(&HeaderMap::new()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("x-kb-page"));
    }

    #[test]
    fn a_malformed_page_id_is_invalid_input_and_is_not_echoed() {
        for bad in [
            "",
            "not-a-uuid",
            "12345678-9abc-4def-8123-456789abcde",
            "12345678-9abc-4def-8123-456789abcdef0",
            "zz345678-9abc-4def-8123-456789abcdef",
            "{12345678-9abc-4def-8123-456789abcdef}",
            "12345678 9abc 4def 8123 456789abcdef",
            "../../etc/passwd",
        ] {
            let error = page_id(&headers(bad)).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{bad:?}");
            assert_eq!(field(&error), Some("x-kb-page"), "{bad:?}");
            assert!(
                bad.is_empty() || !error.message.contains(bad),
                "the message repeats the header value: {}",
                error.message
            );
        }
    }

    #[test]
    fn a_header_that_is_not_visible_ascii_is_rejected() {
        let mut map = HeaderMap::new();
        map.insert(PAGE_HEADER, HeaderValue::from_bytes(&[0xe9, 0x41]).unwrap());
        let error = page_id(&map).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("x-kb-page"));
    }

    // --- bodies ---

    #[test]
    fn a_raw_body_is_the_payload_and_is_not_copied() {
        let body = InvokeBody::Raw(vec![0, 1, 255]);
        let bytes = payload(&body).unwrap();
        assert_eq!(&*bytes, [0, 1, 255]);
        assert!(matches!(bytes, Cow::Borrowed(_)));
    }

    #[test]
    fn nothing_to_send_is_an_empty_payload_in_every_shape() {
        for body in empty_bodies() {
            assert!(payload(&body).unwrap().is_empty(), "{body:?}");
        }
    }

    #[test]
    fn a_json_array_of_bytes_is_the_postmessage_fallback_of_a_uint8array() {
        let body = InvokeBody::Json(json!([0, 1, 127, 128, 255]));
        assert_eq!(&*payload(&body).unwrap(), [0, 1, 127, 128, 255]);
    }

    #[test]
    fn other_json_bodies_are_invalid_input() {
        for body in [
            json!([256]),
            json!([-1]),
            json!([1.5]),
            json!(["1"]),
            json!([[1]]),
            json!([null]),
            json!([0, 1, 300]),
            json!({ "a": 1 }),
            json!("bytes"),
            json!(7),
            json!(true),
        ] {
            let error = payload(&InvokeBody::Json(body.clone())).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{body}");
            assert_eq!(field(&error), Some("body"), "{body}");
        }
    }

    #[test]
    fn an_import_needs_a_non_empty_update() {
        for body in empty_bodies() {
            let error = update(&body).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{body:?}");
            assert_eq!(field(&error), Some("body"), "{body:?}");
        }
        let body = InvokeBody::Raw(vec![4, 5, 6]);
        assert_eq!(&*update(&body).unwrap(), [4, 5, 6]);
        let body = InvokeBody::Json(json!([4, 5, 6]));
        assert_eq!(&*update(&body).unwrap(), [4, 5, 6]);
    }

    #[test]
    fn an_export_without_a_version_vector_asks_for_everything() {
        for body in empty_bodies() {
            assert!(since(&body).unwrap().is_none(), "{body:?}");
        }
        let body = InvokeBody::Raw(vec![9, 8]);
        assert_eq!(since(&body).unwrap().as_deref(), Some(&[9u8, 8][..]));
    }

    #[test]
    fn an_export_with_a_garbled_body_is_invalid_input() {
        let error = since(&InvokeBody::Json(json!({ "vv": [1] }))).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }
}
