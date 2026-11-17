//! Role-framed, content-bound cursors. The checkpoint is an object identity,
//! not a count of objects that a later read might interpret differently.
use super::{Args, Document, Error, Result, document};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, sync::Arc};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_dump::{
    json,
    text::{Sink, Text},
};

pub struct Fingerprint(Sha256);
impl Fingerprint {
    pub fn new(args: &Args<'_>, meter: &Arc<Meter>, provider: Option<&[u8; 32]>) -> Result<Self> {
        let mut digest = Self(Sha256::new());
        digest.field(b"protocol", b"unityrs.inspect.cursor.2", meter)?;
        for (index, path) in args.inputs.iter().enumerate() {
            digest.field(b"input-index", &(index as u64).to_le_bytes(), meter)?;
            digest.field(b"input-path", path, meter)?;
        }
        for (key, value) in &args.options {
            if !matches!(*key, "--cursor" | "--limit") {
                digest.field(b"option-name", key.as_bytes(), meter)?;
                digest.field(b"option-value", value, meter)?;
            }
        }
        if let Some(bytes) = provider {
            digest.field(b"tpk-digest", bytes, meter)?;
        }
        Ok(digest)
    }
    pub fn field(&mut self, role: &[u8], bytes: &[u8], meter: &Arc<Meter>) -> Result<()> {
        meter.count_decode(
            (role.len() as u64)
                .saturating_add(bytes.len() as u64)
                .saturating_add(16),
        )?;
        self.0.update((role.len() as u64).to_le_bytes());
        self.0.update(role);
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
        Ok(())
    }
    pub fn finish(self) -> String {
        json::hex(&self.0.finalize())
    }
}

fn bad() -> Error {
    Error::new(
        "bad_cursor",
        "cursor refused: inputs changed or listing options changed, or token is invalid",
    )
}
pub struct Resume<'a> {
    fingerprint: &'a str,
    pub key: Key,
}
impl<'a> Resume<'a> {
    pub fn parse(args: &Args<'a>, meter: &Arc<Meter>) -> Result<Option<Self>> {
        let Some(raw) = args.get("--cursor") else {
            return Ok(None);
        };
        meter.count_decode(raw.len() as u64)?;
        let raw = std::str::from_utf8(raw).map_err(|_| bad())?;
        let (payload, checksum) = raw.rsplit_once('.').ok_or_else(bad)?;
        if json::sha256_hex(payload.as_bytes()) != checksum {
            return Err(bad());
        }
        let mut parts = payload.splitn(5, '.');
        let (Some(fingerprint), Some(index), Some(file_index), Some(id), Some(hex)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(bad());
        };
        if fingerprint.len() != 64 || hex.len() % 2 != 0 {
            return Err(bad());
        }
        let mut file = MeteredVec::with_capacity(meter, hex.len() / 2)?;
        for &[a, b] in hex.as_bytes().as_chunks::<2>().0 {
            let byte = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            };
            file.push((byte(a).ok_or_else(bad)? << 4) | byte(b).ok_or_else(bad)?)?;
        }
        Ok(Some(Self {
            fingerprint,
            key: Key {
                index: index.parse().map_err(|_| bad())?,
                file_index: file_index.parse().map_err(|_| bad())?,
                path_id: id.parse().map_err(|_| bad())?,
                file,
            },
        }))
    }
    pub fn verify(&self, actual: &str, found: bool) -> Result<()> {
        if self.fingerprint != actual || !found {
            return Err(bad());
        }
        Ok(())
    }
}

pub struct Key {
    index: usize,
    file_index: usize,
    file: MeteredVec<u8>,
    path_id: i64,
}
impl Key {
    pub fn new(
        index: usize,
        file_index: usize,
        file: &[u8],
        path_id: i64,
        meter: &Arc<Meter>,
    ) -> Result<Self> {
        let mut bytes = MeteredVec::new(meter);
        bytes.extend_from_slice(file)?;
        Ok(Self {
            index,
            file_index,
            file: bytes,
            path_id,
        })
    }
    pub fn matches(&self, index: usize, file_index: usize, file: &[u8], path_id: i64) -> bool {
        self.index == index
            && self.file_index == file_index
            && self.file.as_slice() == file
            && self.path_id == path_id
    }
    pub fn token(&self, fingerprint: &str, meter: &Arc<Meter>) -> Result<Document> {
        let mut text = Text::on(meter);
        let _ = write!(
            text,
            "{fingerprint}.{}.{}.{}.",
            self.index, self.file_index, self.path_id
        );
        json::push_hex(&mut text, self.file.as_slice());
        let payload = document(text)?;
        meter.count_decode(payload.json.len() as u64)?;
        let mut text = Text::on(meter);
        text.push_str(&payload.json);
        text.push('.');
        text.push_str(&json::sha256_hex(payload.json.as_bytes()));
        document(text)
    }
}
