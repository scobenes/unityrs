//! Closed, URL-free transport facts and retry classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Detail {
    Connect,
    Tls,
    HeadTooLarge,
    NoResponse,
    Timeout,
    Truncated,
    Status(u16),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryClass {
    Connect,
    NoResponse,
    Timeout,
    Truncated,
    Status5xx,
    Status429,
}
impl Detail {
    #[must_use]
    pub const fn retry_class(self) -> Option<RetryClass> {
        match self {
            Self::Connect | Self::Tls => Some(RetryClass::Connect),
            Self::NoResponse => Some(RetryClass::NoResponse),
            Self::Timeout | Self::Status(408) => Some(RetryClass::Timeout),
            Self::Truncated => Some(RetryClass::Truncated),
            Self::Status(500..=599) => Some(RetryClass::Status5xx),
            Self::Status(429) => Some(RetryClass::Status429),
            Self::HeadTooLarge | Self::Status(_) => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Failure {
    pub detail: Detail,
    pub errno: Option<i32>,
}
/// Host failures remain distinct from bundle transport failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Transport(Failure),
    Memory,
    InvalidRequest,
    Busy,
    Pool,
}
impl Error {
    #[must_use]
    pub const fn detail(self) -> Option<Detail> {
        match self {
            Self::Transport(failure) => Some(failure.detail),
            _ => None,
        }
    }
    pub(crate) const fn transport(detail: Detail) -> Self {
        Self::Transport(Failure {
            detail,
            errno: None,
        })
    }
}
