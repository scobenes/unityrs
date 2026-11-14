use super::*;
#[cfg(target_os = "linux")]
fn tcp_state(socket: &Socket<'_>) -> u8 {
    let mut info = [0_u8; 256];
    let mut length = 256;
    // SAFETY: initialized writable TCP_INFO buffer and its exact capacity;
    // the first byte is Linux tcp_info.tcpi_state, regardless of tail version.
    super::super::cvt(unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            info.as_mut_ptr().cast(),
            &raw mut length,
        )
    })
    .unwrap();
    info[0]
}
#[test]
#[cfg(target_os = "linux")]
fn syn_sent_disconnect_retains_the_fd_and_reports_hup() {
    use unityrs_testkit::guard::DEADLOCK_GUARD;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // SAFETY: live owned listener. A backlog of zero allows one established
    // unaccepted peer; the next loopback SYN remains in SYN_SENT.
    super::super::cvt(unsafe { libc::listen(listener.as_raw_fd(), 0) }).unwrap();
    let table = SocketTable::new();
    let filler = table.socket(0, false).unwrap();
    let _ = filler.connect(listener.local_addr().unwrap());
    let limit = std::time::Instant::now() + DEADLOCK_GUARD;
    while tcp_state(&filler) != 1 {
        // Linux TCP_ESTABLISHED.
        assert!(
            std::time::Instant::now() < limit,
            "deadlock guard: filler connect"
        );
        std::thread::yield_now();
    }
    let pending = table.socket(1, false).unwrap();
    let fd = pending.as_raw_fd();
    assert_eq!(
        pending
            .connect(listener.local_addr().unwrap())
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EINPROGRESS)
    );
    assert_eq!(tcp_state(&pending), 2); // Linux TCP_SYN_SENT: no peer socket yet.
    table.reset();
    assert!(pending.disconnected().unwrap());
    assert_eq!(pending.as_raw_fd(), fd);
    assert!(super::super::test_socket_live(fd));
    assert_eq!(
        pending.take_error().unwrap().unwrap().raw_os_error(),
        Some(libc::ECONNRESET)
    );
    assert_eq!(table.reset_errors(), 0);
}
#[test]
fn permit_exclusivity_generation_exhaustion_and_ordinary_release() {
    let table = SocketTable::new();
    assert!(table.socket(SOCKET_CAPACITY, false).is_err());
    let socket = table.socket(0, false).unwrap();
    let fd = socket.as_raw_fd();
    let old = table.entries[0].load(SeqCst);
    assert!(super::super::test_zero_linger(fd).unwrap());
    assert!(table.socket(0, false).is_err());
    drop(socket);
    assert!(!super::super::test_socket_live(fd));
    let replacement = table.socket(0, false).unwrap();
    assert_ne!(table.entries[0].load(SeqCst), old);
    // This is the scan's atomic generation recheck. The old fd, even if its
    // number was reused, cannot become sealed by the stale observation.
    assert!(
        table.entries[0]
            .compare_exchange(old, old | SEALED, SeqCst, SeqCst)
            .is_err()
    );
    drop(replacement);
    table.entries[0].store(MAX_GENERATION * GENERATION, SeqCst);
    assert!(table.socket(0, false).is_err());
}
#[test]
fn ordinary_close_also_resets_and_clears_before_reuse() {
    use std::io::Read;
    use unityrs_testkit::guard::DEADLOCK_GUARD;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let table = SocketTable::new();
    let socket = table.socket(0, false).unwrap();
    let result = socket.connect(listener.local_addr().unwrap());
    assert!(result.is_ok() || result.is_err_and(|e| e.raw_os_error() == Some(libc::EINPROGRESS)));
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(DEADLOCK_GUARD)).unwrap();
    drop(socket);
    assert_eq!(
        peer.read(&mut [0; 1]).unwrap_err().raw_os_error(),
        Some(libc::ECONNRESET)
    );
    assert_eq!(table.entries[0].load(SeqCst) & u64::from(u32::MAX), 0);
}
#[test]
fn reset_racing_release_cannot_disconnect_an_unrelated_reused_description() {
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicUsize;
    use unityrs_testkit::guard::DEADLOCK_GUARD;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    for _ in 0..32 {
        let table = SocketTable::new();
        let socket = table.socket(0, false).unwrap();
        let step = AtomicUsize::new(0);
        let (mut unrelated, mut peer) = std::thread::scope(|scope| {
            scope.spawn(|| {
                while step.load(SeqCst) == 0 {
                    std::thread::yield_now();
                }
                table.reset();
            });
            step.store(1, SeqCst);
            drop(socket);
            let unrelated = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let peer = listener.accept().unwrap().0;
            (unrelated, peer)
        });
        peer.set_read_timeout(Some(DEADLOCK_GUARD)).unwrap();
        unrelated.write_all(b"x").unwrap();
        let mut byte = [0; 1];
        peer.read_exact(&mut byte).unwrap();
        assert_eq!(byte, *b"x");
        assert!(table.closing());
        assert!(table.socket(1, false).is_err());
    }
}
#[test]
#[cfg(target_os = "linux")]
fn a_never_connected_socket_is_not_reported_disconnected() {
    let table = SocketTable::new();
    let socket = table.socket(0, false).unwrap();
    // TCP CLOSE polls POLLHUP, yet nothing reset or refused this socket.
    assert!(!socket.disconnected().unwrap());
    table.reset();
    assert!(socket.disconnected().unwrap());
}
#[test]
fn a_registered_socket_connects_and_moves_bytes_both_ways() {
    use std::io::{Read, Write};
    use unityrs_testkit::guard::DEADLOCK_GUARD;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let table = SocketTable::new();
    let socket = table.socket(0, false).unwrap();
    let result = socket.connect(listener.local_addr().unwrap());
    assert!(result.is_ok() || result.is_err_and(|e| e.raw_os_error() == Some(libc::EINPROGRESS)));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut registered = socket.register(runtime.handle()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(DEADLOCK_GUARD)).unwrap();
    runtime.block_on(async {
        tokio::time::timeout(DEADLOCK_GUARD, registered.connected())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(registered.write(b"ab").await.unwrap(), 2);
        let mut sent = [0; 2];
        peer.read_exact(&mut sent).unwrap();
        assert_eq!(&sent, b"ab");
        peer.write_all(b"cd").unwrap();
        let mut got = [0; 2];
        let n = std::future::poll_fn(|cx| registered.poll_read(cx, &mut got))
            .await
            .unwrap();
        assert_eq!(&got[..n], &b"cd"[..n]);
        assert!(!registered.disconnected().unwrap());
        assert!(registered.take_error().unwrap().is_none());
        assert!(registered.is_write_vectored());
        let slices = [IoSlice::new(b"ef"), IoSlice::new(b""), IoSlice::new(b"gh")];
        let n = std::future::poll_fn(|cx| {
            AsyncWrite::poll_write_vectored(Pin::new(&mut registered), cx, &slices)
        })
        .await
        .unwrap();
        assert_eq!(n, 4);
        let mut vectored = [0; 4];
        peer.read_exact(&mut vectored).unwrap();
        assert_eq!(&vectored, b"efgh");
        // More slices than one sendmsg takes: a short write, never an error.
        let many = [IoSlice::new(b"x"); MAX_IOVECS + 1];
        assert_eq!(
            registered.0.get_ref().write_vectored(&many).unwrap(),
            MAX_IOVECS
        );
        let mut taken = [0; MAX_IOVECS];
        peer.read_exact(&mut taken).unwrap();
        // Shutdown sends no FIN: the peer's read times out, and only the
        // drop's zero linger ends the connection, with an RST.
        std::future::poll_fn(|cx| AsyncWrite::poll_shutdown(Pin::new(&mut registered), cx))
            .await
            .unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let quiet = peer.read(&mut [0; 1]).unwrap_err();
        assert!(matches!(
            quiet.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    });
    // Before the drop: once reset, macOS refuses socket options (EINVAL).
    peer.set_read_timeout(Some(DEADLOCK_GUARD)).unwrap();
    drop(registered);
    assert_eq!(
        peer.read(&mut [0; 1]).unwrap_err().raw_os_error(),
        Some(libc::ECONNRESET)
    );
}
