//! Preserve the rejected TLS 1.2 flight without invalid rustls values.
use super::Observation;
use rustls::{ServerConfig, ServerConnection};
use std::{
    io::{self, Write},
    net::TcpListener,
    sync::Arc,
};
use unityrs_testkit::guard::DEADLOCK_GUARD;

pub fn peer(listener: &TcpListener, config: ServerConfig) -> io::Result<Observation> {
    let (mut socket, _) = listener.accept()?;
    socket.set_read_timeout(Some(DEADLOCK_GUARD))?;
    socket.set_write_timeout(Some(DEADLOCK_GUARD))?;
    let mut server = ServerConnection::new(Arc::new(config)).unwrap();
    while !server.wants_write() {
        if server.read_tls(&mut socket)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        server.process_new_packets().map_err(io::Error::other)?;
    }
    let mut flight = Vec::new();
    while server.wants_write() {
        server.write_tls(&mut flight)?;
    }
    socket.write_all(&empty_authorities(&flight))?;
    // The client rejects before authenticating the modified transcript.
    // Keep the peer alive until its socket is released, as in tls_peer.
    loop {
        if server.read_tls(&mut socket)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        server.process_new_packets().map_err(io::Error::other)?;
    }
}

fn empty_authorities(flight: &[u8]) -> Vec<u8> {
    let mut handshake = Vec::new();
    let mut records = flight;
    while !records.is_empty() {
        assert_eq!(&records[..3], &[22, 3, 3]);
        let length = usize::from(u16::from_be_bytes([records[3], records[4]]));
        handshake.extend_from_slice(&records[5..5 + length]);
        records = &records[5 + length..];
    }
    let mut cursor = 0;
    let mut replaced = false;
    while cursor < handshake.len() {
        let length = (usize::from(handshake[cursor + 1]) << 16)
            | (usize::from(handshake[cursor + 2]) << 8)
            | usize::from(handshake[cursor + 3]);
        let end = cursor + 4 + length;
        if handshake[cursor] == 13 {
            // CertificateRequest: retain certificate types and signature
            // schemes, replacing only the u16-length-prefixed authority list.
            let schemes = cursor + 5 + usize::from(handshake[cursor + 4]);
            let names = schemes
                + 2
                + usize::from(u16::from_be_bytes([
                    handshake[schemes],
                    handshake[schemes + 1],
                ]));
            assert_eq!(&handshake[names..end], &[0, 4, 0, 2, 0x30, 0]);
            let mut authorities = vec![0; 2 + 32_600 * 2];
            authorities[..2].copy_from_slice(&65_200_u16.to_be_bytes());
            handshake.splice(names..end, authorities);
            let length = u32::try_from(names + 2 + 65_200 - cursor - 4).unwrap();
            handshake[cursor + 1..cursor + 4].copy_from_slice(&length.to_be_bytes()[1..]);
            replaced = true;
            break;
        }
        cursor = end;
    }
    assert!(replaced, "TLS 1.2 CertificateRequest in peer flight");
    // Match rustls's original default fragmentation, including messages that
    // share records. Aligning messages would bypass the measured buffer limit.
    let mut wire = Vec::new();
    for payload in handshake.chunks(16_384) {
        let length = u16::try_from(payload.len()).unwrap().to_be_bytes();
        wire.extend_from_slice(&[22, 3, 3, length[0], length[1]]);
        wire.extend_from_slice(payload);
    }
    wire
}
