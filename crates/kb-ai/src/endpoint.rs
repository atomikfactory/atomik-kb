//! The Ollama endpoint policy (`docs/architecture/ai.md`, section 2): parsing, normalisation and
//! the local/remote classification that decides whether a consent is needed.
//!
//! The parser is written by hand (no URL crate) and is deliberately strict: anything it does not
//! understand is refused, and anything it cannot prove to be this machine is *remote*. A wrong
//! answer must never be "local".

use std::net::Ipv6Addr;

/// Longest endpoint accepted, in characters (checked on the input and on the normalised form).
pub const MAX_ENDPOINT_CHARS: usize = 512;

/// Longest host name (RFC 1035).
const MAX_HOST_CHARS: usize = 253;
/// Longest label of a host name.
const MAX_LABEL_CHARS: usize = 63;

/// A parsed, normalised endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `http(s)://host[:port]`, no trailing slash, host lower-cased (an IPv6 host is written in
    /// brackets and in its canonical form).
    pub url: String,
    /// The host only (for the consent dialog), without brackets for IPv6.
    pub host: String,
    /// The host is this machine: `localhost`, `127.0.0.0/8` or `::1`.
    pub local: bool,
}

/// Why an endpoint is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    /// Not an `http` or `https` URL with a host.
    #[error("the address must start with http:// or https:// and name a host")]
    NotHttp,
    /// Carries a user name or password.
    #[error("the address must not contain a user name or password")]
    Credentials,
    /// Has a path other than `/`, a query or a fragment.
    #[error("the address must not have a path, query or fragment")]
    Extra,
    /// Longer than [`MAX_ENDPOINT_CHARS`].
    #[error("the address is too long")]
    TooLong,
}

/// Parses and normalises an endpoint (`http://127.0.0.1:11434/` → `http://127.0.0.1:11434`).
///
/// Surrounding white space is ignored; the scheme and the host are lower-cased; the port, when
/// given, must be 1 to 65535 and is written without leading zeros; a trailing `/` is dropped. A
/// host is a DNS name (ASCII letters, digits, `-`, `_`, dot-separated), an IPv4 address or an IPv6
/// address in brackets. Internal white space, control characters and backslashes are refused.
///
/// # Errors
///
/// See [`EndpointError`].
pub fn parse(raw: &str) -> Result<Endpoint, EndpointError> {
    let input = raw.trim();
    if input.chars().count() > MAX_ENDPOINT_CHARS {
        return Err(EndpointError::TooLong);
    }
    if input
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(EndpointError::NotHttp);
    }
    let (scheme, rest) = input.split_once("://").ok_or(EndpointError::NotHttp)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(EndpointError::NotHttp);
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if authority.contains('@') {
        return Err(EndpointError::Credentials);
    }
    if !(tail.is_empty() || tail == "/") {
        return Err(EndpointError::Extra);
    }
    let (host, bracketed, port) = split_authority(authority)?;
    let mut url = format!("{scheme}://");
    if bracketed {
        url.push('[');
        url.push_str(&host);
        url.push(']');
    } else {
        url.push_str(&host);
    }
    if let Some(port) = port {
        url.push(':');
        url.push_str(&port.to_string());
    }
    if url.chars().count() > MAX_ENDPOINT_CHARS {
        return Err(EndpointError::TooLong);
    }
    let local = is_local_host(&host);
    Ok(Endpoint { url, host, local })
}

