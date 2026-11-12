//! The string forms of `fields::Format`, checked as `format`, and the two
//! URL forms, whose caps (as written, on the host as written, and on the
//! `url` crate's serialization) are checked here too, as `range`
//! (`docs/BATCH_SPEC.md` 1.2, 1.6).

use url::Url;

use super::fields::{Format, MAX_ENDPOINT, MAX_HOST, MAX_URL};
use super::model::DateTime;

/// What a string's form check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// It has the form.
    Ok,
    /// A URL over one of its caps.
    Range,
    /// It lacks the form.
    Format,
}

impl From<bool> for Verdict {
    fn from(ok: bool) -> Self {
        if ok { Self::Ok } else { Self::Format }
    }
}

/// Checks `s` against `format`.
pub(crate) fn check(format: Format, s: &str) -> Verdict {
    match format {
        Format::RunId => (s != "." && s != ".." && s.bytes().all(run_id_byte)).into(),
        Format::DateTime => date_time(s).is_some().into(),
        Format::AbsPath => (s.starts_with('/') && !s.contains('\0')).into(),
        Format::Url => url(s),
        Format::Endpoint => endpoint(s),
        Format::Sha256 => lower_hex(s, 64).into(),
        Format::Md5 => lower_hex(s, 32).into(),
        Format::Bucket => s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            .into(),
        Format::Region => s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            .into(),
        Format::Prefix => (s.is_empty() || s.ends_with('/')).into(),
        Format::Id => (!s
            .chars()
            .any(|c| is_control(c) || ('\u{80}'..='\u{9f}').contains(&c)))
        .into(),
        Format::EnvName => env_name(s).into(),
    }
}

fn run_id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')
}

/// A control character as 2.3 counts one: below U+0020, or U+007F.
pub(crate) fn is_control(c: char) -> bool {
    c < ' ' || c == '\u{7f}'
}

fn lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn env_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// An HTTP token (RFC 9110 5.6.2): a header's name.
pub(crate) fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// A header value's bytes: tab, space, visible ASCII and 0x80-0xFF
/// (`docs/BATCH_SPEC.md` 1.2).
pub(crate) fn is_header_value(s: &str) -> bool {
    s.bytes()
        .all(|b| b == b'\t' || b == b' ' || (0x21..=0x7e).contains(&b) || b >= 0x80)
}

fn http_scheme(u: &Url) -> bool {
    matches!(u.scheme(), "http" | "https")
}

fn no_userinfo(u: &Url) -> bool {
    u.username().is_empty() && u.password().is_none()
}

/// The bytes of `s`'s host as written (`docs/BATCH_SPEC.md` 1.2): after
/// the scheme's `:` and every `/` or `\` that follows it, up to the next
/// `/`, `\`, `?` or `#`; after the last `@` there; and before a `:` read
/// outside brackets, where any `[` opens them and any `]` closes them, as
/// `url` 2.5.8's `Parser::parse_host` reads them; tabs and newlines left
/// out, as it leaves them out. So, for an `http` or `https` URL, this is
/// at least the text that `parse_host` hands to the host parser, and the
/// cap bounds that parse (its IDNA mapping and punycode checks included).
/// Not always exactly: `Url::parse` first trims the C0 controls and
/// spaces that lead or end the string, which this still counts when they
/// end a host (`https://e.net ` measures 6, its parsed host 5), so the
/// measure is conservative, refusing some hosts the parse would accept. The
/// serialized host may still be longer than the text: an IPv4 address
/// written short is serialized in full (`http://1/` gives `0.0.0.1`, at
/// most 15 bytes), and an IPv6 one in brackets (at most 41), both bounded
/// whatever was written. For another scheme, refused as `format` later,
/// the parse is the whole string's, which the cap as written bounds.
pub(crate) fn written_host_len(s: &str) -> usize {
    let mut b = s.bytes().filter(|b| !matches!(b, b'\t' | b'\n' | b'\r'));
    if !b.any(|b| b == b':') {
        return 0;
    }
    let mut b = b.skip_while(|b| matches!(b, b'/' | b'\\')).peekable();
    // The authority: its length to the last `@`, and the host's after it.
    let (mut host, mut bracket, mut port) = (0usize, false, false);
    while let Some(c) = b.next_if(|b| !matches!(b, b'/' | b'\\' | b'?' | b'#')) {
        match c {
            b'@' => (host, bracket, port) = (0, false, false),
            _ if port => {}
            b'[' => {
                bracket = true;
                host += 1;
            }
            b']' => {
                bracket = false;
                host += 1;
            }
            b':' if !bracket => port = true,
            _ => host += 1,
        }
    }
    host
}

