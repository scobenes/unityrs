//! `{"env": NAME}` header values, read once per variable at preflight
//! (`BATCH_SPEC.md` 1.2), into one shared table every stage reads; and the
//! masker's inputs: each URL's query and userinfo, and every header value.
use super::headers;
use crate::spec::model::{HeaderValue, Spec};
use std::{collections::TryReserveError, ffi::OsString};

/// The longest value a variable may hold (`header_env_too_long` past it).
pub const VALUE_CAP: usize = 16 * 1024;
/// Where preflight reads a variable. The engine's is the process
/// environment; a library caller or a test may supply its own.
pub trait Environment: Send + Sync {
    fn var(&self, name: &str) -> Option<OsString>;
}
pub struct System;
impl Environment for System {
    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Unset or empty.
    Missing,
    /// Over [`VALUE_CAP`] bytes.
    TooLong,
    /// Not UTF-8, or a byte other than tab, space, visible ASCII or
    /// 0x80-0xFF.
    Invalid,
}
impl Refusal {
    /// Its 69 reason (`BATCH_SPEC.md` 9.3); the detail is the variable.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Missing => "header_env_missing",
            Self::TooLong => "header_env_too_long",
            Self::Invalid => "header_env_invalid",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<'a> {
    /// The first refused variable, in document order.
    Refused(Refusal, &'a str),
    Memory,
}
impl From<TryReserveError> for Error<'_> {
    fn from(_: TryReserveError) -> Self {
        Self::Memory
    }
}
/// Each distinct variable once, sorted by name, its value shared by every
/// bundle that names it (`C_hdr`).
#[derive(Default)]
pub struct Values {
    entries: Vec<(Box<str>, Box<str>)>,
}
impl headers::Values for Values {
    fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .binary_search_by(|(n, _)| (**n).cmp(name))
            .ok()
            .and_then(|at| self.entries.get(at))
            .map(|(_, v)| &**v)
    }
}
impl Values {
    /// Every variable the spec's headers name, read once each, in document
    /// order, so the first refusal is the first variable that fails.
    pub fn resolve<'a>(spec: &'a Spec, environment: &dyn Environment) -> Result<Self, Error<'a>> {
        let mut values = Self::default();
        for bundle in spec.bundles() {
            for (_, value) in bundle.headers() {
                let HeaderValue::Env(name) = value else {
                    continue;
                };
                let Err(at) = values.entries.binary_search_by(|(n, _)| (**n).cmp(name)) else {
                    continue;
                };
                let value = read(environment, name).map_err(|r| Error::Refused(r, name))?;
                values.entries.try_reserve(1)?;
                values.entries.insert(at, (copy(name)?, copy(&value)?));
            }
        }
        Ok(values)
    }
    /// Distinct variables read.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// The masker's list (2.11): each URL's query and userinfo as written,
    /// every literal header value and every resolved one. Empty parts are
    /// left out; the masker skips them anyway. A library run has no masker
    /// and calls none of this; B10c-2 feeds the list to the process's.
    pub fn secrets<'a>(&'a self, spec: &'a Spec, out: &mut Vec<&'a str>) -> Result<(), Error<'a>> {
        for bundle in spec.bundles() {
            if let Some(url) = bundle.url() {
                for part in [query(url), userinfo(url)].into_iter().flatten() {
                    out.try_reserve(1)?;
                    out.push(part);
                }
            }
            for (_, value) in bundle.headers() {
                let text = match value {
                    HeaderValue::Literal(text) => Some(text),
                    HeaderValue::Env(name) => headers::Values::get(self, name),
                };
                if let Some(text) = text.filter(|t| !t.is_empty()) {
                    out.try_reserve(1)?;
                    out.push(text);
                }
            }
        }
        Ok(())
    }
}
fn read(environment: &dyn Environment, name: &str) -> Result<String, Refusal> {
    let value = environment
        .var(name)
        .filter(|v| !v.is_empty())
        .ok_or(Refusal::Missing)?;
    if value.len() > VALUE_CAP {
        return Err(Refusal::TooLong);
    }
    let value = value.into_string().map_err(|_| Refusal::Invalid)?;
    if value
        .bytes()
        .any(|b| !(b == b'\t' || (0x20..0x7f).contains(&b) || b >= 0x80))
    {
        return Err(Refusal::Invalid);
    }
    Ok(value)
}
fn copy(text: &str) -> Result<Box<str>, TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(text.len())?;
    owned.push_str(text);
    Ok(owned.into_boxed_str())
}
/// The query as written: after the first `?`, up to a `#`.
fn query(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once('?')?;
    let query = rest.split_once('#').map_or(rest, |(q, _)| q);
    (!query.is_empty()).then_some(query)
}
/// The userinfo as written: in the authority, before its last `@`. URL
/// validation refuses one, so this is the masker's defence in depth.
fn userinfo(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once(':')?;
    let rest = rest.trim_start_matches(['/', '\\']);
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let (info, _) = rest.get(..end)?.rsplit_once('@')?;
    (!info.is_empty()).then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_and_userinfo_are_taken_as_written() {
        assert_eq!(query("http://h/p?a=1&b=2#f"), Some("a=1&b=2"));
        assert_eq!(query("http://h/p?#f"), None);
        assert_eq!(query("http://h/p"), None);
        assert_eq!(userinfo("http://u:p@h/x@y"), Some("u:p"));
        assert_eq!(userinfo("http://h/x@y"), None);
        assert_eq!(userinfo("http://a@b@h?q@r"), Some("a@b"));
    }
}
