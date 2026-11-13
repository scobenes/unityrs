//! Socket and TLS IO with one concrete hyper connection type.
use super::socket::Registered;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tokio_rustls::client::TlsStream;

pub enum Stream {
    Plain(Registered<TcpStream>),
    Tls(Box<TlsStream<Registered<TcpStream>>>),
}
impl<T: AsyncRead + Unpin> AsyncRead for Registered<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut().value.as_mut() {
            Some(value) => Pin::new(value).poll_read(cx, buf),
            None => Poll::Ready(Err(io::ErrorKind::NotConnected.into())),
        }
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for Registered<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut().value.as_mut() {
            Some(value) => Pin::new(value).poll_write(cx, buf),
            None => Poll::Ready(Err(io::ErrorKind::NotConnected.into())),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut().value.as_mut() {
            Some(value) => Pin::new(value).poll_flush(cx),
            None => Poll::Ready(Err(io::ErrorKind::NotConnected.into())),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Hyper shuts down completed connections before dropping its IO. A TCP
        // half-close would send FIN before zero-linger Drop could send RST.
        self.get_mut().close();
        Poll::Ready(Ok(()))
    }
    fn is_write_vectored(&self) -> bool {
        self.value
            .as_ref()
            .is_some_and(AsyncWrite::is_write_vectored)
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut().value.as_mut() {
            Some(value) => Pin::new(value).poll_write_vectored(cx, bufs),
            None => Poll::Ready(Err(io::ErrorKind::NotConnected.into())),
        }
    }
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
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
