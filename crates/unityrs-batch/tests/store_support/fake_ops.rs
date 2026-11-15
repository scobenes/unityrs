//! S3-shaped state transitions for the independent loopback peer.
#![allow(dead_code)]
use super::fake::{Object, Operation, State, Upload};
use hyper::{HeaderMap, Method, Uri};
use md5::{Digest, Md5};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub struct Input {
    pub operation: Operation,
    pub bucket: String,
    pub key: String,
    pub query: BTreeMap<String, String>,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Answer {
    pub fn bytes(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }
    pub fn xml(body: String) -> Self {
        let mut answer = Self::bytes(200, body.into_bytes());
        answer
            .headers
            .push(("content-type".into(), "application/xml".into()));
        answer
    }
    pub fn error(status: u16, code: &str) -> Self {
        let mut result = Self::xml(format!(
            "<Error><Code>{}</Code><Message>fake store</Message></Error>",
            xml(code)
        ));
        result.status = status;
        result
    }
}

pub fn identify(method: &Method, uri: &Uri, headers: HeaderMap, path_style: bool) -> Option<Input> {
    let decoded = decode(uri.path())?;
    let path = decoded.strip_prefix('/')?;
    let (bucket, key) = if path_style {
        path.split_once('/').unwrap_or((path, ""))
    } else {
        let host = headers.get("host")?.to_str().ok()?;
        (host.split('.').next()?, path)
    };
    let query: BTreeMap<_, _> =
        url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
    let operation = match method.as_str() {
        "PUT" if query.contains_key("uploadId") => Operation::Part,
        "PUT" => Operation::Put,
        "POST" if query.contains_key("uploads") => Operation::Create,
        "POST" if query.contains_key("uploadId") => Operation::Complete,
        "DELETE" if query.contains_key("uploadId") => Operation::Abort,
        "GET" if query.get("list-type").is_some_and(|value| value == "2") => Operation::List,
        "GET" => Operation::Get,
        "HEAD" => Operation::Head,
        _ => return None,
    };
    Some(Input {
        operation,
        bucket: bucket.into(),
        key: key.into(),
        query,
        headers,
        body: Vec::new(),
    })
}

pub fn apply(state: &mut State, input: &Input) -> Answer {
    if input.bucket != state.bucket {
        return Answer::error(404, "NoSuchBucket");
    }
    let identity = (input.bucket.clone(), input.key.clone());
    match input.operation {
        Operation::Put => {
            if let Some(answer) = precondition(input, state.objects.get(&identity)) {
                return answer;
            }
            let object = Object {
                bytes: input.body.clone(),
                etag: digest(&input.body),
                content_type: content_type(&input.headers),
            };
            let tag = object.etag.clone();
            state.objects.insert(identity, object);
            let mut answer = Answer::bytes(200, Vec::new());
            answer.headers.push(("etag".into(), tag));
            answer
        }
        Operation::Get | Operation::Head => get(input, state.objects.get(&identity)),
        Operation::List => list(state, input),
        Operation::Create => {
            state.upload_sequence += 1;
            let id = format!("upload-{}", state.upload_sequence);
            state.uploads.insert(
                id.clone(),
                Upload {
                    bucket: input.bucket.clone(),
                    key: input.key.clone(),
                    parts: BTreeMap::new(),
                    content_type: content_type(&input.headers),
                },
            );
            Answer::xml(format!(
                "<InitiateMultipartUploadResult><Bucket>{}</Bucket><Key>{}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>",
                xml(&input.bucket),
                xml(&input.key)
            ))
        }
        Operation::Part => {
            let Some(id) = input.query.get("uploadId") else {
                return Answer::error(400, "InvalidArgument");
            };
            let Some(upload) = state
                .uploads
                .get_mut(id)
                .filter(|upload| upload.bucket == input.bucket && upload.key == input.key)
            else {
                return Answer::error(404, "NoSuchUpload");
            };
            let Some(part) = input
                .query
                .get("partNumber")
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=10000).contains(n))
            else {
                return Answer::error(400, "InvalidArgument");
            };
            upload.parts.insert(part, input.body.clone());
            let mut answer = Answer::bytes(200, Vec::new());
            answer.headers.push(("etag".into(), digest(&input.body)));
            answer
        }
        Operation::Complete => complete(state, input),
        Operation::Abort => {
            let Some(id) = input.query.get("uploadId") else {
                return Answer::error(400, "InvalidArgument");
            };
            if !state
                .uploads
                .get(id)
                .is_some_and(|upload| upload.bucket == input.bucket && upload.key == input.key)
            {
                return Answer::error(404, "NoSuchUpload");
            }
            state.uploads.remove(id);
            Answer::bytes(204, Vec::new())
        }
    }
}

fn get(input: &Input, object: Option<&Object>) -> Answer {
    let Some(object) = object else {
        return Answer::error(404, "NoSuchKey");
    };
    if let Some(answer) = precondition(input, Some(object)) {
        return answer;
    }
    let mut answer = Answer::bytes(
        200,
        if input.operation == Operation::Head {
            Vec::new()
        } else {
            object.bytes.clone()
        },
    );
    answer.headers.extend([
        ("etag".into(), object.etag.clone()),
        (
            "last-modified".into(),
            "Thu, 01 Jan 2026 00:00:00 GMT".into(),
        ),
        ("content-length".into(), object.bytes.len().to_string()),
    ]);
    if let Some(kind) = &object.content_type {
        answer.headers.push(("content-type".into(), kind.clone()));
    }
    answer
}

