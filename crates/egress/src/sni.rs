//! Extract the SNI hostname from a TLS ClientHello.
//!
//! HTTPS host filtering keys off the Server Name Indication the client sends
//! in the clear at the start of the handshake — so the proxy allowlists by the
//! name the client *asked for*, with no TLS interception. The ClientHello
//! arrives from inside an untrusted container, so this parser is fully
//! bounds-checked: every field length is validated against what remains, and
//! any truncation, overrun, or unexpected tag yields `SniVerdict::Unverifiable`
//! (fail closed) rather than a panic — never a silent `Absent`.
//!
//! Wire layout walked here (TLS 1.2/1.3 ClientHello):
//! record[type=0x16, version, len] → handshake[type=0x01, len24] →
//! client_version, random[32], session_id, cipher_suites, compression_methods,
//! extensions → extension[type=0x0000 (server_name)] →
//! server_name_list → entry[type=0x00 (host_name), name].

/// A minimal, panic-free big-endian cursor over a byte slice.
struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b }
    }

    fn u8(&mut self) -> Option<u8> {
        let (&first, rest) = self.b.split_first()?;
        self.b = rest;
        Some(first)
    }

    fn u16(&mut self) -> Option<usize> {
        let hi = self.u8()? as usize;
        let lo = self.u8()? as usize;
        Some((hi << 8) | lo)
    }

    fn u24(&mut self) -> Option<usize> {
        let a = self.u8()? as usize;
        let b = self.u16()?;
        Some((a << 16) | b)
    }

    /// Take exactly `n` bytes, or `None` if fewer remain.
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.b.len() {
            return None;
        }
        let (head, rest) = self.b.split_at(n);
        self.b = rest;
        Some(head)
    }

    fn is_empty(&self) -> bool {
        self.b.is_empty()
    }
}

/// The result of inspecting a first TLS flight for its SNI. The proxy allowlists
/// by the name the client *asked for*, so it must tell a hello that PROVABLY
/// carries no SNI apart from one it simply could not parse: a truncated or
/// record-fragmented ClientHello that "has no SNI" is exactly how a client hides
/// a mismatched SNI past the check (send record 1 with a short, SNI-less prefix;
/// smuggle the real SNI in record 2). The two cases must diverge — allow vs.
/// fail closed.
#[derive(Debug, PartialEq, Eq)]
pub enum SniVerdict {
    /// A complete, well-formed ClientHello carrying this host_name SNI.
    Present(String),
    /// A complete, well-formed ClientHello that provably carries NO server_name
    /// extension (the handshake body was fully present and walked to its end).
    Absent,
    /// The bytes are not a parseable, self-contained ClientHello: not a
    /// handshake record, not a ClientHello, or a handshake that runs past the
    /// bytes available (truncated, or fragmented across TLS records — this
    /// parser only sees the first record). The caller MUST fail closed.
    Unverifiable,
}

/// Inspect a TLS ClientHello record for its first host_name SNI. Returns a
/// three-way [`SniVerdict`]: an unparseable or fragmented hello is
/// `Unverifiable` (fail closed), NOT silently `Absent` — a hello whose handshake
/// spans beyond this single record must never be waved through as "no SNI".
pub fn extract_sni(record: &[u8]) -> SniVerdict {
    match parse_sni(record) {
        Some(verdict) => verdict,
        // Any bounds check that failed (`?`) means a length claimed more bytes
        // than were present: truncated or record-fragmented. Fail closed.
        None => SniVerdict::Unverifiable,
    }
}

