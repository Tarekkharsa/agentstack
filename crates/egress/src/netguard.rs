//! Address-class guard for resolved egress targets — the anti-SSRF layer.
//!
//! Policy ([`EgressGuard`](crate::decide::EgressGuard)) decides on the *name* a
//! client asks for. But a name the policy allows can still resolve to an
//! address that is not a legitimate egress target: the host's own loopback, the
//! private LAN behind the proxy, or the cloud metadata endpoint
//! (`169.254.169.254`). Because the sidecar proxy dials on the sandbox's behalf,
//! reaching those turns the proxy into an SSRF pivot into the host/internal
//! network — precisely what a locked-down sandbox must not be able to do.
//!
//! So after policy allows a host we resolve it and require every resolved
//! address to be **global unicast**: anything loopback / private / link-local /
//! unique-local / unspecified / multicast / reserved is refused. A literal-IP
//! CONNECT (e.g. `CONNECT 169.254.169.254:80`) flows through the same check, so
//! naming an address directly can't dodge it either. For IPv6 "global unicast"
//! is necessary but not sufficient — the special-purpose prefixes that sit
//! inside `2000::/3` are refused too, because 6to4 and Teredo embed an IPv4
//! address and would otherwise smuggle a forbidden v4 target past the v4 arm
//! (see [`is_forbidden_v6`]).
//!
//! Tests and the Docker demo legitimately target loopback / the host gateway
//! (`host.docker.internal`), so callers may opt into allowing local targets;
//! that opt-in is never the default and never set in production.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// True when `ip` is NOT a safe, globally-routable egress target and must be
/// refused. We deny-by-exclusion: only global-unicast addresses are allowed
/// through, so a range we forgot to name fails closed, not open.
///
/// (Rust's `IpAddr::is_global` is still unstable, so the classification is
/// spelled out here over raw octets/segments — all stable APIs, no new deps.)
pub fn is_forbidden_target(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_forbidden_v4(v4),
        // An IPv4-mapped/compatible v6 address (`::ffff:a.b.c.d`) is really the
        // v4 target wearing a v6 coat — unwrap and judge it as v4, else a
        // forbidden v4 could slip through the v6 path.
        IpAddr::V6(v6) => match v6.to_ipv4_mapped().or_else(|| to_ipv4_compatible(v6)) {
            Some(v4) => is_forbidden_v4(v4),
            None => is_forbidden_v6(v6),
        },
    }
}

fn is_forbidden_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    ip.is_loopback()            // 127.0.0.0/8
        || ip.is_private()      // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local()   // 169.254.0.0/16 — includes the metadata IP
        || ip.is_broadcast()    // 255.255.255.255
        || ip.is_documentation()// 192.0.2/24, 198.51.100/24, 203.0.113/24
        || ip.is_unspecified()  // 0.0.0.0
        || ip.is_multicast()    // 224.0.0.0/4
        || a == 0               // 0.0.0.0/8 "this network"
        || a >= 240             // 240.0.0.0/4 reserved (and 255.x)
        || (a == 100 && (64..=127).contains(&b)) // 100.64/10 carrier-grade NAT
        || (a == 198 && (18..=19).contains(&b))  // 198.18/15 benchmarking
        || (a == 192 && b == 0) // 192.0.0.0/24 IETF protocol
}

fn is_forbidden_v6(ip: Ipv6Addr) -> bool {
    // Deny-by-exclusion, then subtract: the only routable public space is
    // global unicast `2000::/3`, so refusing everything outside it covers ::1,
    // ::, ff00::/8 multicast, fe80::/10 link-local, fc00::/7 unique-local, AND
    // fec0::/10 deprecated site-local in one stroke.
    //
    // But `2000::/3` is NOT uniformly safe, so "global unicast" alone is not
    // the whole test. Several special-purpose prefixes live inside it, and two
    // of them — 6to4 and Teredo — *embed an IPv4 address*: `2002:a9fe:a9fe::`
    // is the cloud metadata IP (169.254.169.254) wearing a v6 coat, and a host
    // with a 6to4/Teredo tunnel will happily decapsulate it. Those would
    // otherwise sail through this check and hand back the SSRF pivot the v4
    // arm exists to deny.
    //
    // We refuse the transition prefixes OUTRIGHT rather than decoding the
    // embedded v4 and judging it as v4 (the way `to_ipv4_mapped` does): 6to4
    // and Teredo are obsolete transition technologies that no legitimate MCP
    // server needs, so failing closed costs nothing real — and it avoids
    // trusting our own decoder of a hostile address, which unwrap-and-judge
    // would require.
    let s = ip.segments();
    let global_unicast = (s[0] & 0xe000) == 0x2000; // 2000::/3
    if !global_unicast {
        return true;
    }
    // Special-purpose prefixes *inside* `2000::/3` (IANA IPv6 Special-Purpose
    // Address Registry) that are never a legitimate egress target.
    //
    // 2002::/16 — 6to4, RFC 3056. Segments 1-2 ARE an embedded IPv4 address.
    let is_6to4 = s[0] == 0x2002;
    // 2001::/32 — Teredo, RFC 4380. The client IPv4 is the last 32 bits XOR
    // 0xffffffff, so this prefix embeds a v4 address too.
    let is_teredo = s[0] == 0x2001 && s[1] == 0x0000;
    // 2001:db8::/32 — documentation, RFC 3849.
    let is_documentation = s[0] == 0x2001 && s[1] == 0x0db8;
    // 2001:2::/48 — benchmarking, RFC 5180.
    let is_benchmarking = s[0] == 0x2001 && s[1] == 0x0002 && s[2] == 0x0000;
    // 2001:10::/28 (ORCHID, RFC 4843, deprecated) and 2001:20::/28 (ORCHIDv2,
    // RFC 7343) — cryptographic identifiers, non-routable by construction. A
    // /28 is the 16 bits of s[0] plus the top 12 bits of s[1], hence 0xfff0.
    // (`matches!(x, a | b)` is an or-pattern match — Rust for "x is one of".)
    let is_orchid = s[0] == 0x2001 && matches!(s[1] & 0xfff0, 0x0010 | 0x0020);

    is_6to4 || is_teredo || is_documentation || is_benchmarking || is_orchid
}

