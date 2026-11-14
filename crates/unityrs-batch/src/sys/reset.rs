//! In-place TCP disconnect; no descriptor replacement, duplication or close.
//!
//! Generation argument: each permit slot packs a generation, descriptor and
//! sealed bit in ONE `SeqCst` atomic. Release CASes its exact generation to empty
//! BEFORE owner close. Reset reads the fd, then CASes that same generation to
//! sealed; a failed CAS never uses the sampled fd. A successful seal prevents
//! release: the opaque owner retains its fd through process exit instead.
//! Thus there is no check-to-syscall reuse window, even when a returning signal
//! interrupts Drop. A generation never wraps; exhausted slots refuse admission.
//! Concurrent/nested scans may disconnect the same sealed description; nobody
//! can close/reuse it. Reset errors leave it sealed. EINTR retries, all other
//! failures are counted, and the scan continues. No reset waits for an owner.
//!
//! The opaque owner offers no cloning or borrowed owned-fd conversion. Its
//! connect checks closing before the syscall AND after it: a syscall crossing
//! the scan is disconnected again before the caller can use the connection.
//! No async registration changes description: Tokio 1.53.3 `io/async_fd.rs`'s
//! register safety contract requires a stable open description until deregister.
//! [`Socket::register`] therefore returns an opaque [`RegisteredSocket`]: the
//! `AsyncFd`, its readiness guards and every mutable or owned access stay
//! private, so no caller can duplicate, replace or take the descriptor while
//! it is registered. It exposes only the I/O and readiness the HTTP layer uses.
//! EPOLLHUP (not EPOLLERR) signals disconnect; poll exposes that as POLLHUP.
//! On macOS shutdown is best-effort and non-normative.
use std::{
    io,
    io::IoSlice,
    net::SocketAddr,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, Interest, ReadBuf, unix::AsyncFd};