/// Walk the ClientHello. `None` from any `?` = a truncated/fragmented structure
/// (→ `Unverifiable`). A clean walk to the end with no server_name = `Absent`.
fn parse_sni(record: &[u8]) -> Option<SniVerdict> {
    let mut r = Reader::new(record);

    // Record layer: must be a Handshake record (0x16).
    if r.u8()? != 0x16 {
        return Some(SniVerdict::Unverifiable);
    }
    let _record_version = r.u16()?;
    let record_len = r.u16()?;
    // `take(record_len)` fails if the handshake body extends beyond THIS record
    // (fragmented across records) — the crux of the fix: that is Unverifiable,
    // not Absent.
    let handshake = r.take(record_len)?;

    // Handshake: must be a ClientHello (0x01).
    let mut h = Reader::new(handshake);
    if h.u8()? != 0x01 {
        return Some(SniVerdict::Unverifiable);
    }
    let body_len = h.u24()?;
    let body = h.take(body_len)?;

    let mut c = Reader::new(body);
    c.take(2)?; // client_version
    c.take(32)?; // random
    let sid_len = c.u8()? as usize;
    c.take(sid_len)?; // session_id
    let cs_len = c.u16()?;
    c.take(cs_len)?; // cipher_suites
    let comp_len = c.u8()? as usize;
    c.take(comp_len)?; // compression_methods

    // Extensions block absent entirely (ancient ClientHello, body ends here) →
    // provably no SNI. A PRESENT-but-truncated extensions length is a `?` fail
    // above/below → Unverifiable.
    let ext_total = match c.u16() {
        Some(n) => n,
        None => return Some(SniVerdict::Absent),
    };
    let extensions = c.take(ext_total)?;

    let mut e = Reader::new(extensions);
    while !e.is_empty() {
        let ext_type = e.u16()?;
        let ext_len = e.u16()?;
        let ext_data = e.take(ext_len)?;
        if ext_type == 0x0000 {
            // A server_name extension that won't parse is malformed, not
            // "absent" — fail closed rather than allow.
            return Some(match parse_server_name(ext_data) {
                Some(name) => SniVerdict::Present(name),
                None => SniVerdict::Unverifiable,
            });
        }
    }
    // Walked every extension cleanly; none was server_name.
    Some(SniVerdict::Absent)
}

/// Parse a server_name extension body, returning the first host_name.
fn parse_server_name(data: &[u8]) -> Option<String> {
    let mut r = Reader::new(data);
    let list_len = r.u16()?;
    let list = r.take(list_len)?;
    let mut l = Reader::new(list);
    // Walk entries; return the first host_name (type 0x00).
    while !l.is_empty() {
        let name_type = l.u8()?;
        let name_len = l.u16()?;
        let name = l.take(name_len)?;
        if name_type == 0x00 {
            return std::str::from_utf8(name).ok().map(str::to_string);
        }
    }
    None
}

