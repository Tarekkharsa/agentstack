//! Fixed-destination TCP relay for the lockdown gateway bridge (gateway
//! unification Session 3).
//!
//! In `--lockdown` the sandbox container sits on an internal-only Docker
//! network and cannot reach the host-side gateway directly. This relay — a
//! peer on that internal network, running inside the egress sidecar — accepts a
//! connection and splices it byte-for-byte to ONE preconfigured destination
//! (the host gateway, reached over the sidecar's egress leg).
//!
//! It is deliberately **not** the egress proxy, and must never become it:
//! - it parses nothing (no CONNECT, no SNI, no MCP) — a raw byte pipe;
//! - it consults no policy and does NO SSRF / address-class / DNS validation.
//!   The destination is a fixed address the trusted host CLI supplied, not a
//!   client-chosen target, so the anti-SSRF guard the real proxy enforces
//!   (`netguard`) does not apply here and its opt-out is NOT reused — real
//!   egress keeps that guard on.
//!
//! It only widens *reachability*, never *trust*: authentication stays
//! end-to-end — the host gateway checks the per-run bearer token on every
//! request. A container that reaches the relay without the token reaches a
//! gateway that refuses it.

use std::io;
use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};

/// Bind a relay on `listen` that splices every accepted connection to `dest`
/// (e.g. `host.docker.internal:12345`). Returns the bound local address; the
/// accept loop runs on a spawned task for the process's lifetime (teardown is
/// the container being removed, like the proxy listeners).
pub async fn start_relay(listen: SocketAddr, dest: String) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        // Loop until the listener socket dies; each connection is spliced on
        // its own task so one slow client can't block new accepts.
        while let Ok((inbound, _)) = listener.accept().await {
            let dest = dest.clone();
            tokio::spawn(async move {
                // A failed dial or a client that hangs up just drops this
                // connection — the relay keeps serving.
                let _ = splice(inbound, &dest).await;
            });
        }
    });
    Ok(addr)
}

/// Connect to `dest` and shuttle bytes both ways until either side closes.
async fn splice(mut inbound: TcpStream, dest: &str) -> io::Result<()> {
    let mut outbound = TcpStream::connect(dest).await?;
    tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The relay is a transparent pipe: bytes a client sends arrive at the
    /// destination and the destination's reply comes back — no parsing, no
    /// policy, no address checks in the path.
    #[tokio::test]
    async fn relay_splices_bytes_to_the_fixed_destination() {
        // A destination that upper-cases whatever it receives.
        let dest = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dest_addr = dest.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = dest.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).await.unwrap();
            let up = buf[..n].to_ascii_uppercase();
            s.write_all(&up).await.unwrap();
        });

        let relay_addr = start_relay("127.0.0.1:0".parse().unwrap(), dest_addr.to_string())
            .await
            .unwrap();

        let mut c = TcpStream::connect(relay_addr).await.unwrap();
        c.write_all(b"ping through the relay").await.unwrap();
        let mut buf = [0u8; 64];
        let n = c.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"PING THROUGH THE RELAY");
    }
}