/// Whether a host (as in a URL, IPv6 with or without brackets) is this machine: `localhost`
/// (any case), an IPv4 address in `127.0.0.0/8` or the IPv6 address `::1`. Names that merely
/// start with `localhost` (`localhost.evil.com`) and every other address are not.
#[must_use]
pub fn is_local_host(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[') {
        // Brackets are only for IPv6.
        return inner
            .strip_suffix(']')
            .and_then(|address| address.parse::<Ipv6Addr>().ok())
            .is_some_and(|address| address == Ipv6Addr::LOCALHOST);
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Some([first, ..]) = parse_ipv4(host) {
        return first == 127;
    }
    host.parse::<Ipv6Addr>()
        .is_ok_and(|address| address == Ipv6Addr::LOCALHOST)
}

/// Splits `host[:port]` or `[v6][:port]` into the normalised host (no brackets), whether it was
/// bracketed, and the port.
fn split_authority(authority: &str) -> Result<(String, bool, Option<u16>), EndpointError> {
    if let Some(inner) = authority.strip_prefix('[') {
        let (inside, after) = inner.split_once(']').ok_or(EndpointError::NotHttp)?;
        let address: Ipv6Addr = inside.parse().map_err(|_| EndpointError::NotHttp)?;
        let port = if after.is_empty() {
            None
        } else {
            let digits = after.strip_prefix(':').ok_or(EndpointError::NotHttp)?;
            Some(parse_port(digits)?)
        };
        return Ok((address.to_string(), true, port));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, digits)) => (host, Some(parse_port(digits)?)),
        None => (authority, None),
    };
    if !is_valid_name(host) {
        return Err(EndpointError::NotHttp);
    }
    Ok((host.to_ascii_lowercase(), false, port))
}

/// A port: ASCII digits only, 1 to 65535.
fn parse_port(digits: &str) -> Result<u16, EndpointError> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(EndpointError::NotHttp);
    }
    match digits.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(EndpointError::NotHttp),
    }
}

