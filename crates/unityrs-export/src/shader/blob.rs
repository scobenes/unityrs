// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py. See THIRD_PARTY_LICENSES.
// Ported from AssetStudio v0.16.47 (MIT): AssetStudioUtility/ShaderConverter.cs, ShaderSubProgramEntry's segmented table layout. See THIRD_PARTY_LICENSES.
//! Shader tables and subprograms: export/ShaderConverter.py:543-689.
use super::{Error, output::Output};
use std::{ops::Range, sync::Arc};
use unityrs_core::budget::DecodeBuf;
use unityrs_core::{
    limits::{Meter, MeteredVec},
    stream::Endian,
    stream::Reader,
};

#[derive(Debug)]
pub struct Program<'a> {
    subs: MeteredVec<SubProgram>,
    data: Data<'a>,
    selected: Option<MeteredVec<usize>>,
    capacity: usize,
}
#[derive(Debug)]
struct SubProgram {
    kind: i32,
    keywords: MeteredVec<Range<usize>>,
    local: MeteredVec<Range<usize>>,
    code: Range<usize>,
    segment: usize,
}
fn int(r: &mut Reader<'_>, meter: &Meter) -> Result<i32, Error> {
    meter.count_decode(4)?;
    r.read_i32().map_err(|_| Error::Struct)
}
fn keywords(
    r: &mut Reader<'_>,
    meter: &Arc<Meter>,
    collect: bool,
    strict: bool,
) -> Result<MeteredVec<Range<usize>>, Error> {
    let count = int(r, meter)?;
    if strict && count < 0 {
        return Err(Error::Struct);
    }
    let mut words = MeteredVec::new(meter);
    // Each item requires a real four-byte length. Never reserve the count.
    if i128::from(count.max(0)) * 4 > r.data().len() as i128 - r.position() {
        return Err(Error::Struct);
    }
    for _ in 0..count {
        let len = int(r, meter)?;
        if strict && (len < 0 || i128::from(len) > r.data().len() as i128 - r.position()) {
            return Err(Error::Struct);
        }
        let word = if len > 0 && i128::from(len) <= r.data().len() as i128 - r.position() {
            let word = r.read_range(i64::from(len)).map_err(|_| Error::Struct)?;
            meter.count_decode(word.len() as u64)?;
            r.align(4).map_err(|_| Error::Struct)?;
            word
        } else {
            0..0
        };
        if collect {
            words.push(word)?;
        }
    }
    Ok(words)
}
impl<'a> Program<'a> {
    /// The table in segment zero indexes independently compressed segments.
    pub(super) fn modern(
        segments: MeteredVec<DecodeBuf>,
        version: (u32, u32),
        form: super::View<'_>,
        platforms: super::view::Seq<'_>,
        platform_index: usize,
        meter: &Arc<Meter>,
    ) -> Result<Program<'static>, Error> {
        let data = Data::Segments(segments);
        let mut table = Reader::new(data.bytes(), Endian::Little);
        let count = int(&mut table, meter)?;
        let capacity = usize::try_from(count).map_err(|_| Error::LayoutIndex)?;
        let stride = if version >= (2019, 3) { 12 } else { 8 };
        if 4 + capacity as i128 * stride > data.bytes().len() as i128 {
            return Err(Error::LayoutIndex);
        }
        let selection =
            super::layout::Selection::collect(form, platforms, platform_index, capacity)?;
        let mut subs = MeteredVec::new(meter);
        for (indices, code) in [(&selection.code, true), (&selection.parameters, false)] {
            for index in indices.as_slice() {
                meter.count_decode(1)?;
                if *index >= capacity {
                    return Err(Error::LayoutIndex);
                }
                table
                    .set_position(4 + *index as i128 * stride)
                    .map_err(|_| Error::LayoutIndex)?;
                let start =
                    usize::try_from(int(&mut table, meter)?).map_err(|_| Error::LayoutIndex)?;
                let length =
                    usize::try_from(int(&mut table, meter)?).map_err(|_| Error::LayoutIndex)?;
                let segment = if stride == 12 {
                    usize::try_from(int(&mut table, meter)?).map_err(|_| Error::LayoutIndex)?
                } else {
                    0
                };
                let bytes = data.segment(segment)?;
                // Offset-only tables use zero length; keep the segment bound.
                let end = if length == 0 {
                    bytes.len()
                } else {
                    start.checked_add(length).ok_or(Error::LayoutIndex)?
                };
                if start >= bytes.len() || end > bytes.len() {
                    return Err(Error::LayoutIndex);
                }
                if code {
                    let mut r =
                        Reader::new(bytes.get(..end).ok_or(Error::LayoutIndex)?, Endian::Little);
                    r.set_position(start as i128)
                        .map_err(|_| Error::LayoutIndex)?;
                    let mut sub = read_sub(&mut r, meter, true, true).map_err(|e| match e {
                        Error::Struct => Error::LayoutIndex,
                        other => other,
                    })?;
                    sub.segment = segment;
                    subs.push(sub)?;
                }
            }
        }
        Ok(Program {
            subs,
            data,
            selected: Some(selection.code),
            capacity,
        })
    }
    pub(super) fn legacy(data: DecodeBuf, meter: &Arc<Meter>) -> Result<Program<'static>, Error> {
        let data = Data::Decoded(data);
        let mut r = Reader::new(data.bytes(), Endian::Big);
        let count = int(&mut r, meter)?;
        if count < 0 || 4 + i128::from(count) * 8 > data.bytes().len() as i128 {
            return Err(Error::Struct);
        }
        let mut subs = MeteredVec::new(meter);
        for i in 0..count {
            r.set_position(4 + i128::from(i) * 8)
                .map_err(|_| Error::Struct)?;
            let offset = int(&mut r, meter)?;
            if usize::try_from(offset).map_or(true, |offset| offset >= data.bytes().len()) {
                return Err(Error::Struct);
            }
            r.set_position(i128::from(offset))
                .map_err(|_| Error::Struct)?;
            subs.push(read_sub(&mut r, meter, true, true)?)?;
        }
        Ok(Program {
            subs,
            data,
            selected: None,
            capacity: usize::try_from(count).map_err(|_| Error::Struct)?,
        })
    }
    /// Number of parsed table entries available to render.
    #[must_use]
    pub fn subprogram_count(&self) -> usize {
        self.subs.len()
    }
    /// Parse a little-endian platform blob. Tables grow only after valid reads.
    pub fn parse(blob: &'a [u8], version: (u32, u32), meter: &Arc<Meter>) -> Result<Self, Error> {
        Self::parse_data(Data::Borrowed(blob), version, meter)
    }
    pub(super) fn decoded(
        blob: DecodeBuf,
        version: (u32, u32),
        meter: &Arc<Meter>,
    ) -> Result<Program<'static>, Error> {
        Program::parse_data(Data::Decoded(blob), version, meter)
    }
    fn parse_data(data: Data<'a>, version: (u32, u32), meter: &Arc<Meter>) -> Result<Self, Error> {
        let blob = data.bytes();
        let mut r = Reader::new(blob, Endian::Little);
        let count = int(&mut r, meter)?;
        let entry = if version >= (2019, 3) { 12 } else { 8 };
        if count > 0 && 4 + (i128::from(count) - 1) * entry + 4 > blob.len() as i128 {
            // Validate without allocating. Earlier subprogram errors must
            // win over a later missing table entry, as in the reference.
            for i in 0..count {
                r.set_position(4 + i128::from(i) * entry)
                    .map_err(|_| Error::Struct)?;
                let offset = int(&mut r, meter)?;
                r.set_position(i128::from(offset))
                    .map_err(|_| Error::Struct)?;
                read_sub(&mut r, meter, false, false)?;
            }
            return Err(Error::Struct);
        }
        let mut subs = MeteredVec::new(meter);
        for i in 0..count {
            r.set_position(4 + i128::from(i) * entry)
                .map_err(|_| Error::Struct)?;
            let offset = int(&mut r, meter)?;
            r.set_position(i128::from(offset))
                .map_err(|_| Error::Struct)?;
            subs.push(read_sub(&mut r, meter, true, false)?)?;
        }
        Ok(Self {
            subs,
            data,
            selected: None,
            capacity: usize::try_from(count.max(0)).map_err(|_| Error::Struct)?,
        })
    }
    /// Render one table entry, using Python's signed list index convention.
    pub fn export(&self, index: i64, meter: &Arc<Meter>) -> Result<MeteredVec<u8>, Error> {
        let mut out = Output::new(meter);
        self.write(index, &mut out)?;
        Ok(out.bytes)
    }
    pub(super) fn write(&self, index: i64, out: &mut Output<'_>) -> Result<(), Error> {
        let index = if index < 0 {
            self.capacity as i128 + i128::from(index)
        } else {
            i128::from(index)
        };
        let index = usize::try_from(index).map_err(|_| Error::Index)?;
        let index = if let Some(selected) = &self.selected {
            out.work(u64::from(selected.len().max(1).ilog2()) + 1)?;
            selected
                .as_slice()
                .binary_search(&index)
                .map_err(|_| Error::Index)?
        } else {
            index
        };
        let sub = Some(index)
            .and_then(|i| self.subs.as_slice().get(i))
            .ok_or(Error::Index)?;
        for (label, words) in [
            ("Keywords { ", &sub.keywords),
            ("Local Keywords { ", &sub.local),
        ] {
            if !words.is_empty() {
                out.text(label)?;
                for word in words.as_slice() {
                    out.text("\"")?;
                    out.text(
                        self.data
                            .segment(sub.segment)?
                            .get(word.clone())
                            .ok_or(Error::Index)?,
                    )?;
                    out.text("\" ")?;
                }
                out.text("}\n")?;
            }
        }
        out.text("\"")?;
        let code = self
            .data
            .segment(sub.segment)?
            .get(sub.code.clone())
            .ok_or(Error::Index)?;
        if !code.is_empty() {
            match sub.kind {
                1..=8 | 26..=30 => utf8(code, out)?,
                9..=22 => out.text("// shader disassembly not supported on DXBC")?,
                23 | 24 => {
                    // Fixed reads, NUL search and entry-name decoding, including failures.
                    out.work((code.len() as u64).saturating_mul(2))?;
                    let mut r = Reader::new(code, Endian::Little);
                    let tag = r.read_u32().map_err(|_| Error::Struct)?;
                    if tag == 0xf00d_cafe {
                        let offset = r.read_i32().map_err(|_| Error::Struct)?;
                        r.set_position(i128::from(offset))
                            .map_err(|_| Error::Struct)?;
                    }
                    r.read_string_to_null().map_err(|_| Error::Unterminated)?;
                    let remaining = code.len() as i128 - r.position();
                    let remaining = i64::try_from(remaining).map_err(|_| Error::Struct)?;
                    utf8(r.read(remaining).map_err(|_| Error::Struct)?, out)?;
                }
                25 => {}
                n => {
                    out.text("//shader disassembly not supported on ")?;
                    out.int(i128::from(n))?;
                }
            }
        }
        out.text("\"")
    }
}
fn utf8(code: &[u8], out: &mut Output<'_>) -> Result<(), Error> {
    out.work(code.len() as u64)?;
    std::str::from_utf8(code).map_err(|_| Error::Utf8)?;
    out.text(code)
}

