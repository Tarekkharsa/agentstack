//! Parse the target of an HTTP `CONNECT` request — the tunnel a client opens
//! before speaking TLS. The request line is `CONNECT host:port HTTP/1.1`.

/// A parsed CONNECT target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

/// Parse the target host:port from a CONNECT request line (the first line, with
/// or without a trailing CRLF). Returns `None` for anything that isn't a
/// well-formed CONNECT line — the caller rejects the tunnel rather than
/// guessing. Hostile input: bounded, no panics, no allocation beyond the host
/// string.
pub fn parse_connect_target(line: &[u8]) -> Option<Target> {
    // Work on the first line only, trimming any CR/LF.
    let end = line
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(line.len());
    let line = std::str::from_utf8(&line[..end]).ok()?;

    let mut parts = line.split(' ').filter(|s| !s.is_empty());
    if !parts.next()?.eq_ignore_ascii_case("CONNECT") {
        return None;
    }
    let authority = parts.next()?;
    // Method + authority + version; reject a missing or junk version so a bare
    // "CONNECT host:port" (no HTTP version) doesn't slip through as valid.
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }

    split_host_port(authority)
}

/// Split `host:port`, supporting bracketed IPv6 literals (`[::1]:443`). The
/// port must be present and numeric — CONNECT authorities always carry one.
/// The host is normalized (see [`normalize_host`]) so casing / a trailing dot
/// can't be used to slip past a case-sensitive policy match.
fn split_host_port(authority: &str) -> Option<Target> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal: `[addr]:port`.
        let (addr, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':')?;
        (addr.to_string(), port)
    } else {
        let (host, port) = authority.rsplit_once(':')?;
        if host.is_empty() {
            return None;
        }
        (host.to_string(), port)
    };
    let port: u16 = port.parse().ok()?;
    let host = normalize_host(&host);
    if host.is_empty() {
        return None;
    }
    Some(Target { host, port })
}

/// Canonicalize a hostname for policy matching: strip a single trailing `.`
/// (the DNS root label — `evil.example.` and `evil.example` are the same host)
/// and ASCII-lowercase it (DNS is case-insensitive). Without this,
/// `EVIL.EXAMPLE` or `evil.example.` would evade a `!evil.example` deny rule,
/// since the policy glob match is exact and case-sensitive.
pub(crate) fn normalize_host(host: &str) -> String {
    host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(host: &str, port: u16) -> Option<Target> {
        Some(Target {
            host: host.into(),
            port,
        })
    }

    #[test]
    fn parses_a_normal_connect_line() {
        assert_eq!(
            parse_connect_target(b"CONNECT api.example.com:443 HTTP/1.1\r\n"),
            t("api.example.com", 443)
        );
        // No trailing CRLF, lowercase method, extra spaces all tolerated.
        assert_eq!(
            parse_connect_target(b"connect  example.org:8443  HTTP/1.1"),
            t("example.org", 8443)
        );
    }

    #[test]
    fn normalizes_host_case_and_trailing_dot() {
        // Casing and a trailing root dot must not create a distinct host that
        // dodges a case-sensitive `!evil.example` deny rule.
        assert_eq!(
            parse_connect_target(b"CONNECT EVIL.EXAMPLE:443 HTTP/1.1\r\n"),
            t("evil.example", 443)
        );
        assert_eq!(
            parse_connect_target(b"CONNECT evil.example.:443 HTTP/1.1\r\n"),
            t("evil.example", 443)
        );
        assert_eq!(
            parse_connect_target(b"CONNECT Api.Example.Com.:8443 HTTP/1.1"),
            t("api.example.com", 8443)
        );
    }

    #[test]
    fn parses_ipv6_authority() {
        assert_eq!(
            parse_connect_target(b"CONNECT [2001:db8::1]:443 HTTP/1.1\r\n"),
            t("2001:db8::1", 443)
        );
    }

    #[test]
    fn rejects_malformed_or_non_connect() {
        // Not CONNECT.
        assert_eq!(parse_connect_target(b"GET / HTTP/1.1\r\n"), None);
        // Missing port.
        assert_eq!(
            parse_connect_target(b"CONNECT example.com HTTP/1.1\r\n"),
            None
        );
        // Missing HTTP version — don't accept a bare authority.
        assert_eq!(parse_connect_target(b"CONNECT example.com:443"), None);
        // Non-numeric / out-of-range port.
        assert_eq!(parse_connect_target(b"CONNECT h:notaport HTTP/1.1"), None);
        assert_eq!(parse_connect_target(b"CONNECT h:99999 HTTP/1.1"), None);
        // Empty host.
        assert_eq!(parse_connect_target(b"CONNECT :443 HTTP/1.1"), None);
        // Garbage / empty.
        assert_eq!(parse_connect_target(b""), None);
        assert_eq!(parse_connect_target(&[0xff, 0xfe, 0x00]), None);
    }
}
