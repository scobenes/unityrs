//! A bounded cursor, without serde's infallible string and collection growth.
use super::{
    Arc, Builtin, ExtrasNaming, GltfError, Inactive, Lod, Meter, MeteredVec, Profile, RootNaming,
    StaticBatch, Text,
};
use crate::validate::invalid;
type Result<T> = std::result::Result<T, GltfError>;
struct Read<'a> {
    bytes: &'a [u8],
    at: usize,
    meter: &'a Arc<Meter>,
}
impl Read<'_> {
    fn ws(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\n' | b'\t' | b'\r'))
        {
            self.at += 1;
        }
    }
    fn take(&mut self, b: u8) -> bool {
        self.ws();
        if self.bytes.get(self.at) == Some(&b) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn need(&mut self, b: u8) -> Result<()> {
        if self.take(b) {
            Ok(())
        } else {
            Err(invalid("profile syntax/type"))
        }
    }
    fn string(&mut self) -> Result<Text> {
        self.need(b'"')?;
        let mut out = MeteredVec::new(self.meter);
        loop {
            let b = *self
                .bytes
                .get(self.at)
                .ok_or_else(|| invalid("profile string"))?;
            self.at += 1;
            match b {
                b'"' => break,
                0..=31 => return Err(invalid("profile string control")),
                b'\\' => {
                    let escaped = *self
                        .bytes
                        .get(self.at)
                        .ok_or_else(|| invalid("profile escape"))?;
                    self.at += 1;
                    match escaped {
                        b'"' | b'\\' | b'/' => out.push(escaped)?,
                        b'b' => out.push(8)?,
                        b'f' => out.push(12)?,
                        b'n' => out.push(b'\n')?,
                        b'r' => out.push(b'\r')?,
                        b't' => out.push(b'\t')?,
                        b'u' => {
                            let mut c = self.hex()?;
                            if (0xd800..=0xdbff).contains(&c) {
                                // No whitespace is legal within a surrogate escape pair.
                                if self.bytes.get(self.at..self.at.saturating_add(2))
                                    != Some(b"\\u")
                                {
                                    return Err(invalid("profile surrogate"));
                                }
                                self.at += 2;
                                let lo = self.hex()?;
                                if !(0xdc00..=0xdfff).contains(&lo) {
                                    return Err(invalid("profile surrogate"));
                                }
                                c = 0x10000 + ((c - 0xd800) << 10) + lo - 0xdc00;
                            }
                            let c = char::from_u32(c).ok_or_else(|| invalid("profile unicode"))?;
                            let mut buf = [0; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes())?;
                        }
                        _ => return Err(invalid("profile escape")),
                    }
                }
                _ => out.push(b)?,
            }
        }
        std::str::from_utf8(out.as_slice()).map_err(|_| invalid("profile UTF-8"))?;
        Ok(Text(out))
    }
    fn hex(&mut self) -> Result<u32> {
        let mut n = 0;
        for _ in 0..4 {
            let b = *self
                .bytes
                .get(self.at)
                .ok_or_else(|| invalid("profile unicode"))?;
            self.at += 1;
            n = n * 16
                + char::from(b)
                    .to_digit(16)
                    .ok_or_else(|| invalid("profile unicode"))?;
        }
        Ok(n)
    }
    fn uint(&mut self) -> Result<u32> {
        self.ws();
        let start = self.at;
        let mut n = 0u32;
        while let Some(&b @ b'0'..=b'9') = self.bytes.get(self.at) {
            n = n
                .checked_mul(10)
                .and_then(|n| n.checked_add(u32::from(b - b'0')))
                .ok_or_else(|| invalid("profile integer"))?;
            self.at += 1;
        }
        if self.at == start || (self.at > start + 1 && self.bytes.get(start) == Some(&b'0')) {
            return Err(invalid("profile integer"));
        }
        Ok(n)
    }
    fn boolean(&mut self) -> Result<bool> {
        self.ws();
        for (word, value) in [(b"true".as_slice(), true), (b"false".as_slice(), false)] {
            if self.bytes.get(self.at..self.at.saturating_add(word.len())) == Some(word) {
                self.at += word.len();
                return Ok(value);
            }
        }
        Err(invalid("profile boolean"))
    }
    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T>) -> Result<MeteredVec<T>> {
        self.need(b'[')?;
        let mut out = MeteredVec::new(self.meter);
        if self.take(b']') {
            return Ok(out);
        }
        loop {
            out.push(item(self)?)?;
            if self.take(b']') {
                return Ok(out);
            }
            self.need(b',')?;
        }
    }
    fn object(
        &mut self,
        keys: &[&[u8]],
        mut field: impl FnMut(&mut Self, usize) -> Result<()>,
    ) -> Result<u64> {
        self.need(b'{')?;
        let mut seen = 0u64;
        if self.take(b'}') {
            return Ok(seen);
        }
        loop {
            let key = self.string()?;
            let id = keys
                .iter()
                .position(|k| *k == key.as_bytes())
                .ok_or_else(|| invalid("unknown profile key"))?;
            let mask = 1u64 << id;
            if seen & mask != 0 {
                return Err(invalid("duplicate profile key"));
            }
            seen |= mask;
            self.need(b':')?;
            field(self, id)?;
            if self.take(b'}') {
                return Ok(seen);
            }
            self.need(b',')?;
        }
    }
    fn choice<T: Copy>(&mut self, choices: &[(&[u8], T)]) -> Result<T> {
        let s = self.string()?;
        choices
            .iter()
            .find(|(k, _)| *k == s.as_bytes())
            .map(|(_, v)| *v)
            .ok_or_else(|| invalid("profile enum"))
    }
}
#[allow(clippy::too_many_lines)]
pub(super) fn parse(bytes: &[u8], meter: &Arc<Meter>) -> Result<Profile> {
    meter.count_decode((bytes.len() as u64).saturating_mul(32))?;
    let mut r = Read {
        bytes,
        at: 0,
        meter,
    };
    let mut p = Profile::new(meter);
    let seen = r.object(
        &[
            b"profile_version",
            b"root_naming",
            b"layer_skips",
            b"shader_rules",
            b"inactive",
            b"material_property_allowlist",
            b"scene_roots",
            b"exclusions",
            b"builtin",
            b"extras_naming",
            b"export_metadata",
            b"lod",
            b"static_batch",
            b"normalise",
            b"clip_states",
        ],
        |r, id| {
            match id {
                0 => {
                    if r.uint()? != 1 {
                        return Err(invalid("profile version"));
                    }
                }
                1 => {
                    p.root_naming = r.choice(&[
                        (b"name", RootNaming::Name),
                        (b"path_id", RootNaming::PathId),
                    ])?;
                }
                2 => {
                    p.layer_skips = r.list(|r| {
                        let n = r.uint()?;
                        if n > 31 {
                            Err(invalid("profile layer"))
                        } else {
                            Ok(n)
                        }
                    })?;
                }
                3 => {
                    r.object(&[b"drawable", b"additive", b"skip"], |r, id| {
                        let v = r.list(Read::string)?;
                        match id {
                            0 => p.shader_rules.drawable = v,
                            1 => p.shader_rules.additive = v,
                            _ => p.shader_rules.skip = v,
                        }
                        Ok(())
                    })?;
                }
                4 => {
                    p.inactive = r.choice(&[
                        (b"skip", Inactive::Skip),
                        (b"include", Inactive::Include),
                        (b"include_hidden", Inactive::IncludeHidden),
                    ])?;
                }
                5 => p.material_property_allowlist = r.list(Read::string)?,
                6 => p.scene_roots = r.list(Read::string)?,
                7 => p.exclusions = r.list(Read::string)?,
                8 => {
                    p.builtin = r.choice(&[
                        (b"placeholder", Builtin::Placeholder),
                        (b"resolve", Builtin::Resolve),
                        (b"procedural", Builtin::Procedural),
                    ])?;
                }
                9 => {
                    p.extras_naming = r.choice(&[
                        (b"unity", ExtrasNaming::Unity),
                        (b"none", ExtrasNaming::None),
                    ])?;
                }
                10 => {
                    r.object(&[b"generator", b"source", b"input_sha256"], |r, id| {
                        let v = r.string()?;
                        match id {
                            0 => p.export_metadata.generator = Some(v),
                            1 => p.export_metadata.source = Some(v),
                            _ => {
                                if v.as_bytes().len() != 64
                                    || !v.as_bytes().iter().all(u8::is_ascii_hexdigit)
                                {
                                    return Err(invalid("profile SHA-256"));
                                }
                                p.export_metadata.input_sha256 = Some(v);
                            }
                        }
                        Ok(())
                    })?;
                }
                11 => p.lod = r.choice(&[(b"lod0", Lod::Lod0), (b"all", Lod::All)])?,
                12 => {
                    p.static_batch = r.choice(&[
                        (b"unbatch", StaticBatch::Unbatch),
                        (b"keep_combined", StaticBatch::KeepCombined),
                    ])?;
                }
                13 => p.normalise = r.boolean()?,
                _ => {
                    r.need(b'[')?;
                    if !r.take(b']') {
                        return Err(invalid("clip states require P2b"));
                    }
                }
            }
            Ok(())
        },
    )?;
    if seen & 1 == 0 {
        return Err(invalid("missing profile_version"));
    }
    r.ws();
    if r.at != bytes.len() {
        return Err(invalid("profile trailing data"));
    }
    Ok(p)
}