fn content_type(headers: &HeaderMap) -> Option<String> {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(Into::into)
}
fn precondition(input: &Input, object: Option<&Object>) -> Option<Answer> {
    if let Some(condition) = input
        .headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        && object.is_some_and(|object| condition == "*" || condition == object.etag)
    {
        return Some(Answer::error(
            if matches!(input.operation, Operation::Get | Operation::Head) {
                304
            } else {
                412
            },
            "PreconditionFailed",
        ));
    }
    if let Some(condition) = input.headers.get("if-match").and_then(|v| v.to_str().ok()) {
        let Some(object) = object else {
            return Some(Answer::error(404, "NoSuchKey"));
        };
        if condition != "*" && condition != object.etag {
            return Some(Answer::error(412, "PreconditionFailed"));
        }
    }
    None
}

fn list(state: &State, input: &Input) -> Answer {
    let prefix = input.query.get("prefix").map_or("", String::as_str);
    let delimiter = input.query.get("delimiter").map_or("", String::as_str);
    let max = input
        .query
        .get("max-keys")
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(1000)
        .min(1000);
    let start = input
        .query
        .get("continuation-token")
        .and_then(|n| n.strip_prefix("offset:"))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    let mut entries = BTreeMap::new();
    let mut prefixes = BTreeSet::new();
    for ((bucket, key), object) in &state.objects {
        if bucket != &input.bucket || !key.starts_with(prefix) {
            continue;
        }
        if input
            .query
            .get("start-after")
            .is_some_and(|after| key <= after)
        {
            continue;
        }
        if !delimiter.is_empty()
            && let Some(position) = key[prefix.len()..].find(delimiter)
        {
            prefixes.insert(key[..prefix.len() + position + delimiter.len()].to_owned());
            continue;
        }
        entries.insert(key.as_str(), Some(object));
    }
    for prefix in &prefixes {
        entries.insert(prefix, None);
    }
    let total = entries.len();
    let truncated = max != 0 && start.saturating_add(max) < total;
    let mut body = format!(
        "<ListBucketResult><Name>{}</Name><Prefix>{}</Prefix><MaxKeys>{max}</MaxKeys><IsTruncated>{truncated}</IsTruncated>",
        xml(&input.bucket),
        xml(prefix)
    );
    for (key, object) in entries.into_iter().skip(start).take(max) {
        let encoded;
        let key = if input
            .query
            .get("encoding-type")
            .is_some_and(|value| value == "url")
        {
            encoded = super::sign::encode(key);
            encoded.as_str()
        } else {
            key
        };
        if let Some(object) = object {
            write!(&mut body,"<Contents><Key>{}</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>{}</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",xml(key),xml(&object.etag),object.bytes.len()).unwrap();
        } else {
            write!(
                &mut body,
                "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
                xml(key)
            )
            .unwrap();
        }
    }
    if truncated {
        write!(
            &mut body,
            "<NextContinuationToken>offset:{}</NextContinuationToken>",
            start + max
        )
        .unwrap();
    }
    if input
        .query
        .get("encoding-type")
        .is_some_and(|value| value == "url")
    {
        body.push_str("<EncodingType>url</EncodingType>");
    }
    body.push_str("</ListBucketResult>");
    Answer::xml(body)
}

#[derive(Deserialize)]
struct Completion {
    #[serde(rename = "Part", default)]
    parts: Vec<CompletedPart>,
}
#[derive(Deserialize)]
struct CompletedPart {
    #[serde(rename = "PartNumber")]
    number: u32,
    #[serde(rename = "ETag")]
    etag: String,
}
fn complete(state: &mut State, input: &Input) -> Answer {
    let Some(id) = input.query.get("uploadId") else {
        return Answer::error(400, "InvalidArgument");
    };
    let Some(upload) = state
        .uploads
        .get(id)
        .filter(|upload| upload.bucket == input.bucket && upload.key == input.key)
    else {
        return Answer::error(404, "NoSuchUpload");
    };
    let Some(completion) = std::str::from_utf8(&input.body)
        .ok()
        .and_then(|text| quick_xml::de::from_str::<Completion>(text).ok())
    else {
        return Answer::error(400, "MalformedXML");
    };
    if completion.parts.is_empty()
        || !completion
            .parts
            .windows(2)
            .all(|p| p[0].number < p[1].number)
    {
        return Answer::error(400, "InvalidPartOrder");
    }
    let mut bytes = Vec::new();
    let mut digests = Vec::new();
    for part in &completion.parts {
        let Some(body) = upload.parts.get(&part.number) else {
            return Answer::error(400, "InvalidPart");
        };
        if part.etag.trim_matches('"') != digest(body).trim_matches('"') {
            return Answer::error(400, "InvalidPart");
        }
        bytes.extend_from_slice(body);
        digests.extend_from_slice(&Md5::digest(body));
    }
    let etag = format!(
        "\"{}-{}\"",
        super::sign::hex(&Md5::digest(digests)),
        completion.parts.len()
    );
    let object = Object {
        bytes,
        etag: etag.clone(),
        content_type: upload.content_type.clone(),
    };
    state
        .objects
        .insert((input.bucket.clone(), input.key.clone()), object);
    state.uploads.remove(id);
    Answer::xml(format!(
        "<CompleteMultipartUploadResult><Location>http://fake/</Location><Bucket>{}</Bucket><Key>{}</Key><ETag>{}</ETag></CompleteMultipartUploadResult>",
        xml(&input.bucket),
        xml(&input.key),
        xml(&etag)
    ))
}

pub fn digest(bytes: &[u8]) -> String {
    format!("\"{}\"", super::sign::hex(&Md5::digest(bytes)))
}
pub(super) fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn decode(text: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            u8::try_from(
                char::from(input.next()?).to_digit(16)? * 16
                    + char::from(input.next()?).to_digit(16)?,
            )
            .ok()?
        } else {
            byte
        });
    }
    String::from_utf8(bytes).ok()
}