#[derive(Debug)]
enum Data<'a> {
    Borrowed(&'a [u8]),
    Decoded(DecodeBuf),
    Segments(MeteredVec<DecodeBuf>),
}
impl Data<'_> {
    fn segment(&self, i: usize) -> Result<&[u8], Error> {
        match self {
            Self::Segments(b) => b
                .as_slice()
                .get(i)
                .map(DecodeBuf::as_slice)
                .ok_or(Error::LayoutIndex),
            _ if i == 0 => Ok(self.bytes()),
            _ => Err(Error::LayoutIndex),
        }
    }
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(b) => b,
            Self::Decoded(b) => b.as_slice(),
            Self::Segments(b) => b.as_slice().first().map_or(&[], DecodeBuf::as_slice),
        }
    }
}

fn read_sub(
    r: &mut Reader<'_>,
    meter: &Arc<Meter>,
    collect: bool,
    strict: bool,
) -> Result<SubProgram, Error> {
    let version = int(r, meter)?;
    let kind = int(r, meter)?;
    if !(0..=31).contains(&kind) {
        return Err(Error::ProgramType(kind));
    }
    r.set_position(r.position() + 12 + if version >= 201_608_170 { 4 } else { 0 })
        .map_err(|_| Error::Struct)?;
    let words = keywords(r, meter, collect, strict)?;
    let local = if (201_806_140..202_012_090).contains(&version) {
        keywords(r, meter, collect, strict)?
    } else {
        MeteredVec::new(meter)
    };
    let len = int(r, meter)?;
    if strict && (len < 0 || i128::from(len) > r.data().len() as i128 - r.position()) {
        return Err(Error::Struct);
    }
    let code = r.read_range(i64::from(len)).map_err(|_| Error::Struct)?;
    meter.count_decode(code.len() as u64)?;
    r.align(4).map_err(|_| Error::Struct)?;
    Ok(SubProgram {
        kind,
        keywords: words,
        local,
        code,
        segment: 0,
    })
}