pub const SOCKET_CAPACITY: usize = 514;
/// The most slices one vectored write sends (below every host's `IOV_MAX`);
/// the rest wait for the caller's next write, as a short write's do.
pub const MAX_IOVECS: usize = 64;
const SEALED: u64 = 1 << 63;
const GENERATION: u64 = 1 << 32;
const MAX_GENERATION: u64 = (1 << 31) - 1;
pub struct SocketTable {
    closing: AtomicBool,
    entries: [AtomicU64; SOCKET_CAPACITY],
    errors: AtomicU64,
}
impl Default for SocketTable {
    fn default() -> Self {
        Self::new()
    }
}
impl SocketTable {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            closing: AtomicBool::new(false),
            entries: [const { AtomicU64::new(0) }; SOCKET_CAPACITY],
            errors: AtomicU64::new(0),
        }
    }
    #[must_use]
    pub fn closing(&self) -> bool {
        self.closing.load(SeqCst)
    }
    #[must_use]
    pub fn reset_errors(&self) -> u64 {
        self.errors.load(SeqCst)
    }
    /// One entry per runtime permit. Caller cannot install a duplicated socket:
    /// this method creates and owns a fresh nonblocking TCP description.
    pub fn socket(&self, permit: usize, ipv6: bool) -> io::Result<Socket<'_>> {
        let entry = self
            .entries
            .get(permit)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let raw = tokio::net::TcpSocket::new_v4;
        let socket = if ipv6 {
            tokio::net::TcpSocket::new_v6()?
        } else {
            raw()?
        };
        let linger = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        // SAFETY: socket owns a live fd; linger is initialized and borrowed
        // for its exact size. Zero linger cannot block an ordinary close.
        super::cvt(unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                (&raw const linger).cast(),
                libc::socklen_t::try_from(size_of::<libc::linger>())
                    .map_err(|_| io::ErrorKind::InvalidInput)?,
            )
        })?;
        // Transfer unique ownership; no description is duplicated.
        // SAFETY: into_raw_fd transfers the socket's unique ownership.
        let fd = unsafe { OwnedFd::from_raw_fd(socket.into_raw_fd()) };
        if fd.as_raw_fd() < 3 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let old = entry.load(SeqCst);
        let generation = (old >> 32) & MAX_GENERATION;
        if old & (SEALED | u64::from(u32::MAX)) != 0 || generation == MAX_GENERATION {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let word = ((generation + 1) * GENERATION)
            | u64::try_from(fd.as_raw_fd()).map_err(|_| io::ErrorKind::InvalidInput)?;
        entry
            .compare_exchange(old, word, SeqCst, SeqCst)
            .map_err(|_| io::ErrorKind::WouldBlock)?;
        let owned = Socket {
            fd: Some(fd),
            table: self,
            entry,
            word,
        };
        // Publication precedes this load. Late publication never connects.
        if self.closing() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        Ok(owned)
    }
    /// Async-signal-safe, allocation-free and lock-free. It never changes a
    /// descriptor's file description, closes a descriptor, or runs a destructor.
    pub fn reset(&self) {
        self.closing.store(true, SeqCst);
        for entry in &self.entries {
            let word = entry.load(SeqCst);
            let Ok(fd) = i32::try_from(word & u64::from(u32::MAX)) else {
                continue;
            };
            if fd < 3 {
                continue;
            }
            if word & SEALED == 0
                && entry
                    .compare_exchange(word, word | SEALED, SeqCst, SeqCst)
                    .is_err()
            {
                continue;
            }
            if disconnect(fd).is_err() {
                self.errors.fetch_add(1, SeqCst);
            }
        }
    }
}
/// Ownership is deliberately opaque: no safe clone, dup, into-inner, or
/// external close. A sealed owner is retained until _exit. Ordinary Drop
/// clears the exact generation before close. Register this owner, not its fd.
pub struct Socket<'a> {
    fd: Option<OwnedFd>,
    table: &'a SocketTable,
    entry: &'a AtomicU64,
    word: u64,
}
impl<'a> Socket<'a> {
    pub fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.table.closing() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        // SAFETY: this opaque owner keeps the descriptor alive. recv borrows
        // exactly buffer's writable extent and never blocks on readiness.
        let n = unsafe {
            libc::recv(
                self.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_DONTWAIT,
            )
        };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    }
    pub fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        if self.table.closing() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL;
        #[cfg(not(target_os = "linux"))]
        let flags = libc::MSG_DONTWAIT;
        // SAFETY: this opaque owner keeps the fd alive. send borrows the
        // supplied readable slice and is nonblocking even on a full socket.
        let n = unsafe {
            libc::send(
                self.as_raw_fd(),
                buffer.as_ptr().cast(),
                buffer.len(),
                flags,
            )
        };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    }
    /// One nonblocking `sendmsg` of up to [`MAX_IOVECS`] slices, with no
    /// allocation: std's `IoSlice` is ABI-compatible with `iovec` on Unix,
    /// so the caller's slices are the iovec array itself.
    pub fn write_vectored(&self, buffers: &[IoSlice<'_>]) -> io::Result<usize> {
        if self.table.closing() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        let buffers = buffers.get(..MAX_IOVECS).unwrap_or(buffers);
        // SAFETY: zeroed msghdr (integers and null pointers) is valid: no
        // name, no control data.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = buffers.as_ptr().cast_mut().cast::<libc::iovec>();
        // msg_iovlen is size_t on Linux and int on macOS.
        #[allow(clippy::useless_conversion)]
        let length = buffers
            .len()
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        message.msg_iovlen = length;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL;
        #[cfg(not(target_os = "linux"))]
        let flags = libc::MSG_DONTWAIT;
        // SAFETY: message names at most MAX_IOVECS readable iovecs (IoSlice's
        // guaranteed iovec layout) borrowed for this call; sendmsg only reads
        // them. This opaque owner keeps the fd alive; nonblocking.
        let n = unsafe { libc::sendmsg(self.as_raw_fd(), &raw const message, flags) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        let mut error = 0_i32;
        let mut length =
            libc::socklen_t::try_from(size_of::<i32>()).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: writable integer and exact length, live owned socket.
        super::cvt(unsafe {
            libc::getsockopt(
                self.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut error).cast(),
                &raw mut length,
            )
        })?;
        Ok((error != 0).then(|| io::Error::from_raw_os_error(error)))
    }
    pub fn connect(&self, address: SocketAddr) -> io::Result<()> {
        if self.table.closing() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        let fd = self.as_raw_fd();
        let result = connect(fd, address);
        if self.table.closing() {
            let _ = disconnect(fd);
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        result
    }
    /// Whether this socket was disconnected: the kernel reports `POLLHUP`
    /// AND either the table is closing (a reset scan ran) or `SO_ERROR` holds
    /// a pending error (a reset or refusal). `POLLHUP` alone is not enough: a
    /// socket that never connected is in TCP CLOSE and polls `POLLHUP` too.
    /// Reading `SO_ERROR` consumes that pending error, as [`Self::take_error`]
    /// does, so a caller wanting the error itself calls that first.
    pub fn disconnected(&self) -> io::Result<bool> {
        let mut p = libc::pollfd {
            fd: self.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: p is one initialized writable pollfd; zero timeout.
        super::cvt(unsafe { libc::poll(&raw mut p, 1, 0) }).or_else(|e| {
            if e.kind() == io::ErrorKind::Interrupted {
                Ok(())
            } else {
                Err(e)
            }
        })?;
        if p.revents & libc::POLLHUP == 0 {
            return Ok(false);
        }
        Ok(self.table.closing() || self.take_error()?.is_some())
    }
    /// One check of an in-progress connect: `SO_ERROR`'s failure, success
    /// once a peer name exists, else `WouldBlock`, which clears the readiness.
    fn finish_connect(&self) -> io::Result<()> {
        if self.table.closing() {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        if let Some(error) = self.take_error()? {
            return Err(error);
        }
        // SAFETY: zeroed storage is a valid sockaddr_storage of integers.
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut length = libc::socklen_t::try_from(size_of::<libc::sockaddr_storage>())
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: writable storage and its exact length; live owned socket.
        super::cvt(unsafe {
            libc::getpeername(self.as_raw_fd(), (&raw mut storage).cast(), &raw mut length)
        })
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ENOTCONN) {
                io::ErrorKind::WouldBlock.into()
            } else {
                e
            }
        })
    }
    /// Register with `handle`'s reactor (under `Handle::enter`, so a future
    /// holding only a runtime handle can register). The returned owner keeps
    /// the `AsyncFd` private, so the descriptor stays this owner's alone.
    pub fn register(self, handle: &tokio::runtime::Handle) -> io::Result<RegisteredSocket<'a>> {
        let _entered = handle.enter();
        // SAFETY: AsyncFd owns this opaque Socket; its descriptor stays open
        // and unchanged, including during reset. RegisteredSocket exposes no
        // get_mut, into_inner, AsFd or guard, so nothing can replace, take or
        // duplicate it while registered. Deregistration precedes Drop.
        unsafe { AsyncFd::register(self) }
            .map(RegisteredSocket)
            .map_err(|e| e.into_parts().1)
    }
}
/// A [`Socket`] registered for readiness: what the HTTP layer adopts. Its
/// operations are nonblocking I/O and readiness waits only; there is no clone,
/// no descriptor access, no mutable inner access and no readiness guard.
pub struct RegisteredSocket<'a>(AsyncFd<Socket<'a>>);
impl RegisteredSocket<'_> {
    /// Complete a connect [`Socket::connect`] left in progress (`EINPROGRESS`):
    /// wait for write readiness, then report `SO_ERROR`.
    pub async fn connected(&self) -> io::Result<()> {
        self.0
            .async_io(Interest::WRITABLE, Socket::finish_connect)
            .await
    }
    /// Wait until the socket may be readable; a later read can still find
    /// nothing, which clears the readiness (as Tokio's `TcpStream` does).
    pub async fn readable(&self) -> io::Result<()> {
        self.0.readable().await.map(drop)
    }
    /// Wait until the socket may be writable (see [`Self::readable`]).
    pub async fn writable(&self) -> io::Result<()> {
        self.0.writable().await.map(drop)
    }
    pub async fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0
            .async_io(Interest::READABLE, |s| s.read(buffer))
            .await
    }
    pub async fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .async_io(Interest::WRITABLE, |s| s.write(buffer))
            .await
    }
    pub fn poll_read(&self, cx: &mut Context<'_>, buffer: &mut [u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            if let Ok(result) = guard.try_io(|s| s.get_ref().read(buffer)) {
                return Poll::Ready(result);
            }
        }
    }
    pub fn poll_write(&self, cx: &mut Context<'_>, buffer: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            if let Ok(result) = guard.try_io(|s| s.get_ref().write(buffer)) {
                return Poll::Ready(result);
            }
        }
    }
    pub fn poll_write_vectored(
        &self,
        cx: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            if let Ok(result) = guard.try_io(|s| s.get_ref().write_vectored(buffers)) {
                return Poll::Ready(result);
            }
        }
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.0.get_ref().take_error()
    }
    /// See [`Socket::disconnected`].
    pub fn disconnected(&self) -> io::Result<bool> {
        self.0.get_ref().disconnected()
    }
}
impl AsyncRead for RegisteredSocket<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = ready!(RegisteredSocket::poll_read(
            &self,
            cx,
            buffer.initialize_unfilled()
        ))?;
        buffer.advance(n);
        Poll::Ready(Ok(()))
    }
}
impl AsyncWrite for RegisteredSocket<'_> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        RegisteredSocket::poll_write(&self, cx, buffer)
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        RegisteredSocket::poll_write_vectored(&self, cx, buffers)
    }
    fn is_write_vectored(&self) -> bool {
        true
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    /// A no-op: sends no FIN. No FIN (or TLS `close_notify`) may precede the
    /// reset's RST, so a connection closes only by drop (zero linger: RST)
    /// or by the exit scan's reset, never by a half-close.
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl AsRawFd for Socket<'_> {
    fn as_raw_fd(&self) -> i32 {
        self.fd.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }
}
impl Drop for Socket<'_> {
    fn drop(&mut self) {
        let empty = self.word & !(u64::from(u32::MAX));
        if self
            .entry
            .compare_exchange(self.word, empty, SeqCst, SeqCst)
            .is_err()
        {
            // A reset owns the generation. Retain the description even if a
            // future/owner is accidentally dropped while the process finishes.
            if let Some(fd) = self.fd.take() {
                std::mem::forget(fd);
            }
        }
    }
}
fn disconnect(fd: i32) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let address = libc::sockaddr {
            sa_family: libc::sa_family_t::try_from(libc::AF_UNSPEC)
                .map_err(|_| io::ErrorKind::InvalidInput)?,
            sa_data: [0; 14],
        };
        loop {
            // SAFETY: sockaddr has the exact initialized size supplied; the
            // sealed entry guarantees the descriptor cannot close or be reused.
            let rc = unsafe {
                libc::connect(
                    fd,
                    &raw const address,
                    libc::socklen_t::try_from(size_of::<libc::sockaddr>())
                        .map_err(|_| io::ErrorKind::InvalidInput)?,
                )
            };
            if rc == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: scalar syscall, live owned fd; non-normative test host path.
        super::cvt(unsafe { libc::shutdown(fd, libc::SHUT_RDWR) })
    }
}
fn connect(fd: i32, address: SocketAddr) -> io::Result<()> {
    // SAFETY: both address structs contain only integer fields. Zeroing
    // initializes all padding and platform-specific length fields.
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let length = match address {
        SocketAddr::V4(a) => {
            // SAFETY: storage is aligned and large enough for sockaddr_in.
            let s = unsafe { &mut *(&raw mut storage).cast::<libc::sockaddr_in>() };
            s.sin_family = libc::sa_family_t::try_from(libc::AF_INET)
                .map_err(|_| io::ErrorKind::InvalidInput)?;
            s.sin_port = a.port().to_be();
            #[cfg(target_os = "macos")]
            {
                s.sin_len = u8::try_from(size_of::<libc::sockaddr_in>())
                    .map_err(|_| io::ErrorKind::InvalidInput)?;
            }
            s.sin_addr.s_addr = u32::from_ne_bytes(a.ip().octets());
            size_of::<libc::sockaddr_in>()
        }
        SocketAddr::V6(a) => {
            // SAFETY: storage is aligned and large enough for sockaddr_in6.
            let s = unsafe { &mut *(&raw mut storage).cast::<libc::sockaddr_in6>() };
            s.sin6_family = libc::sa_family_t::try_from(libc::AF_INET6)
                .map_err(|_| io::ErrorKind::InvalidInput)?;
            s.sin6_port = a.port().to_be();
            #[cfg(target_os = "macos")]
            {
                s.sin6_len = u8::try_from(size_of::<libc::sockaddr_in6>())
                    .map_err(|_| io::ErrorKind::InvalidInput)?;
            }
            s.sin6_addr.s6_addr = a.ip().octets();
            s.sin6_scope_id = a.scope_id();
            s.sin6_flowinfo = a.flowinfo();
            size_of::<libc::sockaddr_in6>()
        }
    };
    let length = libc::socklen_t::try_from(length).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: storage holds the initialized sockaddr corresponding to length.
    super::cvt(unsafe { libc::connect(fd, (&raw const storage).cast(), length) })
}

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;