/// The checks a `url` or `endpoint` passes before it is parsed, in order:
/// ASCII as written (`format`), at most `cap` bytes as written, its host
/// as written at most [`MAX_HOST`] bytes (`range`). So `Url::parse` never
/// reads more than `cap` bytes, nor maps a long host.
fn as_written(s: &str, cap: u64) -> Verdict {
    if !s.is_ascii() {
        Verdict::Format
    } else if s.len() as u64 > cap || written_host_len(s) as u64 > MAX_HOST {
        Verdict::Range
    } else {
        Verdict::Ok
    }
}

fn url(s: &str) -> Verdict {
    match as_written(s, MAX_URL) {
        Verdict::Ok => {}
        v => return v,
    }
    let Ok(u) = Url::parse(s) else {
        return Verdict::Format;
    };
    // Its form, then its serialized length (`docs/BATCH_SPEC.md` 1.2).
    if !(http_scheme(&u) && no_userinfo(&u)) {
        Verdict::Format
    } else if u.as_str().len() as u64 > MAX_URL {
        Verdict::Range
    } else {
        Verdict::Ok
    }
}

fn endpoint(s: &str) -> Verdict {
    match as_written(s, MAX_ENDPOINT) {
        Verdict::Ok => {}
        v => return v,
    }
    let Ok(u) = Url::parse(s) else {
        return Verdict::Format;
    };
    // Its form, then its serialized length (`docs/BATCH_SPEC.md` 1.6).
    if !(http_scheme(&u)
        && no_userinfo(&u)
        && u.query().is_none()
        && u.fragment().is_none()
        && matches!(u.path(), "" | "/"))
    {
        Verdict::Format
    } else if u.as_str().len() as u64 > MAX_ENDPOINT {
        Verdict::Range
    } else {
        Verdict::Ok
    }
}

/// Whether an endpoint's host is an IP address (the `path_style` rule).
pub(crate) fn endpoint_host_is_ip(s: &str) -> bool {
    Url::parse(s).is_ok_and(|u| matches!(u.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_))))
}

/// Reads `n` ASCII digits at `*at` as a number.
fn digits(b: &[u8], at: &mut usize, n: usize) -> Option<u32> {
    let s = b.get(*at..*at + n)?;
    let mut v = 0u32;
    for &d in s {
        if !d.is_ascii_digit() {
            return None;
        }
        v = v * 10 + u32::from(d - b'0');
    }
    *at += n;
    Some(v)
}

fn byte(b: &[u8], at: &mut usize, want: &[u8]) -> Option<u8> {
    let c = *b.get(*at)?;
    if want.contains(&c) {
        *at += 1;
        Some(c)
    } else {
        None
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        _ => 28,
    }
}

/// Whether `hour:minute` at `offset` east of UTC is 23:59 in UTC, the one
/// minute a leap second ends (RFC 3339 5.7).
fn leap_minute(hour: u32, minute: u32, offset: i32) -> bool {
    let local = i32::try_from(hour * 60 + minute).unwrap_or(i32::MAX);
    (local - offset).rem_euclid(24 * 60) == 23 * 60 + 59
}

