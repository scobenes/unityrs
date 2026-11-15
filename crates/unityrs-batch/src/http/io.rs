//! Socket and TLS IO with one concrete hyper connection type.
use crate::sys::reset::RegisteredSocket;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::client::TlsStream;

/// Test-only replacement for plain socket reads. Scripts can retain a
/// short OS read until a complete scheduled piece is available, so parser
/// boundaries and stop points do not depend on TCP packet timing.
#[cfg(feature = "test-hooks")]
pub struct PlainReadHooks {
    pub connected: fn(),
    pub read: fn(&RegisteredSocket, &mut Context<'_>, &mut ReadBuf<'_>) -> Poll<io::Result<()>>,
}
#[cfg(feature = "test-hooks")]
static READ_HOOKS: std::sync::OnceLock<PlainReadHooks> = std::sync::OnceLock::new();
#[cfg(feature = "test-hooks")]
/// Installs a process-wide script before any connection is made.
///
/// # Panics
/// Panics if a script was already installed.
pub fn arm_plain_reads(hooks: PlainReadHooks) {
    assert!(READ_HOOKS.set(hooks).is_ok(), "plain reads already armed");
}
#[cfg(feature = "test-hooks")]
pub(super) fn plain_connected() {
    if let Some(hooks) = READ_HOOKS.get() {
        (hooks.connected)();
    }
}

/// The socket's owner is the table's opaque [`RegisteredSocket`]: its
/// shutdown is a no-op, so neither a FIN nor TLS `close_notify` precedes
/// the RST its zero-linger drop (or the exit's reset) sends.
pub enum Stream {
    Plain(RegisteredSocket),
    Tls(Box<TlsStream<RegisteredSocket>>),
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => {
                #[cfg(feature = "test-hooks")]
                if let Some(hooks) = READ_HOOKS.get() {
                    return (hooks.read)(stream, cx, buf);
                }
                Pin::new(stream).poll_read(cx, buf)
            }
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            // Bypass rustls shutdown too: its close_notify is graceful TLS EOF.
            Self::Tls(stream) => Pin::new(stream.get_mut().0).poll_shutdown(cx),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(stream) => stream.is_write_vectored(),
            Self::Tls(stream) => stream.is_write_vectored(),
        }
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write_vectored(cx, bufs),
        }
    }
}