/// A DNS-style host name (an IPv4 address is one too): ASCII letters, digits, `-` and `_`,
/// dot-separated, no empty label, no label starting or ending with `-`.
fn is_valid_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST_CHARS
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL_CHARS
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// A dotted-quad IPv4 address: exactly four parts of 1 to 3 digits, no leading zeros (`0127.0.0.1`
/// is octal for a resolver, so it is not read as `127.0.0.1` here), each at most 255.
fn parse_ipv4(host: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = host.split('.');
    for slot in &mut octets {
        let part = parts.next()?;
        if part.is_empty()
            || part.len() > 3
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(octets)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn ok(raw: &str) -> Endpoint {
        parse(raw).unwrap_or_else(|error| panic!("{raw:?} refused: {error}"))
    }

    #[test]
    fn accepted_endpoints_are_normalised() {
        // (input, url, host, local)
        let table = [
            (
                "http://127.0.0.1:11434",
                "http://127.0.0.1:11434",
                "127.0.0.1",
                true,
            ),
            (
                "http://127.0.0.1:11434/",
                "http://127.0.0.1:11434",
                "127.0.0.1",
                true,
            ),
            (
                "  http://127.0.0.1:11434/  ",
                "http://127.0.0.1:11434",
                "127.0.0.1",
                true,
            ),
            (
                "HTTP://LocalHost:11434",
                "http://localhost:11434",
                "localhost",
                true,
            ),
            ("http://localhost", "http://localhost", "localhost", true),
            ("http://localhost/", "http://localhost", "localhost", true),
            (
                "https://Ollama.Example.com",
                "https://ollama.example.com",
                "ollama.example.com",
                false,
            ),
            (
                "https://ollama.example.com:8443/",
                "https://ollama.example.com:8443",
                "ollama.example.com",
                false,
            ),
            (
                "http://127.1.2.3:80",
                "http://127.1.2.3:80",
                "127.1.2.3",
                true,
            ),
            (
                "http://127.255.255.255",
                "http://127.255.255.255",
                "127.255.255.255",
                true,
            ),
            ("http://[::1]:11434", "http://[::1]:11434", "::1", true),
            ("http://[::1]", "http://[::1]", "::1", true),
            ("http://[::1]/", "http://[::1]", "::1", true),
            ("http://[0:0:0:0:0:0:0:1]:9", "http://[::1]:9", "::1", true),
            (
                "http://[FE80::1]:11434",
                "http://[fe80::1]:11434",
                "fe80::1",
                false,
            ),
            (
                "http://[2001:0DB8:0:0:0:0:0:1]",
                "http://[2001:db8::1]",
                "2001:db8::1",
                false,
            ),
            (
                "http://192.168.1.5:11434",
                "http://192.168.1.5:11434",
                "192.168.1.5",
                false,
            ),
            ("http://10.0.0.2", "http://10.0.0.2", "10.0.0.2", false),
            (
                "http://0.0.0.0:11434",
                "http://0.0.0.0:11434",
                "0.0.0.0",
                false,
            ),
            (
                "http://localhost.evil.com:11434",
                "http://localhost.evil.com:11434",
                "localhost.evil.com",
                false,
            ),
            (
                "http://evil-localhost:11434",
                "http://evil-localhost:11434",
                "evil-localhost",
                false,
            ),
            (
                "http://my_box:11434",
                "http://my_box:11434",
                "my_box",
                false,
            ),
            (
                "http://127.0.0.1:011434",
                "http://127.0.0.1:11434",
                "127.0.0.1",
                true,
            ),
            (
                "http://127.0.0.1:65535",
                "http://127.0.0.1:65535",
                "127.0.0.1",
                true,
            ),
            (
                "http://127.0.0.1:1",
                "http://127.0.0.1:1",
                "127.0.0.1",
                true,
            ),
        ];
        for (raw, url, host, local) in table {
            let endpoint = ok(raw);
            assert_eq!(endpoint.url, url, "url of {raw:?}");
            assert_eq!(endpoint.host, host, "host of {raw:?}");
            assert_eq!(endpoint.local, local, "local of {raw:?}");
        }
    }

    #[test]
    fn refused_endpoints_name_the_reason() {
        let table = [
            ("", EndpointError::NotHttp),
            ("   ", EndpointError::NotHttp),
            ("localhost:11434", EndpointError::NotHttp),
            ("127.0.0.1", EndpointError::NotHttp),
            ("ftp://127.0.0.1", EndpointError::NotHttp),
            ("file:///etc/passwd", EndpointError::NotHttp),
            ("ws://127.0.0.1:11434", EndpointError::NotHttp),
            ("://127.0.0.1", EndpointError::NotHttp),
            ("http://", EndpointError::NotHttp),
            ("http:///", EndpointError::NotHttp),
            ("http://:11434", EndpointError::NotHttp),
            ("http://host:", EndpointError::NotHttp),
            ("http://host:0", EndpointError::NotHttp),
            ("http://host:65536", EndpointError::NotHttp),
            ("http://host:99999999999", EndpointError::NotHttp),
            ("http://host:+80", EndpointError::NotHttp),
            ("http://host:80a", EndpointError::NotHttp),
            ("http://host:8 0", EndpointError::NotHttp),
            ("http://ho st", EndpointError::NotHttp),
            ("http://host\t/x", EndpointError::NotHttp),
            ("http://ho\u{0}st", EndpointError::NotHttp),
            ("http://a..b", EndpointError::NotHttp),
            ("http://.a", EndpointError::NotHttp),
            ("http://a.", EndpointError::NotHttp),
            ("http://-a.com", EndpointError::NotHttp),
            ("http://a-.com", EndpointError::NotHttp),
            ("http://münchen.local", EndpointError::NotHttp),
            ("http://%31%32%37.0.0.1", EndpointError::NotHttp),
            ("http://127.0.0.1\\@evil.com", EndpointError::NotHttp),
            ("http://[::1", EndpointError::NotHttp),
            ("http://[::1]x", EndpointError::NotHttp),
            ("http://[::1]:", EndpointError::NotHttp),
            ("http://[::1%eth0]:11434", EndpointError::NotHttp),
            ("http://[localhost]", EndpointError::NotHttp),
            ("http://[1:2:3]", EndpointError::NotHttp),
            ("http://::1:11434", EndpointError::NotHttp),
            ("http://user@127.0.0.1", EndpointError::Credentials),
            ("http://user:pw@127.0.0.1:11434", EndpointError::Credentials),
            ("http://:pw@127.0.0.1", EndpointError::Credentials),
            ("http://127.0.0.1@evil.com", EndpointError::Credentials),
            ("http://user@", EndpointError::Credentials),
            ("http://127.0.0.1:11434/api", EndpointError::Extra),
            ("http://127.0.0.1:11434//", EndpointError::Extra),
            ("http://127.0.0.1:11434?x=1", EndpointError::Extra),
            ("http://127.0.0.1:11434/?x=1", EndpointError::Extra),
            ("http://127.0.0.1:11434#frag", EndpointError::Extra),
            ("http://127.0.0.1:11434/#", EndpointError::Extra),
            ("http://127.0.0.1?@evil.com", EndpointError::Extra),
            ("http://localhost/path@x", EndpointError::Extra),
        ];
        for (raw, error) in table {
            assert_eq!(parse(raw), Err(error), "{raw:?}");
        }
    }

    #[test]
    fn length_is_limited_to_512_characters() {
        let long_name = format!("http://{}.example.com", "a".repeat(600));
        assert_eq!(parse(&long_name), Err(EndpointError::TooLong));
        // 512 characters exactly: the limit itself is accepted when the rest is valid, but a
        // single label may not exceed 63 characters, so build it from several labels.
        let label = "a".repeat(60);
        let host = format!("{label}.{label}.{label}.{label}"); // 243 characters
        let url = format!("https://{host}");
        assert!(url.len() < MAX_ENDPOINT_CHARS);
        assert_eq!(parse(&url).unwrap().host, host);
        // A host over 253 characters is not a host.
        let too_long_host = format!("http://{label}.{label}.{label}.{label}.{label}");
        assert_eq!(parse(&too_long_host), Err(EndpointError::NotHttp));
    }

    #[test]
    fn local_hosts_are_localhost_loopback_v4_and_v6_only() {
        for host in [
            "localhost",
            "LOCALHOST",
            "LocalHost",
            "127.0.0.1",
            "127.1.2.3",
            "127.255.255.254",
            "::1",
            "[::1]",
            "0:0:0:0:0:0:0:1",
            "[0:0:0:0:0:0:0:1]",
        ] {
            assert!(is_local_host(host), "{host:?} should be local");
        }
        for host in [
            "",
            "localhost.evil.com",
            "evil.com",
            "foo.localhost",
            "localhost.",
            "localhost:11434",
            "192.168.1.5",
            "10.0.0.1",
            "0.0.0.0",
            "128.0.0.1",
            "126.255.255.255",
            "127.0.0.256",
            "127.0.0",
            "127.0.0.1.1",
            "127.0.0.1.nip.io",
            "0127.0.0.1",
            "127.000.000.001",
            "127.00.0.1",
            "2130706433",
            "0x7f000001",
            "[127.0.0.1]",
            "::",
            "::2",
            "[::1",
            "::1]",
            "::ffff:127.0.0.1",
            "fe80::1",
            "[fe80::1]",
        ] {
            assert!(!is_local_host(host), "{host:?} should be remote");
        }
    }

    #[test]
    fn the_same_endpoint_in_another_spelling_normalises_to_one_string() {
        let a = ok("HTTP://Example.COM:0011434/").url;
        let b = ok("http://example.com:11434").url;
        assert_eq!(a, b);
        let a = ok("http://[0:0:0:0:0:0:0:1]:11434").url;
        let b = ok("http://[::1]:11434/").url;
        assert_eq!(a, b);
    }

    #[test]
    fn a_leading_zero_octet_is_never_taken_for_loopback() {
        // A resolver reads 0127.0.0.1 as octal (87.0.0.1).
        let endpoint = ok("http://0127.0.0.1:11434");
        assert!(!endpoint.local);
        assert_eq!(endpoint.host, "0127.0.0.1");
    }
}
