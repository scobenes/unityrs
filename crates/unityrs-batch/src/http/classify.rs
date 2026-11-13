//! Layer-sensitive, URL-free facts. Never render third-party error messages.
use super::{Detail, Error, Failure};
use std::io;

pub(crate) fn connect(error: &io::Error) -> Error {
    Error::Transport(Failure {
        detail: Detail::Connect,
        errno: error.raw_os_error(),
    })
}
pub(crate) fn tls(error: &io::Error) -> Error {
    let mut cause = error
        .get_ref()
        .map(|e| e as &(dyn std::error::Error + 'static));
    while let Some(error) = cause {
        if error.is::<rustls::Error>() {
            return Error::transport(Detail::Tls);
        }
        cause = error.source();
    }
    connect(error)
}
pub(crate) fn head(error: &hyper::Error) -> Error {
    if error.is_parse_too_large() {
        Error::transport(Detail::HeadTooLarge)
    } else {
        Error::transport(Detail::NoResponse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_typed_rustls_cause_is_tls_and_eof_is_connect() {
        let tls_error = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        );
        assert_eq!(tls(&tls_error).detail(), Some(Detail::Tls));
        for error in [
            io::Error::from(io::ErrorKind::UnexpectedEof),
            io::Error::other("rustls failure /?new-secret=hidden"),
        ] {
            let failure = tls(&error);
            assert_eq!(failure.detail(), Some(Detail::Connect));
            assert_eq!(
                format!("{failure:?}"),
                "Transport(Failure { detail: Connect, errno: None })"
            );
        }
        let error = io::Error::from_raw_os_error(111);
        assert_eq!(
            connect(&error),
            Error::Transport(Failure {
                detail: Detail::Connect,
                errno: Some(111)
            })
        );
    }
}
