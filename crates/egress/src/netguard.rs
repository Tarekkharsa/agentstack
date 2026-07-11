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
//! naming an address directly can't dodge it either.
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
    // Truly deny-by-exclusion for v6: the ONLY routable public space is global
    // unicast `2000::/3`, so allow only that and refuse everything else — that
    // covers ::1, ::, ff00::/8 multicast, fe80::/10 link-local, fc00::/7
    // unique-local, AND fec0::/10 deprecated site-local (which an earlier
    // enumerate-the-bad-ranges approach missed) in one stroke.
    let s = ip.segments();
    let global_unicast = (s[0] & 0xe000) == 0x2000; // 2000::/3
    let is_documentation = s[0] == 0x2001 && s[1] == 0x0db8; // 2001:db8::/32
    !global_unicast || is_documentation
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
}
