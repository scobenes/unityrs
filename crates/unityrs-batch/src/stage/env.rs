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
    /// Reserve both masker buffers during measured phase 1. Two URL
    /// components and every header per bundle bound either list, even
    /// when one variable occurs in many bundles. Empty slots are unused.
    pub(crate) fn mask_storage(spec: &Spec) -> Result<[Vec<&'static str>; 2], Error<'_>> {
        let count = spec.bundles().try_fold(0_usize, |count, bundle| {
            count
                .checked_add(bundle.headers().count())
                .and_then(|n| n.checked_add(if bundle.url().is_some() { 2 } else { 0 }))
                .ok_or(Error::Memory)
        })?;
        let mut buffers = [Vec::new(), Vec::new()];
        for buffer in &mut buffers {
            buffer.try_reserve_exact(count)?;
        }
        Ok(buffers)
    }
    /// Every variable the spec's headers name, read once each, in document
    /// order, so the first refusal is the first variable that fails.
    pub fn resolve<'a>(spec: &'a Spec, environment: &dyn Environment) -> Result<Self, Error<'a>> {
        let mut values = Self::default();
        values.resolve_into(spec, environment)?;
        Ok(values)
    }
    /// Preserve values already read when a later variable fails, so the
    /// process can still mask them in its preflight diagnostic.
    pub(crate) fn resolve_into<'a>(
        &mut self,
        spec: &'a Spec,
        environment: &dyn Environment,
    ) -> Result<(), Error<'a>> {
        for bundle in spec.bundles() {
            for (_, value) in bundle.headers() {
                let HeaderValue::Env(name) = value else {
                    continue;
                };
                let Err(at) = self.entries.binary_search_by(|(n, _)| (**n).cmp(name)) else {
                    continue;
                };
                let value = read(environment, name).map_err(|r| Error::Refused(r, name))?;
                if self.entries.len() == self.entries.capacity() {
                    // C_hdr has 48 B per table entry and 32 B of spare
                    // masker allowance. A 32 B entry with doubling growth
                    // would hold 96 B per entry while realloc moves it.
                    // Grow by a quarter instead: both capacities total at
                    // most 72 B per entry, leaving room for chunk rounding.
                    let extra = (self.entries.capacity() / 4).max(1);
                    self.entries.try_reserve_exact(extra)?;
                }
                // Keep the validated value's allocation, rather than briefly
                // owning a second full-size copy beside the header table.
                self.entries
                    .insert(at, (copy(name)?, value.into_boxed_str()));
            }
        }
        Ok(())
    }
    /// Phase 1's URL and literal-header masker inputs, before any
    /// environment read. The empty table contributes no borrowed values.
    pub(crate) fn unresolved_secrets<'a>(
        spec: &'a Spec,
        out: &mut Vec<&'a str>,
    ) -> Result<(), Error<'a>> {
        static EMPTY: Values = Values {
            entries: Vec::new(),
        };
        EMPTY.secrets(spec, out)
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
    /// and calls none of this; the process host feeds its masker this list.
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
    fn variable_table_growth_fits_headers_at_their_live_value_sizes() {
        use serde_json::json;
        use unityrs_core::limits::heap_cost;
        struct Environment;
        impl super::Environment for Environment {
            fn var(&self, _: &str) -> Option<OsString> {
                Some("v".into())
            }
        }
        // Just past a doubling boundary: a moving realloc must count both
        // table capacities, rather than only the final table's length.
        let count = 8_193_u64;
        let inputs: Vec<_> = (0..count)
            .map(|n| {
                json!({"id":format!("input-{n:05}"),
                "url":"http://127.0.0.1/file", "headers":{
                    "X-Value":{"env":format!("TOKEN_{n:05}")}
                }})
            })
            .collect();
        let raw = json!({"spec_version":1,"run_id":"r","attempt":1,"threads":1,
            "download_concurrency":1,"max_inflight_uncompressed_bytes":67_108_864,
            "max_decoded_bytes":1_048_576,"dep_cache_bytes":1_048_576,
            "spool":{"dir":"/spool","size_bytes":1_073_741_824,"reserve_bytes":0},
            "inputs":inputs,"outputs":{"textures":{"key":"{input}/{name}.png"}},
            "sink":{"kind":"directory","root":"/sink"}});
        let spec = crate::spec::parse(serde_json::to_vec(&raw).unwrap())
            .unwrap()
            .spec;
        // Fit the header term using the actual one-byte values; counting
        // unread values at 16 KiB would hide a growing table behind slack.
        let allowance =
            crate::bound::c_hdr(&spec) - count * (heap_cost(VALUE_CAP as u64) - heap_cost(1));
        let (values, measured) =
            crate::alloc::measured(|| Values::resolve(&spec, &Environment).unwrap());
        assert_eq!(values.len(), usize::try_from(count).unwrap());
        assert!(
            measured.peak <= allowance,
            "header peak {} exceeds live-value allowance {allowance}",
            measured.peak
        );
    }
    #[test]
    fn repeated_headers_fit_measured_masker_storage_without_later_growth() {
        use serde_json::json;
        struct Environment;
        impl super::Environment for Environment {
            fn var(&self, _: &str) -> Option<OsString> {
                Some("resolved secret".into())
            }
        }
        let inputs: Vec<_> = (0..1_000)
            .map(|n| {
                json!({
                    "id":format!("input-{n}"), "url":"http://127.0.0.1/file?query-secret",
                    "headers":{"X-Literal":"literal secret", "X-Env":{"env":"TOKEN"}}
                })
            })
            .collect();
        let raw = json!({"spec_version":1,"run_id":"r","attempt":1,"threads":1,
            "download_concurrency":1,"max_inflight_uncompressed_bytes":67_108_864,
            "max_decoded_bytes":1_048_576,"dep_cache_bytes":1_048_576,
            "spool":{"dir":"/spool","size_bytes":1_073_741_824,"reserve_bytes":0},
            "inputs":inputs,"outputs":{"textures":{"key":"{input}/{name}.png"}},
            "sink":{"kind":"directory","root":"/sink"}});
        let spec = Box::leak(Box::new(
            crate::spec::parse(serde_json::to_vec(&raw).unwrap())
                .unwrap()
                .spec,
        ));
        let values = Box::leak(Box::new(Values::resolve(spec, &Environment).unwrap()));
        let (buffers, measured) = crate::alloc::measured(|| Values::mask_storage(spec).unwrap());
        assert_eq!(
            measured.kept,
            2 * unityrs_core::limits::heap_cost(4_000 * 16)
        );
        let [mut known, mut resolved] = buffers;
        let ((), filled) = crate::alloc::measured(|| {
            Values::unresolved_secrets(spec, &mut known).unwrap();
            values.secrets(spec, &mut resolved).unwrap();
        });
        assert_eq!(known.len(), 2_000);
        assert_eq!(resolved.len(), 3_000);
        assert_eq!(filled.kept, 0);
        assert_eq!(filled.peak, 0);
        crate::test_alloc::REFUSE.with(|r| r.set(1));
        let refused = Values::mask_storage(spec);
        crate::test_alloc::REFUSE.with(|r| r.set(0));
        assert_eq!(refused.err(), Some(Error::Memory));
    }
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