/// Build a minimal but structurally valid ClientHello record carrying one
/// host_name SNI — doubles as documentation of the wire format the parser
/// walks. Crate-visible under test so the proxy's SNI-wiring tests can reuse it.
#[cfg(test)]
pub(crate) fn client_hello_with_sni(name: &str) -> Vec<u8> {
    let name = name.as_bytes();

    // server_name extension body.
    let mut entry = Vec::new();
    entry.push(0x00); // name_type = host_name
    entry.extend_from_slice(&(name.len() as u16).to_be_bytes());
    entry.extend_from_slice(name);
    let mut sni_body = Vec::new();
    sni_body.extend_from_slice(&(entry.len() as u16).to_be_bytes()); // list len
    sni_body.extend_from_slice(&entry);

    // extension = type(0x0000) + len + body.
    let mut ext = Vec::new();
    ext.extend_from_slice(&0x0000u16.to_be_bytes());
    ext.extend_from_slice(&(sni_body.len() as u16).to_be_bytes());
    ext.extend_from_slice(&sni_body);

    // ClientHello body.
    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]); // client_version TLS 1.2
    body.extend_from_slice(&[0u8; 32]); // random
    body.push(0x00); // session_id len 0
    body.extend_from_slice(&0x0002u16.to_be_bytes()); // cipher_suites len
    body.extend_from_slice(&[0x13, 0x01]); // one cipher suite
    body.push(0x01); // compression_methods len
    body.push(0x00); // null compression
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes()); // extensions len
    body.extend_from_slice(&ext);

    // Handshake header.
    let mut hs = Vec::new();
    hs.push(0x01); // ClientHello
    let bl = body.len();
    hs.extend_from_slice(&[(bl >> 16) as u8, (bl >> 8) as u8, bl as u8]);
    hs.extend_from_slice(&body);

    // Record header.
    let mut rec = Vec::new();
    rec.push(0x16); // Handshake
    rec.extend_from_slice(&[0x03, 0x01]); // legacy record version
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_sni_hostname() {
        let hello = client_hello_with_sni("api.search.example");
        assert_eq!(
            extract_sni(&hello),
            SniVerdict::Present("api.search.example".into())
        );
    }

    #[test]
    fn truncation_anywhere_is_unverifiable_never_panics_never_absent() {
        let hello = client_hello_with_sni("example.com");
        // Every strict prefix of a valid ClientHello is truncated, so it must be
        // Unverifiable (fail closed) — NEVER Absent (which the caller allows) and
        // never a panic. This is the anti-fronting guarantee: a partial hello
        // must not read as "no SNI".
        for cut in 0..hello.len() {
            assert_eq!(
                extract_sni(&hello[..cut]),
                SniVerdict::Unverifiable,
                "prefix len {cut} must be Unverifiable, not Absent/Present"
            );
        }
        assert_eq!(
            extract_sni(&hello),
            SniVerdict::Present("example.com".into())
        );
    }

    #[test]
    fn record_fragmented_clienthello_is_unverifiable_not_absent() {
        // A ClientHello whose handshake body_len claims MORE than the first TLS
        // record carries — the SNI would live in a second record this parser
        // never sees. It must be Unverifiable (fail closed), not Absent. This is
        // the exact domain-fronting-via-fragmentation bypass.
        let full = client_hello_with_sni("evil.example");
        // `full` is one record: [0x16, ver(2), rec_len(2), handshake...]. Re-frame
        // it so the record header advertises only the first half of the
        // handshake, leaving the rest (with the SNI) "in a later record".
        let handshake = &full[5..];
        let short_rec_len = (handshake.len() / 2) as u16;
        let mut rec = vec![0x16, 0x03, 0x01];
        rec.extend_from_slice(&short_rec_len.to_be_bytes());
        rec.extend_from_slice(handshake); // more bytes than short_rec_len covers
                                          // The record layer take(short_rec_len) yields a truncated handshake whose
                                          // declared body_len overruns it → Unverifiable.
        assert_eq!(extract_sni(&rec), SniVerdict::Unverifiable);
    }

    #[test]
    fn non_clienthello_and_junk_are_unverifiable() {
        assert_eq!(extract_sni(&[]), SniVerdict::Unverifiable);
        // A complete non-handshake record (0x17) is not a ClientHello — the
        // proxy must fail closed, not treat it as SNI-less.
        assert_eq!(
            extract_sni(&[0x17, 0x03, 0x03, 0x00, 0x00]),
            SniVerdict::Unverifiable
        );
        assert_eq!(extract_sni(&[0xff; 64]), SniVerdict::Unverifiable);
    }

    #[test]
    fn complete_clienthello_without_sni_is_absent() {
        // A COMPLETE ClientHello with an empty extensions block provably carries
        // no SNI — the one case the caller may legitimately allow.
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend_from_slice(&[0u8; 32]);
        body.push(0x00);
        body.extend_from_slice(&0x0002u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(0x01);
        body.push(0x00);
        body.extend_from_slice(&0u16.to_be_bytes()); // extensions block, length 0
        let mut hs = vec![0x01];
        let bl = body.len();
        hs.extend_from_slice(&[(bl >> 16) as u8, (bl >> 8) as u8, bl as u8]);
        hs.extend_from_slice(&body);
        let mut rec = vec![0x16, 0x03, 0x01];
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        assert_eq!(extract_sni(&rec), SniVerdict::Absent);
    }
}