/// An RFC 3339 `date-time` (section 5.6), its form only: every field in
/// its range, the day within its month, `T` and `Z` in either case, a
/// fraction of any length, and second 60 only where a leap second can be
/// (section 5.7): at 23:59:60 once converted to UTC by its offset. Its
/// conversion is preflight's (`docs/plans/batch.md` 2.11 step 6).
pub(crate) fn date_time(s: &str) -> Option<DateTime> {
    let b = s.as_bytes();
    let at = &mut 0;
    let year = digits(b, at, 4)?;
    byte(b, at, b"-")?;
    let month = digits(b, at, 2)?;
    byte(b, at, b"-")?;
    let day = digits(b, at, 2)?;
    byte(b, at, b"Tt")?;
    let hour = digits(b, at, 2)?;
    byte(b, at, b":")?;
    let minute = digits(b, at, 2)?;
    byte(b, at, b":")?;
    let second = digits(b, at, 2)?;
    let mut nanos = 0u32;
    if byte(b, at, b".").is_some() {
        let start = *at;
        let mut scale = 100_000_000u32;
        while let Some(&d) = b.get(*at).filter(|d| d.is_ascii_digit()) {
            nanos += u32::from(d - b'0') * scale;
            scale /= 10;
            *at += 1;
        }
        if *at == start {
            return None;
        }
    }
    let offset_minutes = match byte(b, at, b"Zz+-")? {
        b'Z' | b'z' => 0,
        sign => {
            let oh = digits(b, at, 2)?;
            byte(b, at, b":")?;
            let om = digits(b, at, 2)?;
            if oh > 23 || om > 59 {
                return None;
            }
            let m = i32::try_from(oh * 60 + om).ok()?;
            if sign == b'-' { -m } else { m }
        }
    };
    let whole = *at == b.len();
    let ranges = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour <= 23
        && minute <= 59
        && (second <= 59 || (second == 60 && leap_minute(hour, minute, offset_minutes)));
    (whole && ranges).then_some(DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        nanos,
        offset_minutes,
    })
}

#[cfg(test)]
mod tests {
    use super::{Verdict, check, date_time, is_header_value, is_token, written_host_len};
    use crate::spec::fields::Format;

    #[test]
    fn date_times() {
        for ok in [
            "2026-11-11T12:00:00Z",
            "2026-11-11t12:00:00z",
            "2024-02-29T23:59:59.123456789123+05:30",
            "2016-12-31T23:59:60Z",
            "2017-01-01T05:29:60.5+05:30",
            "2016-12-31T15:59:60-08:00",
            "2026-01-31T00:00:00-23:59",
        ] {
            assert!(date_time(ok).is_some(), "{ok}");
        }
        for bad in [
            "",
            "2026-11-11",
            "2026-11-11 12:00:00Z",
            "2026-11-11T12:00:00",
            "2025-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-11-11T24:00:00Z",
            "2026-11-11T12:60:00Z",
            "2026-11-11T12:00:61Z",
            "2026-11-11T12:00:60Z",
            "2016-12-31T23:59:60+01:00",
            "2016-12-31T23:58:60Z",
            "2017-01-01T05:30:60+05:30",
            "2026-11-11T12:00:00.Z",
            "2026-11-11T12:00:00+24:00",
            "2026-11-11T12:00:00+0530",
            "2026-11-11T12:00:00Zx",
            "+2026-11-11T12:00:00Z",
        ] {
            assert!(date_time(bad).is_none(), "{bad}");
        }
        let d = date_time("2026-11-11T12:34:56.5-01:30").unwrap();
        assert_eq!(
            (d.hour, d.minute, d.second, d.nanos, d.offset_minutes),
            (12, 34, 56, 500_000_000, -90)
        );
    }

