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
    // Reject port 0: it never names a real CONNECT target (and won't connect),
    // and policy patterns can't pin it — so refusing it here keeps the parser
    // and the `host:port` policy grammar consistent.
    if port == 0 {
        return None;
    }
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
///
/// Delegates to the single shared implementation in `core` so the CONNECT
/// parser and the CLI's gateway-only classifier can never diverge — same
/// input, same canonical host, or the fence has a seam.
pub(crate) fn normalize_host(host: &str) -> String {
    agentstack_core::manifest::normalize_host(host)
}

/// Does this CONNECT host string denote a numeric IP address in ANY form a
/// permissive resolver (`getaddrinfo`/`inet_aton`) would accept? Under lockdown
/// every legitimate target is a DNS name, so a numeric host is a bypass of the
/// hostname-based gateway-only fence — and Rust's `IpAddr::parse` alone is not
/// enough, because it accepts only canonical dotted-decimal (`93.184.216.34`)
/// and colon-hex (`::1`) forms while `inet_aton` also accepts:
///   - single-integer IPv4:  `2130706433`      (= 127.0.0.1)
///   - hexadecimal labels:   `0x7f.0x0.0x0.0x1`, `0x7f000001`
///   - octal / leading-zero: `0177.0.0.01`
///   - shortened (1–4 parts): `127.1`, `10.0.258`
///
/// The detection is purely LEXICAL and fail-closed: we never resolve DNS to
/// decide (that would open a rebinding/TOCTOU window). A host is numeric iff it
/// parses as a canonical `IpAddr`, OR it is 1–4 dot-separated parts that are
/// EACH a decimal, octal, or hex integer literal. A single non-numeric label
/// (any real hostname has one) makes the whole host a name, not an address.
pub(crate) fn is_numeric_host(host: &str) -> bool {
    // Defense in depth: a trailing root dot is stripped by `normalize_host`
    // before this is reached, but strip it here too so a numeric literal can
    // never slip in by that route.
    let host = host.strip_suffix('.').unwrap_or(host);
    // A `%zone` suffix (scoped IPv6 like `fe80::1%eth0`) is stripped before the
    // parse — Rust's `IpAddr` rejects the zone form but a permissive resolver
    // accepts it, so the bare address underneath is what matters.
    let host = host.split('%').next().unwrap_or(host);
    // A colon means an IPv6 literal: the CONNECT parser already split the port
    // off (`[addr]:port` → `addr`), so any remaining `:` is address syntax, not
    // a port — and never a DNS name. Refuse it whether or not `IpAddr` can parse
    // this exact spelling (e.g. a zone-scoped or otherwise non-canonical form a
    // resolver would still accept).
    if host.contains(':') {
        return true;
    }
    // Canonical dotted-decimal IPv4 and any IPv6 (incl. `::1`, mapped forms).
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    // inet_aton accepts 1..=4 dot-separated numeric parts (each decimal, octal,
    // or hex). More than 4 parts, or any empty/non-numeric part, is not an IP.
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return false;
    }
    parts.iter().all(|p| is_numeric_label(p))
}

/// One `inet_aton` component: a hex (`0x…`), or a decimal/octal run of digits.
/// A leading-zero octal (`0177`) and a plain decimal are both just "all ASCII
/// digits" here — we don't validate the octal range, because a label a resolver
/// would REJECT as an IP is one we can safely treat as numeric-and-refused
/// under lockdown anyway (fail closed, never fail open).
fn is_numeric_label(label: &str) -> bool {
    if let Some(hex) = label
        .strip_prefix("0x")
        .or_else(|| label.strip_prefix("0X"))
    {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit())
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
    fn is_numeric_host_catches_every_inet_aton_encoding() {
        // Canonical dotted-decimal and IPv6 (what IpAddr::parse already caught).
        assert!(is_numeric_host("93.184.216.34"));
        assert!(is_numeric_host("127.0.0.1"));
        assert!(is_numeric_host("::1"));
        assert!(is_numeric_host("2001:db8::1"));
        // Single-integer IPv4 (= 127.0.0.1) — IpAddr::parse REJECTS this, the
        // resolver accepts it. This is the core bypass the lexical guard closes.
        assert!(is_numeric_host("2130706433"));
        // Hexadecimal, dotted and single-value.
        assert!(is_numeric_host("0x7f.0x0.0x0.0x1"));
        assert!(is_numeric_host("0x7f000001"));
        assert!(is_numeric_host("0X7F000001"));
        // Octal / leading-zero labels.
        assert!(is_numeric_host("0177.0.0.01"));
        // Mixed base across labels.
        assert!(is_numeric_host("0x7f.1"));
        assert!(is_numeric_host("0177.0x0.0.1"));
        // Shortened (1–4 part) forms.
        assert!(is_numeric_host("127.1"));
        assert!(is_numeric_host("10.0.258"));
        // Trailing root dot on a numeric literal must not slip past.
        assert!(is_numeric_host("2130706433."));
        assert!(is_numeric_host("127.0.0.1."));
        // Scoped / zone-id IPv6 spellings that Rust's IpAddr rejects but a
        // permissive resolver accepts — a colon (or a `%zone` over a colon
        // address) is IPv6 syntax, never a DNS name.
        assert!(is_numeric_host("fe80::1%eth0"));
        assert!(is_numeric_host("2001:db8::1%eth0"));
        assert!(is_numeric_host("2001:db8::1"));
        assert!(is_numeric_host("::ffff:127.0.0.1"));

        // Real hostnames are NOT numeric — a single non-numeric label is enough.
        assert!(!is_numeric_host("mcp.example.com"));
        assert!(!is_numeric_host("api.example.com"));
        assert!(!is_numeric_host("0x7f.example.com"));
        assert!(!is_numeric_host("deadbeef")); // hex-looking but no 0x prefix
        assert!(!is_numeric_host("3com.example")); // digit-led label, has letters
                                                   // More than four numeric parts is not an inet_aton address.
        assert!(!is_numeric_host("1.2.3.4.5"));
        assert!(!is_numeric_host(""));
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
        // Port 0 is refused (consistent with the policy grammar, which can't
        // pin port 0).
        assert_eq!(parse_connect_target(b"CONNECT h:0 HTTP/1.1"), None);
        // Empty host.
        assert_eq!(parse_connect_target(b"CONNECT :443 HTTP/1.1"), None);
        // Garbage / empty.
        assert_eq!(parse_connect_target(b""), None);
        assert_eq!(parse_connect_target(&[0xff, 0xfe, 0x00]), None);
    }
}
