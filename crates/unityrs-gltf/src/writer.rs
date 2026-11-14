//! JSON is exactly sized; BIN is emitted in view order without seeking.
use crate::{
    Data, GltfError, Layout, Options, Report, Scene, json::Json, layout, normalise, schema,
    validate,
};
use std::{io::Write, sync::Arc};
use unityrs_core::limits::{Meter, MeteredVec};

/// Write one GLB. The sink must be bounded or charge its own storage before
/// growing. All layout/JSON failures occur before the header is emitted;
/// work/I/O failures while streaming can leave a partial file.
pub fn write_glb(
    s: &Scene,
    opts: &Options,
    meter: &Arc<Meter>,
    out: &mut dyn Write,
) -> Result<Report, GltfError> {
    let l = layout(s, opts, meter)?;
    let mut measure = Json::new(meter, None);
    schema::document(s, &l, &mut measure)?;
    let json_len =
        u32::try_from(measure.len).map_err(|_| validate::invalid("JSON length exceeds u32"))?;
    let padded = crate::layout::align(json_len)?;
    let bin_padded = crate::layout::align(l.bin_length)?;
    let total = 20u32
        .checked_add(padded)
        .and_then(|v| v.checked_add(if l.bin_length == 0 { 0 } else { 8 }))
        .and_then(|v| v.checked_add(bin_padded))
        .ok_or_else(|| validate::invalid("GLB length exceeds u32"))?;
    meter.count_decode(u64::from(json_len))?;
    let mut json = MeteredVec::with_capacity(meter, measure.len)?;
    schema::document(s, &l, &mut Json::new(meter, Some(&mut json)))?;
    if json.len() != measure.len {
        return Err(validate::invalid("JSON size changed"));
    }
    meter.count_decode(u64::from(padded) + 20)?;
    out.write_all(b"glTF")?;
    word(out, 2)?;
    word(out, total)?;
    word(out, padded)?;
    word(out, 0x4e4f_534a)?;
    out.write_all(json.as_slice())?;
    padding(out, padded - json_len, b' ')?;
    drop(json);
    if l.bin_length != 0 {
        meter.count_decode(8)?;
        word(out, bin_padded)?;
        word(out, 0x004e_4942)?;
        binary(s, &l, *opts, meter, out)?;
        meter.count_decode(u64::from(bin_padded - l.bin_length))?;
        padding(out, bin_padded - l.bin_length, 0)?;
    }
    Ok(l.report)
}
fn word(out: &mut dyn Write, v: u32) -> Result<(), GltfError> {
    Ok(out.write_all(&v.to_le_bytes())?)
}
fn padding(out: &mut dyn Write, n: u32, value: u8) -> Result<(), GltfError> {
    for _ in 0..n {
        out.write_all(&[value])?;
    }
    Ok(())
}
struct Chunk<'a> {
    bytes: [u8; 65536],
    len: usize,
    out: &'a mut dyn Write,
}
impl Chunk<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), GltfError> {
        if self.len + bytes.len() > self.bytes.len() {
            self.flush()?;
        }
        let end = self.len + bytes.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or_else(|| validate::invalid("chunk capacity"))?
            .copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), GltfError> {
        self.out.write_all(
            self.bytes
                .get(..self.len)
                .ok_or_else(|| validate::invalid("chunk size"))?,
        )?;
        self.len = 0;
        Ok(())
    }
}
fn begin(
    l: &Layout,
    id: usize,
    offset: &mut u32,
    meter: &Meter,
    out: &mut dyn Write,
) -> Result<(), GltfError> {
    let v = l
        .views
        .as_slice()
        .get(id)
        .ok_or_else(|| validate::invalid("buffer view index"))?;
    let gap = v
        .byte_offset
        .checked_sub(*offset)
        .ok_or_else(|| validate::invalid("buffer view order"))?;
    meter.count_decode(u64::from(gap) + u64::from(v.byte_length))?;
    padding(out, gap, 0)?;
    *offset = v.byte_offset + v.byte_length;
    Ok(())
}
#[allow(clippy::large_stack_arrays)] // Fixed 64 KiB, independent of model size.
fn binary(
    s: &Scene,
    l: &Layout,
    opts: Options,
    meter: &Meter,
    out: &mut dyn Write,
) -> Result<(), GltfError> {
    let mut offset = 0;
    let mut chunk = Chunk {
        bytes: [0; 65536],
        len: 0,
        out,
    };
    for &id in l.accessor_order.as_slice() {
        let a = s
            .accessors
            .as_slice()
            .get(id)
            .ok_or_else(|| validate::invalid("accessor index"))?;
        let al = l
            .accessors
            .as_slice()
            .get(id)
            .ok_or_else(|| validate::invalid("accessor layout"))?;
        if al.id.is_none() || (al.view.is_none() && al.sparse.is_none()) {
            continue;
        }
        chunk.flush()?;
        if let Some((iv, _)) = al.sparse {
            begin(l, iv, &mut offset, meter, chunk.out)?;
            for &index in a
                .sparse_indices
                .as_ref()
                .ok_or_else(|| validate::invalid("sparse indices"))?
                .as_slice()
            {
                chunk.put(&index.to_le_bytes())?;
            }
            chunk.flush()?;
        }
        let view = al
            .view
            .or(al.sparse.map(|(_, v)| v))
            .ok_or_else(|| validate::invalid("accessor view"))?;
        begin(l, view, &mut offset, meter, chunk.out)?;
        if let Some(weights) = al.joint_weights {
            let weight_data = validate::accessor(s, weights, crate::Kind::Weights)?;
            meter.count_decode(a.count as u64 * 40)?;
            for row in 0..a.count {
                for v in normalise::joints(a, weight_data, row)? {
                    match &a.data {
                        Data::U8(_) => chunk
                            .put(&[u8::try_from(v).map_err(|_| validate::invalid("joint u8"))?])?,
                        Data::U16(_) => chunk.put(
                            &u16::try_from(v)
                                .map_err(|_| validate::invalid("joint u16"))?
                                .to_le_bytes(),
                        )?,
                        _ => return Err(validate::invalid("joint component")),
                    }
                }
            }
            continue;
        }
        match &a.data {
            Data::Float(v) => {
                meter.count_decode(v.len() as u64 * 16)?;
                for r in 0..v.len() / a.kind.width() {
                    for value in normalise::row(a, r, opts.normalise)?
                        .iter()
                        .take(a.kind.width())
                    {
                        chunk.put(&value.to_le_bytes())?;
                    }
                }
            }
            Data::U8(v) => {
                chunk.flush()?;
                chunk.out.write_all(v.as_slice())?;
            }
            Data::U16(v) => {
                for value in v.as_slice() {
                    chunk.put(&value.to_le_bytes())?;
                }
            }
            Data::U32(v) => {
                for value in v.as_slice() {
                    chunk.put(&value.to_le_bytes())?;
                }
            }
        }
    }
    chunk.flush()?;
    for (image, &view) in s.images.as_slice().iter().zip(l.image_views.as_slice()) {
        begin(l, view, &mut offset, meter, chunk.out)?;
        chunk.out.write_all(image.bytes.as_slice())?;
    }
    Ok(())
}