/// `::a.b.c.d` (deprecated IPv4-compatible form), which `to_ipv4_mapped` does
/// not cover. Excludes `::` and `::1`, which are handled as native v6.
fn to_ipv4_compatible(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    if s[0..6].iter().all(|&x| x == 0) && !(s[6] == 0 && (s[7] == 0 || s[7] == 1)) {
        Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().unwrap())
    }

    #[test]
    fn blocks_loopback_private_and_metadata() {
        for s in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.3.4",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata — the classic SSRF target
            "0.0.0.0",
            "100.100.0.1", // CGN
            "224.0.0.1",   // multicast
            "240.0.0.1",   // reserved
        ] {
            assert!(is_forbidden_target(v4(s)), "{s} must be forbidden");
        }
    }

    #[test]
    fn blocks_forbidden_v6() {
        for s in [
            "::1", "::", "fe80::1", "fc00::1", "fd12::9", "ff02::1",
            "fec0::1", // deprecated site-local — must NOT read as global
        ] {
            assert!(is_forbidden_target(v6(s)), "{s} must be forbidden");
        }
        // IPv4-mapped loopback must not sneak past the v6 arm.
        assert!(is_forbidden_target(v6("::ffff:127.0.0.1")));
        assert!(is_forbidden_target(v6("::ffff:169.254.169.254")));
    }

    #[test]
    fn allows_global_unicast() {
        for s in ["1.1.1.1", "8.8.8.8", "93.184.216.34"] {
            assert!(!is_forbidden_target(v4(s)), "{s} must be allowed");
        }
        assert!(!is_forbidden_target(v6("2606:4700:4700::1111"))); // public v6
    }

    /// Global unicast (`2000::/3`) is necessary but NOT sufficient: the
    /// special-purpose prefixes inside it are refused, and the two that embed
    /// an IPv4 address are the ones that matter — they smuggle a forbidden v4
    /// target (the metadata IP) past the v4 arm.
    #[test]
    fn blocks_special_purpose_inside_global_unicast() {
        for s in [
            // 6to4 (2002::/16) — segments 1-2 are the embedded v4. This exact
            // address is 169.254.169.254, the cloud metadata endpoint.
            "2002:a9fe:a9fe::",
            "2002:7f00:1::", // 6to4 for 127.0.0.1
            // Teredo (2001:0::/32) — the client v4 is the last 32 bits XOR
            // 0xffffffff, so 5601:5601 decodes to 169.254.169.254.
            "2001:0:4136:e378:8000:63bf:5601:5601",
            "2001::1",             // anything in the Teredo prefix
            "2001:2::1",           // 2001:2::/48 benchmarking (RFC 5180)
            "2001:10::1",          // ORCHID, deprecated (RFC 4843)
            "2001:1f:ffff:ffff::", // last address of the 2001:10::/28 block
            "2001:20::1",          // ORCHIDv2 (RFC 7343)
            "2001:2f::1",          // inside 2001:20::/28
            "2001:db8::1",         // documentation (RFC 3849)
        ] {
            assert!(is_forbidden_target(v6(s)), "{s} must be forbidden");
        }
    }

    /// The carve-outs must not over-reach: ordinary public v6 that merely
    /// starts with 0x2001 or sits near the special prefixes stays allowed.
    #[test]
    fn special_purpose_carve_outs_do_not_overreach() {
        for s in [
            "2600::1",                  // plain global unicast
            "2001:4860:4860::8888",     // Google DNS — starts 0x2001, not special
            "2001:1::1",                // 2001:1::/32 is NOT one of the blocked prefixes
            "2001:3::1",                // adjacent to the 2001:2::/48 benchmarking block
            "2001:30::1",               // just past ORCHIDv2's 2001:20::/28
            "2003::1",                  // adjacent to 6to4's 2002::/16
            "2606:4700:4700::1111",     // Cloudflare DNS
            "2a00:1450:4001:81b::200e", // a real routable address
        ] {
            assert!(!is_forbidden_target(v6(s)), "{s} must be allowed");
        }
    }
}
