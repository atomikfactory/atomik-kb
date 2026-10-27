//! The raw half of the attachment IPC contract (ADR-0020): attachment bytes travel as
//! bytes, never inside JSON (ADR-0015). Like the page-document commands (`doc_ipc`), a raw
//! attachment command gets its attachment id from the `x-kb-attachment` request header and
//! its payload from the request body; `attachment_read` also takes an optional byte range
//! from the `x-kb-range-start` and `x-kb-range-end` headers. This module turns headers and
//! bodies into plain values, so the commands stay thin and the rules are testable without a
//! webview.

use std::borrow::Cow;

use kb_types::attachments::{CHUNK_SIZE, MAX_PREVIEW_BYTES};
use kb_types::{ApiError, AttachmentId, ByteRange};
use serde_json::json;
use tauri::http::HeaderMap;
use tauri::ipc::InvokeBody;

use crate::doc_ipc;

/// Request header that carries the attachment id, as UUID text.
pub const ATTACHMENT_HEADER: &str = "x-kb-attachment";

/// Request header of `attachment_read` that carries the first byte of the range, as decimal
/// text.
pub const RANGE_START_HEADER: &str = "x-kb-range-start";

/// Request header of `attachment_read` that carries one past the last byte of the range, as
/// decimal text.
pub const RANGE_END_HEADER: &str = "x-kb-range-end";

/// `InvalidInput` for `field` that also says which limit was exceeded (`details.limit`), the
/// shape the core uses for `size` (ADR-0020).
pub fn limit_exceeded(field: &str, message: &str, limit: u64) -> ApiError {
    ApiError::invalid(field, message).with_details(json!({ "field": field, "limit": limit }))
}

/// The attachment a raw command is about.
///
/// A missing, non-ASCII or malformed header is `InvalidInput` for the field
/// `x-kb-attachment`. The message never repeats the header value.
pub fn attachment_id(headers: &HeaderMap) -> Result<AttachmentId, ApiError> {
    let value = headers.get(ATTACHMENT_HEADER).ok_or_else(|| {
        ApiError::invalid(ATTACHMENT_HEADER, "the attachment id header is missing")
    })?;
    value
        .to_str()
        .ok()
        .and_then(|text| text.parse::<AttachmentId>().ok())
        .ok_or_else(|| ApiError::invalid(ATTACHMENT_HEADER, "the attachment id must be UUID text"))
}

/// The byte range `attachment_read` was asked for: both range headers or neither.
///
/// - Neither header: `None`, the whole attachment (the shell then refuses one larger than
///   `MAX_PREVIEW_BYTES`).
/// - Only one of them, or a value that is not plain decimal digits that fit a `u64`:
///   `InvalidInput` for the field of the header in question.
/// - An empty or inverted range (`start >= end`), or one longer than `MAX_PREVIEW_BYTES`
///   (the most the shell answers in one piece): `InvalidInput` for the field `range`, the
///   latter with `details.limit`. A range past the end of the attachment is the core's to
///   refuse (it knows the size).
pub fn byte_range(headers: &HeaderMap) -> Result<Option<ByteRange>, ApiError> {
    let start = header_number(headers, RANGE_START_HEADER)?;
    let end = header_number(headers, RANGE_END_HEADER)?;
    let (start, end) = match (start, end) {
        (None, None) => return Ok(None),
        (Some(start), Some(end)) => (start, end),
        (Some(_), None) => {
            return Err(ApiError::invalid(
                RANGE_END_HEADER,
                "the range end header is missing: give both range headers or neither",
            ));
        }
        (None, Some(_)) => {
            return Err(ApiError::invalid(
                RANGE_START_HEADER,
                "the range start header is missing: give both range headers or neither",
            ));
        }
    };
    let length = match end.checked_sub(start) {
        Some(length) if length > 0 => length,
        _ => {
            return Err(ApiError::invalid(
                "range",
                "the range must end after it starts",
            ));
        }
    };
    if length > MAX_PREVIEW_BYTES {
        return Err(limit_exceeded(
            "range",
            "the range is longer than one read may answer; ask for it in pieces",
            MAX_PREVIEW_BYTES,
        ));
    }
    Ok(Some(ByteRange { start, end }))
}

/// A header that holds a decimal number: `None` when it is absent. Only ASCII digits are
/// accepted (no sign, no spaces, no radix prefix); a value too large for `u64` is refused.
fn header_number(headers: &HeaderMap, name: &str) -> Result<Option<u64>, ApiError> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse::<u64>().ok())
        .map(Some)
        .ok_or_else(|| ApiError::invalid(name, "the value must be a decimal number of bytes"))
}

