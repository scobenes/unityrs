//! One deadline for resolution, sequential TCP attempts, and the TLS handshake.
use super::{
    Detail, Error,
    io::Stream,
    permit::{Charge, OperationToken, RequestPermit},
    resolve::{self, Source},
};
use crate::sys::reset::{RegisteredSocket, SocketTable};
use rustls::{ClientConfig, pki_types::ServerName};
use std::{future::poll_fn, mem::MaybeUninit, sync::Arc, time::Duration};
use tokio::{
    runtime::Handle,
    time::{Instant, timeout_at},
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use url::Url;

/// An `https` request's server name and its stream's storage, both taken
/// before the first socket: a refusal after a connect would drop (reset) it.
struct Secure {
    name: ServerName<'static>,
    storage: Box<MaybeUninit<TlsStream<RegisteredSocket>>>,
}
pub(crate) struct Connector {
    pub(crate) pool: Handle,
    pub(crate) table: Arc<SocketTable>,
    pub(crate) tls: Arc<ClientConfig>,
    pub(crate) resolver: Source,
}
impl Connector {
    pub(crate) async fn connect(
        &self,
        permit: &mut RequestPermit,
        url: &Url,
        timeout: Duration,
        token: Option<OperationToken>,
        envelope: &mut Charge,
    ) -> Result<Stream, Error> {
        // A previous timed-out lookup's wait is outside this connection timer.
        if token.is_none() {
            poll_fn(|cx| permit.poll_available(cx)).await;
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(Error::InvalidRequest)?;
        timeout_at(deadline, self.until(permit, url, deadline, token, envelope))
            .await
            .unwrap_or_else(|_| Err(Error::transport(Detail::Connect)))
    }
    async fn until(
        &self,
        permit: &mut RequestPermit,
        url: &Url,
        deadline: Instant,
        token: Option<OperationToken>,
        envelope: &mut Charge,
    ) -> Result<Stream, Error> {
        let host = url.host_str().ok_or(Error::InvalidRequest)?;
        // url's IPv6 host_str includes brackets; the resolver and ServerName
        // consume the address itself, whereas Host keeps the authority form.
        let host = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        let port = url.port_or_known_default().ok_or(Error::InvalidRequest)?;
        let addresses =
            resolve::lookup(&self.pool, permit, &self.resolver, host, port, token).await?;
        envelope
            .grow(resolve::LOOKUP_ENVELOPE)
            .ok_or(Error::Memory)?;
        let mut secure = if url.scheme() == "http" {
            None
        } else {
            Some(Secure::new(host)?)
        };
        let mut last = Error::transport(Detail::Connect);
        let count = addresses.len();
        for index in 0..count {
            let address = addresses.get(index).ok_or(Error::InvalidRequest)?;
            let remaining = u32::try_from(count - index).map_err(|_| Error::InvalidRequest)?;
            let now = Instant::now();
            let left = deadline.saturating_duration_since(now);
            if left.is_zero() {
                return Err(Error::transport(Detail::Connect));
            }
            let share = now + left / remaining;
            // The table makes the zero-linger socket and publishes it in
            // this permit's entry before any connect; a closing table or a
            // sealed or busy entry refuses, and no further address is tried.
            let socket = match self.table.socket(permit.index(), address.is_ipv6()) {
                Ok(socket) => socket,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::InvalidInput
                    ) =>
                {
                    return Err(super::classify::connect(&error));
                }
                Err(error) => {
                    last = super::classify::connect(&error);
                    continue;
                }
            };
            let socket = match socket.register(&self.pool) {
                Ok(socket) => socket,
                Err(error) => {
                    last = super::classify::connect(&error);
                    continue;
                }
            };
            // timeout_at owns only the readiness wait. The socket's owner
            // clears its entry and closes (zero linger: RST) on failure.
            match timeout_at(share, connect(&socket, address)).await {
                Ok(Ok(())) => return self.secure(socket, secure.take()).await,
                Ok(Err(error)) => last = super::classify::connect(&error),
                Err(_) => last = Error::transport(Detail::Connect),
            }
        }
        Err(last)
    }
    async fn secure(
        &self,
        stream: RegisteredSocket,
        secure: Option<Secure>,
    ) -> Result<Stream, Error> {
        let Some(Secure { name, storage }) = secure else {
            return Ok(Stream::Plain(stream));
        };
        let stream = TlsConnector::from(self.tls.clone())
            .connect(name, stream)
            .await
            .map_err(|e| super::classify::tls(&e))?;
        Ok(Stream::Tls(Box::write(storage, stream)))
    }
}
impl Secure {
    fn new(host: &str) -> Result<Self, Error> {
        let mut owned = String::new();
        owned
            .try_reserve_exact(host.len())
            .map_err(|_| Error::Memory)?;
        owned.push_str(host);
        let name = ServerName::try_from(owned).map_err(|_| Error::InvalidRequest)?;
        let storage = crate::alloc::try_box_uninit().map_err(|_| Error::Memory)?;
        Ok(Self { name, storage })
    }
}
/// One connect: at once, or left in progress and completed on readiness.
async fn connect(socket: &RegisteredSocket, address: std::net::SocketAddr) -> std::io::Result<()> {
    match socket.connect(address) {
        Err(error) if error.raw_os_error() == Some(libc::EINPROGRESS) => socket.connected().await,
        result => result,
    }
}
