//! Independent test-only `SigV4` verifier: SHA-256 HMAC without the signer's ring.
#![allow(dead_code)]
use hyper::{HeaderMap, Method, Uri};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Credentials {
    pub access_key: String,
    pub secret: String,
    pub token: Option<String>,
    pub region: String,
    pub skew: Duration,
}
impl Default for Credentials {
    fn default() -> Self {
        Self {
            access_key: "AKIDEXAMPLE".into(),
            secret: "secret".into(),
            token: None,
            region: "us-east-1".into(),
            skew: Duration::from_mins(15),
        }
    }
}

pub fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = [0x36; 64];
    let mut outer = [0x5c; 64];
    for i in 0..64 {
        inner[i] ^= block[i];
        outer[i] ^= block[i];
    }
    let mut hash = Sha256::new();
    hash.update(inner);
    hash.update(message);
    let inside = hash.finalize();
    let mut hash = Sha256::new();
    hash.update(outer);
    hash.update(inside);
    hash.finalize().into()
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut out, "{byte:02x}").unwrap();
    }
    out
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

pub fn verify(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
    credentials: &Credentials,
) -> bool {
    verify_inner(method, uri, headers, body, credentials).is_some()
}

fn verify_inner(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
    c: &Credentials,
) -> Option<()> {
    let auth = header(headers, "authorization")?.strip_prefix("AWS4-HMAC-SHA256 ")?;
    let fields: std::collections::BTreeMap<_, _> = auth
        .split(',')
        .map(str::trim)
        .map(|part| part.split_once('='))
        .collect::<Option<_>>()?;
    if fields.len() != 3 {
        return None;
    }
    let credential = fields.get("Credential")?;
    let (access_key, scope) = credential.split_once('/')?;
    if access_key != c.access_key {
        return None;
    }
    let date = header(headers, "x-amz-date")?;
    let timestamp = timestamp(date)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if now.abs_diff(timestamp) > c.skew.as_secs() {
        return None;
    }
    let want_scope = format!("{}/{}/s3/aws4_request", date.get(..8)?, c.region);
    if scope != want_scope {
        return None;
    }
    if header(headers, "x-amz-security-token") != c.token.as_deref() {
        return None;
    }
    let payload = header(headers, "x-amz-content-sha256")?;
    if payload != "UNSIGNED-PAYLOAD" && payload != hex(&Sha256::digest(body)) {
        return None;
    }
    let signed = *fields.get("SignedHeaders")?;
    let canonical_headers = canonical_headers(headers, signed)?;
    let mut query: Vec<_> = url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
        .map(|(k, v)| (encode(&k), encode(&v)))
        .collect();
    query.sort();
    let query = query
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        uri.path(),
        query,
        canonical_headers,
        signed,
        payload
    );
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let date_key = hmac(
        format!("AWS4{}", c.secret).as_bytes(),
        date.get(..8)?.as_bytes(),
    );
    let region_key = hmac(&date_key, c.region.as_bytes());
    let service_key = hmac(&region_key, b"s3");
    let signing_key = hmac(&service_key, b"aws4_request");
    let expected = hex(&hmac(&signing_key, to_sign.as_bytes()));
    let signature = fields.get("Signature")?;
    if expected.len() != signature.len() {
        return None;
    }
    expected
        .bytes()
        .zip(signature.bytes())
        .fold(0, |diff, (a, b)| diff | (a ^ b))
        .eq(&0)
        .then_some(())
}

fn canonical_headers(headers: &HeaderMap, signed: &str) -> Option<String> {
    let names: Vec<_> = signed.split(';').collect();
    if !names.windows(2).all(|pair| pair[0] < pair[1])
        || !names.contains(&"host")
        || !names.contains(&"x-amz-date")
    {
        return None;
    }
    let mut canonical_headers = String::new();
    for name in names {
        if name != name.to_ascii_lowercase() {
            return None;
        }
        let values: Option<Vec<_>> = headers
            .get_all(name)
            .iter()
            .map(|value| {
                Some(
                    value
                        .to_str()
                        .ok()?
                        .split_ascii_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .collect();
        let values = values?;
        if values.is_empty() {
            return None;
        }
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(&values.join(","));
        canonical_headers.push('\n');
    }
    Some(canonical_headers)
}

pub fn encode(text: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => write!(&mut out, "%{byte:02X}").unwrap(),
        }
    }
    out
}

fn timestamp(date: &str) -> Option<u64> {
    if date.len() != 16 || date.get(8..9)? != "T" || date.get(15..)? != "Z" {
        return None;
    }
    let number = |from, to| date.get(from..to)?.parse::<u64>().ok();
    let (year, month, day) = (number(0, 4)?, number(4, 6)?, number(6, 8)?);
    let (hour, minute, second) = (number(9, 11)?, number(11, 13)?, number(13, 15)?);
    if !(1970..=2100).contains(&year)
        || !(1..=12).contains(&month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let leap = |year| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > months[usize::try_from(month - 1).ok()?] {
        return None;
    }
    let mut days = 0;
    for past in 1970..year {
        days += if leap(past) { 366 } else { 365 };
    }
    days += months
        .iter()
        .take(usize::try_from(month - 1).ok()?)
        .sum::<u64>()
        + day
        - 1;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}