/// The piece of plaintext an `attachment_write` carries: at least one byte and at most
/// `CHUNK_SIZE` (4 MiB). An empty body or a longer one is `InvalidInput` for the field
/// `body` (the latter with `details.limit`); a body that is not bytes is too.
pub fn piece(body: &InvokeBody) -> Result<Cow<'_, [u8]>, ApiError> {
    let bytes = doc_ipc::payload(body)?;
    if bytes.is_empty() {
        return Err(ApiError::invalid("body", "a piece must not be empty"));
    }
    if bytes.len() > CHUNK_SIZE {
        return Err(limit_exceeded(
            "body",
            "a piece must not be larger than one chunk",
            CHUNK_SIZE as u64,
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use kb_types::ErrorCode;
    use serde_json::{Value, json};
    use tauri::http::{HeaderName, HeaderValue};

    use super::*;

    const ID: &str = "12345678-9abc-4def-8123-456789abcdef";

    fn id() -> AttachmentId {
        ID.parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn field(error: &ApiError) -> Option<&str> {
        error.details.as_ref()?.get("field")?.as_str()
    }

    fn limit(error: &ApiError) -> Option<u64> {
        error.details.as_ref()?.get("limit")?.as_u64()
    }

    // --- attachment id header ---

    #[test]
    fn the_header_names_are_the_documented_ones() {
        assert_eq!(ATTACHMENT_HEADER, "x-kb-attachment");
        assert_eq!(RANGE_START_HEADER, "x-kb-range-start");
        assert_eq!(RANGE_END_HEADER, "x-kb-range-end");
    }

    #[test]
    fn the_attachment_id_is_read_from_the_header() {
        assert_eq!(
            attachment_id(&headers(&[(ATTACHMENT_HEADER, ID)])).unwrap(),
            id()
        );
        assert_eq!(
            attachment_id(&headers(&[(ATTACHMENT_HEADER, &ID.to_uppercase())])).unwrap(),
            id()
        );
        // Header names are case-insensitive.
        assert_eq!(
            attachment_id(&headers(&[("X-KB-Attachment", ID)])).unwrap(),
            id()
        );
    }

    #[test]
    fn a_missing_attachment_header_is_invalid_input_naming_the_header() {
        let error = attachment_id(&HeaderMap::new()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("x-kb-attachment"));
    }

    #[test]
    fn a_malformed_attachment_id_is_invalid_input_and_is_not_echoed() {
        for bad in [
            "",
            "not-a-uuid",
            "12345678-9abc-4def-8123-456789abcde",
            "12345678-9abc-4def-8123-456789abcdef0",
            "{12345678-9abc-4def-8123-456789abcdef}",
            "..\\..\\Windows\\System32",
            "../../etc/passwd",
        ] {
            let error = attachment_id(&headers(&[(ATTACHMENT_HEADER, bad)])).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{bad:?}");
            assert_eq!(field(&error), Some("x-kb-attachment"), "{bad:?}");
            assert!(
                bad.is_empty() || !error.message.contains(bad),
                "the message repeats the header value: {}",
                error.message
            );
        }
    }

    #[test]
    fn an_attachment_header_that_is_not_visible_ascii_is_rejected() {
        let mut map = HeaderMap::new();
        map.insert(
            ATTACHMENT_HEADER,
            HeaderValue::from_bytes(&[0xe9, 0x41]).unwrap(),
        );
        let error = attachment_id(&map).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("x-kb-attachment"));
    }

    // --- range headers ---

    #[test]
    fn no_range_headers_means_the_whole_attachment() {
        assert_eq!(byte_range(&HeaderMap::new()).unwrap(), None);
        assert_eq!(
            byte_range(&headers(&[(ATTACHMENT_HEADER, ID)])).unwrap(),
            None
        );
    }

    #[test]
    fn both_range_headers_make_a_half_open_range() {
        let range = byte_range(&headers(&[
            (RANGE_START_HEADER, "0"),
            (RANGE_END_HEADER, "10"),
        ]))
        .unwrap();
        assert_eq!(range, Some(ByteRange { start: 0, end: 10 }));

        let range = byte_range(&headers(&[
            (RANGE_START_HEADER, "4194304"),
            (RANGE_END_HEADER, "4194305"),
        ]))
        .unwrap();
        assert_eq!(
            range,
            Some(ByteRange {
                start: 4_194_304,
                end: 4_194_305
            })
        );
    }

    #[test]
    fn a_range_may_start_anywhere_in_a_two_gigabyte_file() {
        let start = 2_u64 * 1024 * 1024 * 1024 - 100;
        let range = byte_range(&headers(&[
            (RANGE_START_HEADER, &start.to_string()),
            (RANGE_END_HEADER, &(start + 100).to_string()),
        ]))
        .unwrap();
        assert_eq!(
            range,
            Some(ByteRange {
                start,
                end: start + 100
            })
        );
    }

    #[test]
    fn the_longest_range_is_the_preview_limit() {
        let at_limit = byte_range(&headers(&[
            (RANGE_START_HEADER, "5"),
            (RANGE_END_HEADER, &(5 + MAX_PREVIEW_BYTES).to_string()),
        ]))
        .unwrap();
        assert_eq!(
            at_limit,
            Some(ByteRange {
                start: 5,
                end: 5 + MAX_PREVIEW_BYTES
            })
        );

        let error = byte_range(&headers(&[
            (RANGE_START_HEADER, "5"),
            (RANGE_END_HEADER, &(6 + MAX_PREVIEW_BYTES).to_string()),
        ]))
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("range"));
        assert_eq!(limit(&error), Some(MAX_PREVIEW_BYTES));
    }

    #[test]
    fn one_range_header_without_the_other_is_refused() {
        let error = byte_range(&headers(&[(RANGE_START_HEADER, "0")])).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("x-kb-range-end"));

        let error = byte_range(&headers(&[(RANGE_END_HEADER, "10")])).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("x-kb-range-start"));
    }

    #[test]
    fn empty_and_inverted_ranges_are_invalid_input_for_the_range() {
        for (start, end) in [("5", "5"), ("0", "0"), ("10", "5"), ("1", "0")] {
            let error = byte_range(&headers(&[
                (RANGE_START_HEADER, start),
                (RANGE_END_HEADER, end),
            ]))
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{start}..{end}");
            assert_eq!(field(&error), Some("range"), "{start}..{end}");
        }
    }

    #[test]
    fn range_values_must_be_plain_decimal_numbers() {
        for bad in [
            "",
            " 5",
            "5 ",
            "+5",
            "-5",
            "0x10",
            "1e3",
            "5.0",
            "five",
            "1_000",
            // one more than u64::MAX
            "18446744073709551616",
        ] {
            let error = byte_range(&headers(&[
                (RANGE_START_HEADER, bad),
                (RANGE_END_HEADER, "100"),
            ]))
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{bad:?}");
            assert_eq!(field(&error), Some("x-kb-range-start"), "{bad:?}");

            let error = byte_range(&headers(&[
                (RANGE_START_HEADER, "0"),
                (RANGE_END_HEADER, bad),
            ]))
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{bad:?}");
            assert_eq!(field(&error), Some("x-kb-range-end"), "{bad:?}");
        }
    }

    #[test]
    fn leading_zeros_and_the_largest_u64_are_numbers() {
        let error = byte_range(&headers(&[
            (RANGE_START_HEADER, "0000"),
            (RANGE_END_HEADER, "18446744073709551615"),
        ]))
        .unwrap_err();
        // A perfectly good number, refused only for being a gigantic range.
        assert_eq!(field(&error), Some("range"));
        assert_eq!(limit(&error), Some(MAX_PREVIEW_BYTES));

        let range = byte_range(&headers(&[
            (RANGE_START_HEADER, "007"),
            (RANGE_END_HEADER, "0010"),
        ]))
        .unwrap();
        assert_eq!(range, Some(ByteRange { start: 7, end: 10 }));
    }

    // --- bodies ---

    #[test]
    fn a_piece_of_one_byte_up_to_one_chunk_is_accepted_and_not_copied() {
        let body = InvokeBody::Raw(vec![7]);
        let bytes = piece(&body).unwrap();
        assert_eq!(&*bytes, [7]);
        assert!(matches!(bytes, Cow::Borrowed(_)));

        let body = InvokeBody::Raw(vec![0; CHUNK_SIZE]);
        assert_eq!(piece(&body).unwrap().len(), CHUNK_SIZE);
    }

    #[test]
    fn an_empty_piece_is_invalid_input_for_the_body() {
        for body in [
            InvokeBody::Raw(Vec::new()),
            InvokeBody::Json(Value::Null),
            InvokeBody::Json(json!({})),
            InvokeBody::Json(json!([])),
            InvokeBody::default(),
        ] {
            let error = piece(&body).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{body:?}");
            assert_eq!(field(&error), Some("body"), "{body:?}");
            assert_eq!(limit(&error), None, "{body:?}");
        }
    }

    #[test]
    fn a_piece_larger_than_a_chunk_is_invalid_input_with_the_limit() {
        let error = piece(&InvokeBody::Raw(vec![0; CHUNK_SIZE + 1])).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("body"));
        assert_eq!(limit(&error), Some(CHUNK_SIZE as u64));
    }

    #[test]
    fn the_postmessage_fallback_is_accepted_and_garbage_is_not() {
        let body = InvokeBody::Json(json!([0, 1, 255]));
        assert_eq!(&*piece(&body).unwrap(), [0, 1, 255]);

        for body in [json!({ "a": 1 }), json!("bytes"), json!([256]), json!(7)] {
            let error = piece(&InvokeBody::Json(body.clone())).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{body}");
            assert_eq!(field(&error), Some("body"), "{body}");
        }
    }

    #[test]
    fn limit_exceeded_names_the_field_and_the_limit() {
        let error = limit_exceeded("size", "too big", 42);
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.details, Some(json!({ "field": "size", "limit": 42 })));
    }
}
