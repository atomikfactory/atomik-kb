#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::time::parse_rfc3339_ms;
use crate::wire::{constant_time_eq, parse_query, q_quote};

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn client() -> OAuthClient {
    OAuthClient {
        client_id: "1234-abc.apps.googleusercontent.com".to_owned(),
        client_secret: Some("GOCSPX-s3cret/value".to_owned()),
    }
}

fn response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body: body.as_bytes().to_vec(),
    }
}

fn json_response(status: u16, body: &str) -> HttpResponse {
    response(status, &[("Content-Type", "application/json")], body)
}

/// The decoded query of a URL.
fn query_of(url: &str) -> Vec<(String, String)> {
    let (_, query) = url.split_once('?').expect("a query");
    parse_query(query)
}

fn value_of(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

/// The decoded form body of a request.
fn body_pairs(request: &HttpRequest) -> Vec<(String, String)> {
    parse_query(std::str::from_utf8(&request.body).unwrap())
}

fn header_of<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn body_json(request: &HttpRequest) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

fn path_of(url: &str) -> &str {
    url.split('?').next().unwrap()
}

// ---------------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------------

#[test]
fn constants_are_the_contract() {
    assert_eq!(
        AUTH_ENDPOINT,
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(TOKEN_ENDPOINT, "https://oauth2.googleapis.com/token");
    assert_eq!(REVOKE_ENDPOINT, "https://oauth2.googleapis.com/revoke");
    assert_eq!(API, "https://www.googleapis.com");
    assert_eq!(SCOPE, "https://www.googleapis.com/auth/drive.file");
    assert_eq!(FOLDER_NAME, "atomik-kb backups");
    assert_eq!(FOLDER_MIME, "application/vnd.google-apps.folder");
    assert_eq!(BACKUP_MIME, "application/octet-stream");
    assert_eq!(MAX_LISTED, 200);
    assert_eq!(CHUNK_SIZE, 8 * 1024 * 1024);
    assert_eq!(
        CHUNK_SIZE % (256 * 1024),
        0,
        "Drive wants multiples of 256 KiB"
    );
}

// ---------------------------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------------------------

#[test]
fn pkce_matches_rfc_7636_appendix_b() {
    let random: [u8; 32] = [
        116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37,
        77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
    ];
    let pair = pkce(random);
    assert_eq!(pair.verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(
        pair.challenge,
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn pkce_is_43_url_safe_characters_without_padding() {
    for random in [[0u8; 32], [0xff; 32], [0xfb; 32]] {
        let pair = pkce(random);
        for text in [&pair.verifier, &pair.challenge] {
            assert_eq!(text.len(), 43);
            assert!(
                text.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{text}"
            );
        }
    }
}

#[test]
fn pkce_is_deterministic_and_depends_on_the_bytes() {
    assert_eq!(pkce([7; 32]), pkce([7; 32]));
    assert_ne!(pkce([7; 32]).verifier, pkce([8; 32]).verifier);
    assert_ne!(pkce([7; 32]).challenge, pkce([8; 32]).challenge);
    let pair = pkce([7; 32]);
    assert_ne!(pair.verifier, pair.challenge);
}

// ---------------------------------------------------------------------------------------------
// The authorisation URL and the redirect
// ---------------------------------------------------------------------------------------------

#[test]
fn authorize_url_carries_every_parameter() {
    let url = authorize_url(
        &client(),
        "http://127.0.0.1:51234",
        "the-state",
        "the-challenge",
    );
    assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
    let query = query_of(&url);
    let expected = [
        ("client_id", "1234-abc.apps.googleusercontent.com"),
        ("redirect_uri", "http://127.0.0.1:51234"),
        ("response_type", "code"),
        ("scope", "https://www.googleapis.com/auth/drive.file"),
        ("state", "the-state"),
        ("code_challenge", "the-challenge"),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    assert_eq!(query.len(), expected.len());
    for (key, value) in expected {
        assert_eq!(value_of(&query, key).as_deref(), Some(value), "{key}");
    }
}

#[test]
fn authorize_url_percent_encodes_every_value() {
    let url = authorize_url(&client(), "http://127.0.0.1:51234", "a b&c=d/é", "x");
    // Raw text: the reserved characters of the redirect and the scope are escaped.
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A51234&"));
    assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file&"));
    assert!(url.contains("state=a%20b%26c%3Dd%2F%C3%A9&"));
    // The query has exactly nine parameters, whatever the values hold.
    assert_eq!(query_of(&url).len(), 9);
    assert_eq!(
        value_of(&query_of(&url), "state").as_deref(),
        Some("a b&c=d/é")
    );
}

#[test]
fn encoding_keeps_the_rfc_3986_unreserved_set_only() {
    assert_eq!(encode("AZaz09-._~"), "AZaz09-._~");
    assert_eq!(
        encode(" !*'();:@&=+$,/?#[]%"),
        "%20%21%2A%27%28%29%3B%3A%40%26%3D%2B%24%2C%2F%3F%23%5B%5D%25"
    );
    assert_eq!(encode("é"), "%C3%A9");
}

#[test]
fn parse_redirect_returns_the_code() {
    assert_eq!(
        parse_redirect("code=4%2F0AbCdEf&scope=x&state=abc", "abc").unwrap(),
        "4/0AbCdEf"
    );
    assert_eq!(
        parse_redirect("state=abc&code=4/0AbC", "abc").unwrap(),
        "4/0AbC"
    );
    // A leading `?` is tolerated, `+` is a space.
    assert_eq!(parse_redirect("?code=a+b&state=s", "s").unwrap(), "a b");
}

#[test]
fn parse_redirect_checks_the_state() {
    assert_eq!(
        parse_redirect("code=c&state=other", "abc"),
        Err(DriveError::State)
    );
    assert_eq!(parse_redirect("code=c", "abc"), Err(DriveError::State));
    assert_eq!(
        parse_redirect("code=c&state=", "abc"),
        Err(DriveError::State)
    );
    assert_eq!(
        parse_redirect("code=c&state=ABC", "abc"),
        Err(DriveError::State)
    );
    assert_eq!(
        parse_redirect("code=c&state=abcd", "abc"),
        Err(DriveError::State)
    );
    // An empty expectation never matches, even an empty state.
    assert_eq!(parse_redirect("code=c&state=", ""), Err(DriveError::State));
    assert_eq!(parse_redirect("code=c", ""), Err(DriveError::State));
    // The state is checked before the answer: a forged refusal is not a refusal.
    assert_eq!(
        parse_redirect("error=access_denied&state=forged", "abc"),
        Err(DriveError::State)
    );
}

#[test]
fn parse_redirect_maps_the_errors() {
    assert_eq!(
        parse_redirect("error=access_denied&state=abc", "abc"),
        Err(DriveError::Denied)
    );
    let other = parse_redirect("error=invalid_scope&state=abc", "abc").unwrap_err();
    assert!(matches!(other, DriveError::Protocol(_)));
    assert!(!other.to_string().contains("invalid_scope"));
    for query in ["state=abc", "state=abc&code=", "state=abc&scope=x"] {
        assert!(
            matches!(parse_redirect(query, "abc"), Err(DriveError::Protocol(_))),
            "{query}"
        );
    }
}

#[test]
fn constant_time_eq_compares_whole_strings() {
    assert!(constant_time_eq("abc", "abc"));
    assert!(constant_time_eq("", ""));
    assert!(!constant_time_eq("abc", "abd"));
    assert!(!constant_time_eq("abc", "ab"));
}

// ---------------------------------------------------------------------------------------------
// Token requests and answers
// ---------------------------------------------------------------------------------------------

#[test]
fn token_request_is_a_form_post() {
    let request = token_request(
        &client(),
        "4/0Code",
        "the-verifier",
        "http://127.0.0.1:5000",
    );
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, TOKEN_ENDPOINT);
    assert_eq!(
        header_of(&request, "content-type"),
        Some("application/x-www-form-urlencoded")
    );
    let pairs = body_pairs(&request);
    let expected = [
        ("client_id", "1234-abc.apps.googleusercontent.com"),
        ("client_secret", "GOCSPX-s3cret/value"),
        ("code", "4/0Code"),
        ("code_verifier", "the-verifier"),
        ("grant_type", "authorization_code"),
        ("redirect_uri", "http://127.0.0.1:5000"),
    ];
    assert_eq!(pairs.len(), expected.len());
    for (key, value) in expected {
        assert_eq!(value_of(&pairs, key).as_deref(), Some(value), "{key}");
    }
    // The raw body is percent-encoded (the slash of the code).
    let raw = String::from_utf8(request.body).unwrap();
    assert!(raw.contains("code=4%2F0Code&"));
    assert!(!raw.contains(' '));
}

#[test]
fn a_client_without_secret_sends_none() {
    for secret in [None, Some(String::new())] {
        let client = OAuthClient {
            client_id: "id".to_owned(),
            client_secret: secret,
        };
        let exchange = body_pairs(&token_request(&client, "c", "v", "r"));
        assert_eq!(value_of(&exchange, "client_secret"), None);
        assert_eq!(value_of(&exchange, "client_id").as_deref(), Some("id"));
        let refresh = body_pairs(&refresh_request(&client, "rt"));
        assert_eq!(value_of(&refresh, "client_secret"), None);
    }
}

#[test]
fn refresh_request_is_a_form_post() {
    let request = refresh_request(&client(), "1//refresh-token");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, TOKEN_ENDPOINT);
    assert_eq!(
        header_of(&request, "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
    let pairs = body_pairs(&request);
    assert_eq!(pairs.len(), 4);
    assert_eq!(
        value_of(&pairs, "refresh_token").as_deref(),
        Some("1//refresh-token")
    );
    assert_eq!(
        value_of(&pairs, "grant_type").as_deref(),
        Some("refresh_token")
    );
    assert_eq!(
        value_of(&pairs, "client_id").as_deref(),
        Some("1234-abc.apps.googleusercontent.com")
    );
    assert_eq!(
        value_of(&pairs, "client_secret").as_deref(),
        Some("GOCSPX-s3cret/value")
    );
}

#[test]
fn revoke_request_is_a_form_post_of_the_token() {
    let request = revoke_request("1//the token");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, REVOKE_ENDPOINT);
    assert_eq!(
        header_of(&request, "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        String::from_utf8(request.body).unwrap(),
        "token=1%2F%2Fthe%20token"
    );
}

#[test]
fn parse_token_reads_the_answer() {
    let tokens = parse_token(&json_response(
        200,
        r#"{"access_token":"ya29.A","expires_in":3599,"refresh_token":"1//R","scope":"x","token_type":"Bearer"}"#,
    ))
    .unwrap();
    assert_eq!(
        tokens,
        Tokens {
            access_token: "ya29.A".to_owned(),
            expires_in_s: 3599,
            refresh_token: Some("1//R".to_owned()),
        }
    );
    // A refresh usually carries no new refresh token.
    let tokens = parse_token(&json_response(
        200,
        r#"{"access_token":"ya29.B","expires_in":3600}"#,
    ))
    .unwrap();
    assert_eq!(tokens.refresh_token, None);
    assert_eq!(tokens.expires_in_s, 3600);
    // A lifetime sent as text is read too.
    let tokens = parse_token(&json_response(
        200,
        r#"{"access_token":"ya29.C","expires_in":"120","refresh_token":""}"#,
    ))
    .unwrap();
    assert_eq!(tokens.expires_in_s, 120);
    assert_eq!(tokens.refresh_token, None);
}

#[test]
fn parse_token_rejects_an_incomplete_answer() {
    for body in [
        r#"{"expires_in":3600}"#,
        r#"{"access_token":"","expires_in":3600}"#,
        r#"{"access_token":"ya29.A"}"#,
        r#"{"access_token":"ya29.A","expires_in":"soon"}"#,
        r#"{"access_token":"ya29.A","expires_in":-1}"#,
        r#"{"access_token":7,"expires_in":1}"#,
        "not json",
        "",
    ] {
        let error = parse_token(&json_response(200, body)).unwrap_err();
        assert!(matches!(error, DriveError::Protocol(_)), "{body}");
        assert!(!error.to_string().contains("ya29"), "{body}");
    }
}

#[test]
fn parse_token_asks_to_reconnect_for_a_dead_grant_or_client() {
    for (status, error) in [
        (400, "invalid_grant"),
        (401, "invalid_client"),
        (400, "invalid_client"),
        (400, "unauthorized_client"),
        (401, "invalid_grant"),
    ] {
        let body = format!(r#"{{"error":"{error}","error_description":"Bad Request"}}"#);
        assert_eq!(
            parse_token(&json_response(status, &body)),
            Err(DriveError::Reconnect),
            "{status} {error}"
        );
    }
    // A bare 401 is a reconnection too.
    assert_eq!(
        parse_token(&json_response(401, "")),
        Err(DriveError::Reconnect)
    );
}

#[test]
fn parse_token_maps_the_other_failures() {
    let other = parse_token(&json_response(400, r#"{"error":"invalid_request"}"#)).unwrap_err();
    assert!(matches!(other, DriveError::Protocol(_)));
    assert!(matches!(
        parse_token(&json_response(400, "<html>")),
        Err(DriveError::Protocol(_))
    ));
    // `invalid_grant` with another status is not the reconnection signal.
    assert!(matches!(
        parse_token(&json_response(500, r#"{"error":"invalid_grant"}"#)),
        Err(DriveError::Server)
    ));
    assert_eq!(
        parse_token(&json_response(503, "")),
        Err(DriveError::Server)
    );
    assert_eq!(
        parse_token(&json_response(429, "")),
        Err(DriveError::RateLimited)
    );
}

// ---------------------------------------------------------------------------------------------
// Drive requests
// ---------------------------------------------------------------------------------------------

#[test]
fn about_request_asks_for_the_address_only() {
    let request = about_request("ya29.token");
    assert_eq!(request.method, "GET");
    assert_eq!(
        path_of(&request.url),
        "https://www.googleapis.com/drive/v3/about"
    );
    assert_eq!(
        value_of(&query_of(&request.url), "fields").as_deref(),
        Some("user(emailAddress)")
    );
    assert_eq!(
        header_of(&request, "Authorization"),
        Some("Bearer ya29.token")
    );
    assert!(request.body.is_empty());
}

#[test]
fn parse_about_reads_the_address() {
    let address = parse_about(&json_response(
        200,
        r#"{"user":{"emailAddress":"a@b.example"}}"#,
    ));
    assert_eq!(address, Ok(Some("a@b.example".to_owned())));
    for body in [r#"{}"#, r#"{"user":{}}"#, r#"{"user":{"emailAddress":""}}"#] {
        assert_eq!(parse_about(&json_response(200, body)), Ok(None), "{body}");
    }
    assert!(matches!(
        parse_about(&json_response(200, "nope")),
        Err(DriveError::Protocol(_))
    ));
    assert_eq!(
        parse_about(&json_response(401, "{}")),
        Err(DriveError::Reconnect)
    );
}

#[test]
fn find_folder_request_looks_for_the_folder_by_name_and_type() {
    let request = find_folder_request("tok");
    assert_eq!(request.method, "GET");
    assert_eq!(
        path_of(&request.url),
        "https://www.googleapis.com/drive/v3/files"
    );
    let query = query_of(&request.url);
    assert_eq!(
        value_of(&query, "q").as_deref(),
        Some(
            "name = 'atomik-kb backups' and mimeType = 'application/vnd.google-apps.folder' \
             and trashed = false"
        )
    );
    assert_eq!(value_of(&query, "spaces").as_deref(), Some("drive"));
    assert_eq!(header_of(&request, "Authorization"), Some("Bearer tok"));
}

#[test]
fn create_folder_request_posts_the_folder() {
    let request = create_folder_request("tok");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://www.googleapis.com/drive/v3/files");
    assert_eq!(header_of(&request, "Authorization"), Some("Bearer tok"));
    assert!(
        header_of(&request, "Content-Type")
            .unwrap()
            .starts_with("application/json")
    );
    assert_eq!(
        body_json(&request),
        json!({"name": "atomik-kb backups", "mimeType": "application/vnd.google-apps.folder"})
    );
}

#[test]
fn list_backups_request_filters_orders_and_pages() {
    let request = list_backups_request("tok", "FOLDER_1", None);
    assert_eq!(request.method, "GET");
    assert_eq!(
        path_of(&request.url),
        "https://www.googleapis.com/drive/v3/files"
    );
    assert_eq!(header_of(&request, "Authorization"), Some("Bearer tok"));
    let query = query_of(&request.url);
    assert_eq!(
        value_of(&query, "q").as_deref(),
        Some("'FOLDER_1' in parents and trashed = false and name contains '.atomikkb'")
    );
    assert_eq!(
        value_of(&query, "orderBy").as_deref(),
        Some("modifiedTime desc")
    );
    assert_eq!(value_of(&query, "pageSize").as_deref(), Some("200"));
    assert_eq!(
        value_of(&query, "fields").as_deref(),
        Some("nextPageToken,files(id,name,size,modifiedTime)")
    );
    assert_eq!(value_of(&query, "spaces").as_deref(), Some("drive"));
    assert_eq!(value_of(&query, "pageToken"), None);

    let next = list_backups_request("tok", "FOLDER_1", Some("page 2/=="));
    assert_eq!(
        value_of(&query_of(&next.url), "pageToken").as_deref(),
        Some("page 2/==")
    );
    // An empty token is no token.
    let none = list_backups_request("tok", "FOLDER_1", Some(""));
    assert_eq!(value_of(&query_of(&none.url), "pageToken"), None);
}

#[test]
fn queries_escape_quotes_and_backslashes() {
    assert_eq!(q_quote("plain"), "'plain'");
    assert_eq!(q_quote(r"a'b\c"), r"'a\'b\\c'");
    let request = list_backups_request("tok", r"x' or '1'='1\", None);
    let q = value_of(&query_of(&request.url), "q").unwrap();
    assert!(
        q.starts_with(r"'x\' or \'1\'=\'1\\' in parents and trashed = false"),
        "{q}"
    );
}

#[test]
fn parse_file_list_reads_files_and_the_next_page() {
    let body = r#"{
        "kind": "drive#fileList",
        "nextPageToken": "NEXT",
        "files": [
            {"id": "f1", "name": "atomik-kb-auto-2026-10-19-0930.atomikkb", "size": "1048576",
             "modifiedTime": "2026-10-19T07:30:12.345Z"},
            {"id": "f2", "name": "folder", "modifiedTime": "2026-10-18T09:30:00+02:00"},
            {"id": "f3", "name": "odd", "size": 12, "modifiedTime": "yesterday"}
        ]
    }"#;
    let (files, next) = parse_file_list(&json_response(200, body)).unwrap();
    assert_eq!(next.as_deref(), Some("NEXT"));
    assert_eq!(
        files,
        vec![
            DriveFile {
                id: "f1".to_owned(),
                name: "atomik-kb-auto-2026-10-19-0930.atomikkb".to_owned(),
                size: Some(1_048_576),
                modified_time_ms: parse_rfc3339_ms("2026-10-19T07:30:12.345Z"),
            },
            DriveFile {
                id: "f2".to_owned(),
                name: "folder".to_owned(),
                size: None,
                modified_time_ms: parse_rfc3339_ms("2026-10-18T07:30:00Z"),
            },
            DriveFile {
                id: "f3".to_owned(),
                name: "odd".to_owned(),
                size: Some(12),
                modified_time_ms: None,
            },
        ]
    );
    assert!(files[0].modified_time_ms.is_some());
}

#[test]
fn parse_file_list_handles_empty_and_last_pages() {
    assert_eq!(
        parse_file_list(&json_response(200, "{}")).unwrap(),
        (Vec::new(), None)
    );
    assert_eq!(
        parse_file_list(&json_response(200, r#"{"files":[],"nextPageToken":""}"#)).unwrap(),
        (Vec::new(), None)
    );
    let (files, next) =
        parse_file_list(&json_response(200, r#"{"files":[{"id":"a","name":"n"}]}"#)).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(next, None);
}

#[test]
fn parse_file_list_rejects_what_it_cannot_use() {
    for body in [
        r#"{"files":[{"name":"n"}]}"#,
        r#"{"files":[{"id":"","name":"n"}]}"#,
        r#"{"files":[{"id":"a"}]}"#,
        r#"{"files":"many"}"#,
        "[]",
        "",
    ] {
        assert!(
            matches!(
                parse_file_list(&json_response(200, body)),
                Err(DriveError::Protocol(_))
            ),
            "{body}"
        );
    }
    assert_eq!(
        parse_file_list(&json_response(401, "{}")),
        Err(DriveError::Reconnect)
    );
    assert_eq!(
        parse_file_list(&json_response(500, "{}")),
        Err(DriveError::Server)
    );
}

#[test]
fn parse_file_reads_one_file() {
    let file = parse_file(&json_response(
        200,
        r#"{"kind":"drive#file","id":"folder-id","name":"atomik-kb backups","mimeType":"application/vnd.google-apps.folder"}"#,
    ))
    .unwrap();
    assert_eq!(
        file,
        DriveFile {
            id: "folder-id".to_owned(),
            name: "atomik-kb backups".to_owned(),
            size: None,
            modified_time_ms: None,
        }
    );
    assert!(matches!(
        parse_file(&json_response(200, r#"{"name":"x"}"#)),
        Err(DriveError::Protocol(_))
    ));
    assert_eq!(
        parse_file(&json_response(404, "{}")),
        Err(DriveError::NotFound)
    );
}

// ---------------------------------------------------------------------------------------------
// The resumable upload
// ---------------------------------------------------------------------------------------------

#[test]
fn upload_start_request_carries_the_metadata() {
    let name = "atomik-kb-auto-2026-10-19-0930.atomikkb";
    let request = upload_start_request("tok", "FOLDER_1", name, 123_456);
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.url,
        "https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable"
    );
    assert_eq!(header_of(&request, "Authorization"), Some("Bearer tok"));
    assert!(
        header_of(&request, "Content-Type")
            .unwrap()
            .starts_with("application/json")
    );
    assert_eq!(
        header_of(&request, "X-Upload-Content-Type"),
        Some("application/octet-stream")
    );
    assert_eq!(
        header_of(&request, "X-Upload-Content-Length"),
        Some("123456")
    );
    assert_eq!(
        body_json(&request),
        json!({"name": name, "parents": ["FOLDER_1"], "mimeType": "application/octet-stream"})
    );
}

#[test]
fn upload_start_request_keeps_odd_names_inside_the_json() {
    let request = upload_start_request("tok", "F", "a \"b\"\n\\.atomikkb", 1);
    assert_eq!(body_json(&request)["name"], json!("a \"b\"\n\\.atomikkb"));
}

#[test]
fn parse_upload_session_reads_the_location_whatever_its_case() {
    for name in ["Location", "location", "LOCATION"] {
        let answer = response(
            200,
            &[(
                name,
                "https://www.googleapis.com/upload/drive/v3/files?upload_id=ABC",
            )],
            "",
        );
        assert_eq!(
            parse_upload_session(&answer).unwrap(),
            "https://www.googleapis.com/upload/drive/v3/files?upload_id=ABC"
        );
    }
}

#[test]
fn parse_upload_session_needs_a_location_and_a_2xx() {
    for answer in [
        response(200, &[], ""),
        response(200, &[("Location", "  ")], ""),
    ] {
        assert!(matches!(
            parse_upload_session(&answer),
            Err(DriveError::Protocol(_))
        ));
    }
    assert_eq!(
        parse_upload_session(&response(401, &[("Location", "https://x")], "")),
        Err(DriveError::Reconnect)
    );
    assert_eq!(
        parse_upload_session(&json_response(
            403,
            r#"{"error":{"errors":[{"reason":"storageQuotaExceeded"}]}}"#
        )),
        Err(DriveError::Quota)
    );
}

#[test]
fn upload_chunk_request_states_its_range() {
    let session = "https://www.googleapis.com/upload/drive/v3/files?upload_id=ABC";
    let first = upload_chunk_request(session, 0, vec![1; 100], 250);
    assert_eq!(first.method, "PUT");
    assert_eq!(first.url, session);
    assert_eq!(header_of(&first, "Content-Range"), Some("bytes 0-99/250"));
    assert_eq!(first.body, vec![1; 100]);
    assert_eq!(header_of(&first, "Authorization"), None);

    let middle = upload_chunk_request(session, 100, vec![2; 100], 250);
    assert_eq!(
        header_of(&middle, "Content-Range"),
        Some("bytes 100-199/250")
    );

    let last = upload_chunk_request(session, 200, vec![3; 50], 250);
    assert_eq!(header_of(&last, "Content-Range"), Some("bytes 200-249/250"));

    let single = upload_chunk_request(session, 0, vec![9], 1);
    assert_eq!(header_of(&single, "Content-Range"), Some("bytes 0-0/1"));
}

#[test]
fn an_empty_chunk_is_the_status_query() {
    let session = "https://example.test/session";
    let empty = upload_chunk_request(session, 0, Vec::new(), 0);
    assert_eq!(empty.method, "PUT");
    assert_eq!(header_of(&empty, "Content-Range"), Some("bytes */0"));
    assert!(empty.body.is_empty());
}

#[test]
fn upload_status_request_asks_with_an_unknown_range() {
    let session = "https://www.googleapis.com/upload/drive/v3/files?upload_id=ABC";
    let request = upload_status_request(session, 987_654_321);
    assert_eq!(request.method, "PUT");
    assert_eq!(request.url, session);
    assert_eq!(
        header_of(&request, "Content-Range"),
        Some("bytes */987654321")
    );
    assert!(request.body.is_empty());
}

#[test]
fn parse_upload_step_continues_after_the_range() {
    let step = parse_upload_step(&response(308, &[("Range", "bytes=0-8388607")], "")).unwrap();
    assert_eq!(
        step,
        UploadStep::Continue {
            next_offset: 8_388_608
        }
    );
    // The header's name has any case.
    let step = parse_upload_step(&response(308, &[("range", "bytes=0-0")], "")).unwrap();
    assert_eq!(step, UploadStep::Continue { next_offset: 1 });
    let step = parse_upload_step(&response(308, &[("RANGE", " bytes=0-41 ")], "")).unwrap();
    assert_eq!(step, UploadStep::Continue { next_offset: 42 });
}

#[test]
fn parse_upload_step_without_range_starts_over() {
    let step = parse_upload_step(&response(308, &[("Content-Length", "0")], "")).unwrap();
    assert_eq!(step, UploadStep::Continue { next_offset: 0 });
}

#[test]
fn parse_upload_step_rejects_an_unreadable_range() {
    for range in [
        "bytes=5-100",
        "bytes=0-",
        "bytes=0-x",
        "bytes=-5",
        "0-5",
        "bytes 0-5",
        "bytes=0-18446744073709551615",
        "",
    ] {
        assert!(
            matches!(
                parse_upload_step(&response(308, &[("Range", range)], "")),
                Err(DriveError::Protocol(_))
            ),
            "{range}"
        );
    }
}

#[test]
fn parse_upload_step_is_done_on_200_and_201() {
    for status in [200, 201] {
        let step = parse_upload_step(&json_response(
            status,
            r#"{"id":"new-id","name":"atomik-kb-auto-2026-10-19-0930.atomikkb","size":"77"}"#,
        ))
        .unwrap();
        assert_eq!(
            step,
            UploadStep::Done(DriveFile {
                id: "new-id".to_owned(),
                name: "atomik-kb-auto-2026-10-19-0930.atomikkb".to_owned(),
                size: Some(77),
                modified_time_ms: None,
            })
        );
    }
    assert!(matches!(
        parse_upload_step(&json_response(200, "")),
        Err(DriveError::Protocol(_))
    ));
}

#[test]
fn parse_upload_step_maps_the_failures() {
    assert_eq!(
        parse_upload_step(&response(404, &[], "")),
        Err(DriveError::NotFound)
    );
    assert_eq!(
        parse_upload_step(&response(401, &[], "")),
        Err(DriveError::Reconnect)
    );
    assert_eq!(
        parse_upload_step(&response(429, &[], "")),
        Err(DriveError::RateLimited)
    );
    for status in [500, 502, 503, 504] {
        assert_eq!(
            parse_upload_step(&response(status, &[], "")),
            Err(DriveError::Server)
        );
    }
    assert_eq!(
        parse_upload_step(&json_response(
            403,
            r#"{"error":{"errors":[{"reason":"storageQuotaExceeded"}]}}"#
        )),
        Err(DriveError::Quota)
    );
    for status in [204, 400, 410, 416] {
        assert!(
            matches!(
                parse_upload_step(&response(status, &[], "")),
                Err(DriveError::Protocol(_))
            ),
            "{status}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Download, delete, empty answers
// ---------------------------------------------------------------------------------------------

#[test]
fn download_request_asks_for_the_media() {
    let request = download_request("tok", "file-id_1");
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.url,
        "https://www.googleapis.com/drive/v3/files/file-id_1?alt=media"
    );
    assert_eq!(header_of(&request, "Authorization"), Some("Bearer tok"));
    assert!(request.body.is_empty());
}

#[test]
fn delete_request_deletes_the_file() {
    let request = delete_request("tok", "file-id_1");
    assert_eq!(request.method, "DELETE");
    assert_eq!(
        request.url,
        "https://www.googleapis.com/drive/v3/files/file-id_1"
    );
    assert_eq!(header_of(&request, "Authorization"), Some("Bearer tok"));
    assert!(request.body.is_empty());
}

#[test]
fn an_id_never_leaves_its_path_segment() {
    let download = download_request("tok", "../x?y=1/#z");
    assert_eq!(
        download.url,
        "https://www.googleapis.com/drive/v3/files/..%2Fx%3Fy%3D1%2F%23z?alt=media"
    );
    let delete = delete_request("tok", "a/b");
    assert_eq!(
        delete.url,
        "https://www.googleapis.com/drive/v3/files/a%2Fb"
    );
}

#[test]
fn parse_empty_accepts_any_2xx() {
    assert_eq!(parse_empty(&response(204, &[], "")), Ok(()));
    assert_eq!(parse_empty(&response(200, &[], "{}")), Ok(()));
    assert_eq!(
        parse_empty(&response(404, &[], "")),
        Err(DriveError::NotFound)
    );
    assert_eq!(
        parse_empty(&response(401, &[], "")),
        Err(DriveError::Reconnect)
    );
    assert!(matches!(
        parse_empty(&response(400, &[], r#"{"error":"invalid_token"}"#)),
        Err(DriveError::Protocol(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// classify
// ---------------------------------------------------------------------------------------------

fn forbidden(reason_json: &str) -> HttpResponse {
    json_response(403, reason_json)
}

#[test]
fn classify_maps_the_statuses() {
    for status in [200, 201, 204, 206, 299] {
        assert_eq!(classify(&response(status, &[], "")), Ok(()), "{status}");
    }
    assert_eq!(
        classify(&response(401, &[], "")),
        Err(DriveError::Reconnect)
    );
    assert_eq!(classify(&response(404, &[], "")), Err(DriveError::NotFound));
    assert_eq!(
        classify(&response(429, &[], "")),
        Err(DriveError::RateLimited)
    );
    for status in [500, 501, 502, 503, 504, 599] {
        assert_eq!(
            classify(&response(status, &[], "")),
            Err(DriveError::Server)
        );
    }
    for status in [100, 301, 302, 308, 400, 402, 405, 409, 410, 412, 416, 600] {
        assert!(
            matches!(
                classify(&response(status, &[], "")),
                Err(DriveError::Protocol(_))
            ),
            "{status}"
        );
    }
}

#[test]
fn classify_reads_the_403_reason() {
    let quota = forbidden(
        r#"{"error":{"code":403,"message":"The user's Drive storage quota has been exceeded.","errors":[{"message":"m","domain":"global","reason":"storageQuotaExceeded"}]}}"#,
    );
    assert_eq!(classify(&quota), Err(DriveError::Quota));
    for reason in ["userRateLimitExceeded", "rateLimitExceeded"] {
        let body = format!(
            r#"{{"error":{{"code":403,"errors":[{{"domain":"usageLimits","reason":"{reason}"}}]}}}}"#
        );
        assert_eq!(
            classify(&forbidden(&body)),
            Err(DriveError::RateLimited),
            "{reason}"
        );
    }
    // Reasons in `details` (ErrorInfo), in the other spelling, and among several entries.
    let details = forbidden(
        r#"{"error":{"code":403,"details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"RATE_LIMIT_EXCEEDED"}]}}"#,
    );
    assert_eq!(classify(&details), Err(DriveError::RateLimited));
    let several =
        forbidden(r#"{"error":{"errors":[{"reason":"other"},{"reason":"storageQuotaExceeded"}]}}"#);
    assert_eq!(classify(&several), Err(DriveError::Quota));
}

#[test]
fn classify_treats_another_403_as_a_protocol_error() {
    for body in [
        r#"{"error":{"errors":[{"reason":"forbidden"}]}}"#,
        r#"{"error":{"errors":[{"reason":"insufficientFilePermissions"}]}}"#,
        r#"{"error":{"errors":[]}}"#,
        r#"{"error":"access_denied"}"#,
        r#"{"error":{"errors":[{"nothing":1},{"reason":7}]}}"#,
        "<html>forbidden</html>",
        "",
    ] {
        assert!(
            matches!(classify(&forbidden(body)), Err(DriveError::Protocol(_))),
            "{body}"
        );
    }
}

#[test]
fn error_texts_never_hold_a_secret_an_id_or_a_name() {
    let body = r#"{"error":{"message":"secret-name.atomikkb ya29.TOKEN folder-ID-123","errors":[{"reason":"weird"}]}}"#;
    for status in [400, 401, 403, 404, 429, 500, 302] {
        let answer = json_response(status, body);
        for error in [
            classify(&answer),
            parse_empty(&answer),
            parse_token(&answer).map(|_| ()),
            parse_about(&answer).map(|_| ()),
            parse_file(&answer).map(|_| ()),
            parse_file_list(&answer).map(|_| ()),
            parse_upload_session(&answer).map(|_| ()),
            parse_upload_step(&answer).map(|_| ()),
        ] {
            let text = error.unwrap_err().to_string();
            for secret in ["secret-name", "ya29", "TOKEN", "folder-ID", "weird"] {
                assert!(!text.contains(secret), "{status}: {text}");
            }
        }
    }
    // An answer that parses wrongly does not quote the body either.
    let bad = json_response(
        200,
        r#"{"access_token":"ya29.SECRET","expires_in":{"x":1}}"#,
    );
    assert!(
        !parse_token(&bad)
            .unwrap_err()
            .to_string()
            .contains("SECRET")
    );
    let bad = json_response(200, r#"{"files":[{"id":"ID-SECRET","name":5}]}"#);
    assert!(
        !parse_file_list(&bad)
            .unwrap_err()
            .to_string()
            .contains("SECRET")
    );
    let redirect = parse_redirect("code=CODE-SECRET&state=ST-SECRET&error=weird", "ST-SECRET");
    assert!(!redirect.unwrap_err().to_string().contains("SECRET"));
}

#[test]
fn debug_output_hides_secrets() {
    let pair = pkce([5; 32]);
    let secrets = [
        format!("{:?}", client()),
        format!("{pair:?}"),
        format!(
            "{:?}",
            Tokens {
                access_token: "ya29.SECRET".to_owned(),
                expires_in_s: 1,
                refresh_token: Some("1//SECRET".to_owned()),
            }
        ),
        format!("{:?}", about_request("ya29.SECRET")),
        format!("{:?}", download_request("ya29.SECRET", "FILE-SECRET")),
        format!(
            "{:?}",
            token_request(
                &client(),
                "CODE-SECRET",
                &pair.verifier,
                "http://127.0.0.1:1"
            )
        ),
        format!(
            "{:?}",
            upload_chunk_request(
                "https://up.example/?upload_id=SESSION-SECRET",
                0,
                vec![1],
                1
            )
        ),
        format!(
            "{:?}",
            response(
                200,
                &[("Location", "https://up.example/?upload_id=SESSION-SECRET")],
                r#"{"access_token":"ya29.SECRET"}"#
            )
        ),
    ];
    for text in &secrets {
        for secret in ["SECRET", "s3cret", pair.verifier.as_str(), "GOCSPX"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
    // The useful parts stay: method, origin, status.
    assert!(secrets[3].contains("GET") && secrets[3].contains("https://www.googleapis.com"));
    assert!(secrets[7].contains("200"));
    assert!(secrets[2].contains("expires_in_s"));
}

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

#[test]
fn folder_url_points_at_the_drive_web_interface() {
    assert_eq!(
        folder_url("1AbC_d-Ef"),
        "https://drive.google.com/drive/folders/1AbC_d-Ef"
    );
    assert_eq!(
        folder_url("a/b?c"),
        "https://drive.google.com/drive/folders/a%2Fb%3Fc"
    );
}

#[test]
fn is_scheduled_name_reads_the_exact_form() {
    for name in [
        "atomik-kb-auto-2026-10-19-0930.atomikkb",
        "atomik-kb-auto-2024-02-29-2359.atomikkb",
        "atomik-kb-auto-2000-01-01-0000.atomikkb",
    ] {
        assert!(is_scheduled_name(name), "{name}");
    }
    for name in [
        "",
        "atomik-kb-auto-2026-10-19-0930.atomikkb.tmp",
        "atomik-kb-auto-2026-10-19-0930.ATOMIKKB",
        "atomik-kb-auto-2026-10-19-0930",
        "atomik-kb-auto-2026-10-19-0930.atomikk",
        "Atomik-kb-auto-2026-10-19-0930.atomikkb",
        "atomik-kb-2026-10-19-0930.atomikkb",
        "my-atomik-kb-auto-2026-10-19-0930.atomikkb",
        " atomik-kb-auto-2026-10-19-0930.atomikkb",
        "atomik-kb-auto-2026-10-19-930.atomikkb",
        "atomik-kb-auto-2026-10-19-09300.atomikkb",
        "atomik-kb-auto-2026-10-19_0930.atomikkb",
        "atomik-kb-auto-2026/10/19-0930.atomikkb",
        "atomik-kb-auto-26-10-19-0930.atomikkb",
        "atomik-kb-auto-2026-1-19-09-30.atomikkb",
        "atomik-kb-auto-2026-10-19-09a0.atomikkb",
        "atomik-kb-auto-202a-10-19-0930.atomikkb",
        "atomik-kb-auto-2026-10-19-+930.atomikkb",
        "atomik-kb-auto-2026-10-19-0930.atomikkb\n",
        "atomik-kb-auto-é026-10-19-0930.atomikkb",
        "atomik-kb-auto-2026-13-19-0930.atomikkb",
        "atomik-kb-auto-2026-00-19-0930.atomikkb",
        "atomik-kb-auto-2026-10-00-0930.atomikkb",
        "atomik-kb-auto-2026-10-32-0930.atomikkb",
        "atomik-kb-auto-2026-04-31-0930.atomikkb",
        "atomik-kb-auto-2026-02-29-0930.atomikkb",
        "atomik-kb-auto-2026-10-19-2400.atomikkb",
        "atomik-kb-auto-2026-10-19-0960.atomikkb",
    ] {
        assert!(!is_scheduled_name(name), "{name:?}");
    }
}

#[test]
fn is_backup_name_checks_the_extension() {
    assert!(is_backup_name("a.atomikkb"));
    assert!(is_backup_name("atomik-kb-auto-2026-10-19-0930.atomikkb"));
    assert!(is_backup_name("my backup.atomikkb"));
    assert!(!is_backup_name(".atomikkb"));
    assert!(!is_backup_name("a.atomikkb.bak"));
    assert!(!is_backup_name("a.zip"));
    assert!(!is_backup_name(""));
}

// ---------------------------------------------------------------------------------------------
// RFC 3339
// ---------------------------------------------------------------------------------------------

#[test]
fn rfc3339_reads_utc() {
    assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01Z"), Some(1_000));
    assert_eq!(
        parse_rfc3339_ms("2000-03-01T00:00:00Z"),
        Some(951_868_800_000)
    );
    assert_eq!(
        parse_rfc3339_ms("2024-02-29T12:00:00Z"),
        Some(1_709_208_000_000)
    );
    assert_eq!(
        parse_rfc3339_ms("2024-05-01T10:20:30.123Z"),
        Some(1_714_558_830_123)
    );
    assert_eq!(
        parse_rfc3339_ms("2017-01-01T00:00:00Z"),
        Some(1_483_228_800_000)
    );
    // Lower-case separators are allowed by the RFC.
    assert_eq!(
        parse_rfc3339_ms("2024-02-29t12:00:00z"),
        Some(1_709_208_000_000)
    );
}

#[test]
fn rfc3339_reads_offsets() {
    let utc = Some(1_709_208_000_000);
    assert_eq!(parse_rfc3339_ms("2024-02-29T14:00:00+02:00"), utc);
    assert_eq!(parse_rfc3339_ms("2024-02-29T07:00:00-05:00"), utc);
    assert_eq!(parse_rfc3339_ms("2024-02-29T17:30:00+05:30"), utc);
    assert_eq!(parse_rfc3339_ms("2024-02-29T12:00:00+00:00"), utc);
    assert_eq!(parse_rfc3339_ms("2024-02-29T12:00:00-00:00"), utc);
    // The offset can change the date.
    assert_eq!(
        parse_rfc3339_ms("2024-03-01T01:00:00+02:00"),
        parse_rfc3339_ms("2024-02-29T23:00:00Z")
    );
    assert_eq!(
        parse_rfc3339_ms("2024-02-28T23:00:00-02:00"),
        parse_rfc3339_ms("2024-02-29T01:00:00Z")
    );
}

#[test]
fn rfc3339_cuts_the_fraction_to_milliseconds() {
    let base = 1_709_208_000_000;
    assert_eq!(
        parse_rfc3339_ms("2024-02-29T12:00:00.123Z"),
        Some(base + 123)
    );
    assert_eq!(
        parse_rfc3339_ms("2024-02-29T12:00:00.123456Z"),
        Some(base + 123)
    );
    assert_eq!(parse_rfc3339_ms("2024-02-29T12:00:00.5Z"), Some(base + 500));
    assert_eq!(parse_rfc3339_ms("2024-02-29T12:00:00.05Z"), Some(base + 50));
    assert_eq!(parse_rfc3339_ms("2024-02-29T12:00:00.000Z"), Some(base));
    assert_eq!(
        parse_rfc3339_ms("2024-02-29T12:00:00.9999999999Z"),
        Some(base + 999)
    );
    assert_eq!(
        parse_rfc3339_ms("2024-02-29T14:00:00.250+02:00"),
        Some(base + 250)
    );
}

#[test]
fn rfc3339_reads_instants_before_the_epoch() {
    assert_eq!(parse_rfc3339_ms("1969-12-31T23:59:59Z"), Some(-1_000));
    assert_eq!(parse_rfc3339_ms("1969-12-31T23:59:59.999Z"), Some(-1));
    assert_eq!(parse_rfc3339_ms("1969-12-31T00:00:00Z"), Some(-86_400_000));
    assert_eq!(
        parse_rfc3339_ms("1900-01-01T00:00:00Z"),
        Some(-2_208_988_800_000)
    );
    assert_eq!(
        parse_rfc3339_ms("0001-01-01T00:00:00Z"),
        Some(-62_135_596_800_000)
    );
}

#[test]
fn rfc3339_checks_the_calendar() {
    assert!(parse_rfc3339_ms("2000-02-29T00:00:00Z").is_some());
    assert!(parse_rfc3339_ms("2024-02-29T00:00:00Z").is_some());
    assert!(parse_rfc3339_ms("2023-02-29T00:00:00Z").is_none());
    assert!(parse_rfc3339_ms("1900-02-29T00:00:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-04-31T00:00:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-13-01T00:00:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-00-01T00:00:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-01-00T00:00:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-01-32T00:00:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-01-01T24:00:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-01-01T00:60:00Z").is_none());
    assert!(parse_rfc3339_ms("2024-01-01T00:00:61Z").is_none());
    assert!(parse_rfc3339_ms("2024-01-01T00:00:00+24:00").is_none());
    assert!(parse_rfc3339_ms("2024-01-01T00:00:00+01:60").is_none());
    // A leap second is the next second.
    assert_eq!(
        parse_rfc3339_ms("2016-12-31T23:59:60Z"),
        parse_rfc3339_ms("2017-01-01T00:00:00Z")
    );
}

#[test]
fn rfc3339_rejects_other_shapes() {
    for text in [
        "",
        "yesterday",
        "2024-02-29",
        "2024-02-29T12:00:00",
        "2024-02-29T12:00Z",
        "2024-02-29 12:00:00Z",
        "2024-02-29T12:00:00.Z",
        "2024-02-29T12:00:00.1.2Z",
        "2024-02-29T12:00:00+0200",
        "2024-02-29T12:00:00+02",
        "2024-02-29T12:00:00+02:0",
        "2024-02-29T12:00:00Zjunk",
        "2024-02-29T12:00:00Z ",
        " 2024-02-29T12:00:00Z",
        "2024/02/29T12:00:00Z",
        "24-02-29T12:00:00Z",
        "2024-02-29T1a:00:00Z",
        "2024-02-29T12:00:00+0a:00",
        "+2024-02-29T12:00:00Z",
        "２０２４-02-29T12:00:00Z",
        "2024-02-29T12:00:00Zé",
        "2024-02-29T12:00:00é",
    ] {
        assert_eq!(parse_rfc3339_ms(text), None, "{text:?}");
    }
}

#[test]
fn a_full_flow_of_values_fits_together() {
    // Sign-in, token, listing: the values of one run chained as the shell chains them.
    let pair = pkce([42; 32]);
    let url = authorize_url(
        &client(),
        "http://127.0.0.1:4000",
        "state-1",
        &pair.challenge,
    );
    let query = query_of(&url);
    assert_eq!(
        value_of(&query, "code_challenge"),
        Some(pair.challenge.clone())
    );
    let code = parse_redirect("code=4%2F0X&scope=s&state=state-1", "state-1").unwrap();
    let exchange = token_request(&client(), &code, &pair.verifier, "http://127.0.0.1:4000");
    assert_eq!(
        value_of(&body_pairs(&exchange), "code_verifier"),
        Some(pair.verifier)
    );
    let tokens = parse_token(&json_response(
        200,
        r#"{"access_token":"ya29.X","expires_in":3599,"refresh_token":"1//Y"}"#,
    ))
    .unwrap();
    let list = list_backups_request(&tokens.access_token, "F", None);
    assert_eq!(header_of(&list, "Authorization"), Some("Bearer ya29.X"));
}
