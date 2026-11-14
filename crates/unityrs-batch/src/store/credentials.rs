//! The store's credentials, from the environment alone (`docs/BATCH_SPEC.md`
//! 1.6, `docs/plans/extract.md` E37): `AWS_ACCESS_KEY_ID`,
//! `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN` when set and not empty,
//! read once at preflight, with `AWS_CREDENTIAL_EXPIRATION` checked against
//! the deadline. A credential renders as its length alone; the secret only
//! keys the signature.
use crate::{
    reason::Preflight,
    spec::{formats, model::DateTime},
};
use object_store::{StaticCredentialProvider, aws::AwsCredential, aws::AwsCredentialProvider};
use std::{ffi::OsString, fmt, sync::Arc};

/// An access key id's and a secret's most bytes.
pub const KEY_MAX: usize = 128;
/// A session token's most bytes.
pub const TOKEN_MAX: usize = 16 * 1_024;

/// A credential's text. `Debug` and `Display` print its length alone.
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// The three credentials, checked.
#[derive(Debug)]
pub struct Credentials {
    pub key_id: Secret,
    pub secret: Secret,
    pub token: Option<Secret>,
}

impl Credentials {
    /// Reads the process's environment. `deadline` is the spec's.
    pub fn preflight(deadline: Option<&DateTime>) -> Result<Self, Preflight> {
        read(|name| std::env::var_os(name), deadline)
    }
    /// The one provider every client of the sink shares (`C_hdr`): the
    /// strings move into it, uncopied.
    #[must_use]
    pub fn into_provider(self) -> AwsCredentialProvider {
        Arc::new(StaticCredentialProvider::new(AwsCredential {
            key_id: self.key_id.0,
            secret_key: self.secret.0,
            token: self.token.map(|token| token.0),
        }))
    }
}

/// [`Credentials::preflight`] over any environment, in 1.6's order: either
/// of the first two unset or empty is `credentials_missing`; a value over
/// its cap, not UTF-8, or (a key id's or token's) holding a byte outside the
/// header-value set, or an expiration that is not RFC 3339, is
/// `credentials_invalid`; an expiration before the deadline is
/// `credentials_expire`.
///
/// The header-value set here is visible ASCII, space and tab: `object_store`
/// puts the key id and the token in headers through `HeaderValue::from_str`,
/// which refuses any other byte by panicking (`src/aws/credential.rs`), so a
/// non-ASCII byte that 1.2's set would let through is refused here.
pub fn read(
    env: impl Fn(&str) -> Option<OsString>,
    deadline: Option<&DateTime>,
) -> Result<Credentials, Preflight> {
    let key_id = env("AWS_ACCESS_KEY_ID").filter(|v| !v.is_empty());
    let secret = env("AWS_SECRET_ACCESS_KEY").filter(|v| !v.is_empty());
    let (Some(key_id), Some(secret)) = (key_id, secret) else {
        return Err(Preflight::CredentialsMissing);
    };
    let token = env("AWS_SESSION_TOKEN").filter(|v| !v.is_empty());
    let key_id = text(key_id, KEY_MAX, true)?;
    let secret = text(secret, KEY_MAX, false)?;
    let token = token.map(|t| text(t, TOKEN_MAX, true)).transpose()?;
    if let Some(expiration) = env("AWS_CREDENTIAL_EXPIRATION") {
        let at = expiration
            .to_str()
            .and_then(formats::date_time)
            .ok_or(Preflight::CredentialsInvalid)?;
        if deadline.is_some_and(|deadline| instant(&at) < instant(deadline)) {
            return Err(Preflight::CredentialsExpire);
        }
    }
    Ok(Credentials {
        key_id: Secret(key_id),
        secret: Secret(secret),
        token: token.map(Secret),
    })
}

fn text(value: OsString, max: usize, header: bool) -> Result<String, Preflight> {
    let value = value
        .into_string()
        .map_err(|_| Preflight::CredentialsInvalid)?;
    if value.len() > max
        || (header
            && !value
                .bytes()
                .all(|b| b == b'\t' || (0x20..0x7f).contains(&b)))
    {
        return Err(Preflight::CredentialsInvalid);
    }
    Ok(value)
}

/// A date-time's instant, as nanoseconds from 1970-01-01T00:00:00Z. A leap
/// second's `:60` is the next minute's start, the same instant to a second.
#[must_use]
pub fn instant(at: &DateTime) -> i128 {
    let (year, month) = (i64::from(at.year), i64::from(at.month));
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(at.day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400
        + i64::from(at.hour) * 3_600
        + i64::from(at.minute) * 60
        + i64::from(at.second)
        - i64::from(at.offset_minutes) * 60;
    i128::from(seconds) * 1_000_000_000 + i128::from(at.nanos)
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