    #[test]
    fn urls() {
        fn url(s: &str) -> Verdict {
            check(Format::Url, s)
        }
        assert_eq!(url("https://example.net/a?b=c"), Verdict::Ok);
        assert_eq!(url("http://example.net"), Verdict::Ok);
        assert_eq!(url("ftp://example.net/"), Verdict::Format);
        assert_eq!(url("https://u:p@example.net/"), Verdict::Format);
        assert_eq!(url("https://u@example.net/"), Verdict::Format);
        assert_eq!(url("not a url"), Verdict::Format);
        // The cap is on the serialization too: a space becomes `%20`.
        let path = " ".repeat(5462);
        assert_eq!(url(&format!("https://e.net/{path}x")), Verdict::Range);
        // As written: ASCII, then the cap, then the host's, all before
        // the parse (`not a url` past the cap is `range`).
        assert_eq!(url("https://\u{e9}.net/"), Verdict::Format);
        assert_eq!(url("https://e.net/\u{e9}"), Verdict::Format);
        let long = "a".repeat(16 * 1024);
        assert_eq!(url(&format!("not a url {long}")), Verdict::Range);
        let host = "a".repeat(253);
        assert_eq!(url(&format!("https://{host}/")), Verdict::Ok);
        assert_eq!(url(&format!("https://{host}a/")), Verdict::Range);
        assert_eq!(url(&format!("https://u@{host}:443/")), Verdict::Format);
        assert_eq!(url(&format!("https://u@{host}a:443/")), Verdict::Range);
        assert_eq!(url(&format!("https:/\t/{host}a/")), Verdict::Range);
        assert_eq!(url(&format!("https:\\\\{host}a/")), Verdict::Range);
        assert_eq!(url(&format!("https:{host}a")), Verdict::Range);
        let fits = "a".repeat(16 * 1024 - "https://e.net/".len());
        assert_eq!(url(&format!("https://e.net/{fits}")), Verdict::Ok);
        assert_eq!(url(&format!("https://e.net/{fits}a")), Verdict::Range);
        let ep = |s: &str| check(Format::Endpoint, s);
        assert_eq!(ep("https://s3.example.net:9000"), Verdict::Ok);
        assert_eq!(ep("http://127.0.0.1:9000/"), Verdict::Ok);
        assert_eq!(ep("https://s3.example.net/x"), Verdict::Format);
        assert_eq!(ep("https://s3.example.net/?q"), Verdict::Format);
        assert_eq!(ep("https://s3.example.net/#f"), Verdict::Format);
        assert_eq!(ep("https://s3.\u{e9}.net"), Verdict::Format);
        let long = "a".repeat(1024);
        assert_eq!(ep(&format!("https://e.net/{long}")), Verdict::Range);
        // The form before the serialized cap (G2).
        let spaces = " ".repeat(340);
        assert_eq!(ep(&format!("https://e.net/{spaces}x")), Verdict::Format);
        assert_eq!(ep(&format!("https://u@e.net/{spaces}")), Verdict::Format);
    }

    #[test]
    fn a_host_as_written() {
        assert_eq!(written_host_len("https://example.net/a"), 11);
        assert_eq!(written_host_len("https://u:p@example.net:9000/a"), 11);
        assert_eq!(written_host_len("https://[::1]:9000"), 5);
        // Any `[` opens brackets, mid-host too, as `url` reads them: the
        // `:` inside is the host's, the one after `]` the port's.
        assert_eq!(written_host_len("https://a[b:c]d:9000/"), 7);
        assert_eq!(written_host_len("https://a]b:c/"), 3);
        assert_eq!(written_host_len("https:\\\\ab?c"), 2);
        assert_eq!(written_host_len("https:a\tb\nc#d"), 3);
        assert_eq!(written_host_len("no colon"), 0);
        // A trailing space the parse trims is still counted (conservative).
        assert_eq!(written_host_len("https://e.net "), 6);
        assert_eq!(written_host_len("https://e.net\x01"), 6);
    }

    #[test]
    fn small_forms() {
        assert_eq!(check(Format::RunId, "a.b-c_9"), Verdict::Ok);
        assert_eq!(check(Format::RunId, ".."), Verdict::Format);
        assert_eq!(check(Format::RunId, "a/b"), Verdict::Format);
        assert_eq!(check(Format::EnvName, "_A9"), Verdict::Ok);
        assert_eq!(check(Format::EnvName, "9A"), Verdict::Format);
        assert_eq!(check(Format::EnvName, "A=B"), Verdict::Format);
        assert_eq!(check(Format::Id, "a\u{7f}"), Verdict::Format);
        assert_eq!(check(Format::Id, "a\u{80}"), Verdict::Format);
        assert_eq!(check(Format::Id, "a\u{9f}"), Verdict::Format);
        assert_eq!(check(Format::Id, "a\u{a0}\u{e9}"), Verdict::Ok);
        assert_eq!(check(Format::RunId, "."), Verdict::Format);
        assert_eq!(check(Format::RunId, "..."), Verdict::Ok);
        assert_eq!(check(Format::RunId, ".a"), Verdict::Ok);
        assert_eq!(check(Format::Prefix, "a\n/"), Verdict::Ok);
        assert_eq!(check(Format::Prefix, "a/"), Verdict::Ok);
        assert_eq!(check(Format::Prefix, "a"), Verdict::Format);
        assert_eq!(check(Format::AbsPath, "/a\0"), Verdict::Format);
        assert!(is_token("X-Api_Key.1"));
        assert!(!is_token("X Key"));
        assert!(!is_token(""));
        assert!(is_header_value("a\tb c\u{e9}"));
        assert!(!is_header_value("a\rb"));
        assert!(!is_header_value("a\u{7f}"));
    }
}
