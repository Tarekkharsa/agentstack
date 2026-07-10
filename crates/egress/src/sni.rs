//! Extract the SNI hostname from a TLS ClientHello.
//!
//! HTTPS host filtering keys off the Server Name Indication the client sends
//! in the clear at the start of the handshake — so the proxy allowlists by the
//! name the client *asked for*, with no TLS interception. The ClientHello
//! arrives from inside an untrusted container, so this parser is fully
//! bounds-checked: every field length is validated against what remains, and
//! any truncation, overrun, or unexpected tag yields `None` rather than a
//! panic.
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

/// Extract the first host_name SNI from a TLS ClientHello record. `None` when
/// the bytes are not a ClientHello, are truncated, or carry no SNI.
pub fn extract_sni(record: &[u8]) -> Option<String> {
    let mut r = Reader::new(record);

    // Record layer: must be a Handshake record (0x16).
    if r.u8()? != 0x16 {
        return None;
    }
    let _record_version = r.u16()?;
    let record_len = r.u16()?;
    let handshake = r.take(record_len)?;

    // Handshake: must be a ClientHello (0x01).
    let mut h = Reader::new(handshake);
    if h.u8()? != 0x01 {
        return None;
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

    // Extensions (absent in ancient ClientHellos → no SNI).
    let ext_total = c.u16()?;
    let extensions = c.take(ext_total)?;

    let mut e = Reader::new(extensions);
    while !e.is_empty() {
        let ext_type = e.u16()?;
        let ext_len = e.u16()?;
        let ext_data = e.take(ext_len)?;
        if ext_type == 0x0000 {
            return parse_server_name(ext_data);
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but structurally valid ClientHello record carrying one
    /// host_name SNI — doubles as documentation of the wire format the parser
    /// walks.
    fn client_hello_with_sni(name: &str) -> Vec<u8> {
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

    #[test]
    fn extracts_the_sni_hostname() {
        let hello = client_hello_with_sni("api.search.example");
        assert_eq!(extract_sni(&hello).as_deref(), Some("api.search.example"));
    }

    #[test]
    fn truncation_anywhere_yields_none_never_panics() {
        let hello = client_hello_with_sni("example.com");
        // Every prefix of a valid ClientHello must parse to None (or the full
        // value only at full length), never panic.
        for cut in 0..hello.len() {
            let _ = extract_sni(&hello[..cut]);
        }
        assert_eq!(extract_sni(&hello).as_deref(), Some("example.com"));
    }

    #[test]
    fn non_clienthello_and_junk_yield_none() {
        assert_eq!(extract_sni(&[]), None);
        assert_eq!(extract_sni(&[0x17, 0x03, 0x03, 0x00, 0x00]), None); // not handshake
        assert_eq!(extract_sni(&[0xff; 64]), None);
    }

    #[test]
    fn clienthello_without_sni_yields_none() {
        // A ClientHello with an empty extensions block.
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend_from_slice(&[0u8; 32]);
        body.push(0x00);
        body.extend_from_slice(&0x0002u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(0x01);
        body.push(0x00);
        body.extend_from_slice(&0u16.to_be_bytes()); // no extensions
        let mut hs = vec![0x01];
        let bl = body.len();
        hs.extend_from_slice(&[(bl >> 16) as u8, (bl >> 8) as u8, bl as u8]);
        hs.extend_from_slice(&body);
        let mut rec = vec![0x16, 0x03, 0x01];
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        assert_eq!(extract_sni(&rec), None);
    }
}
