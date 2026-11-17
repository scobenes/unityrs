// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py. See THIRD_PARTY_LICENSES.
//! Direct access to the existing bounded converter stages.
use super::{Error, Program, View, convert, output::Output};
use std::sync::Arc;
use unityrs_core::{
    Parsed,
    limits::{Meter, MeteredVec},
};

/// Convert a caller-supplied stage using the same metered field and output walk.
/// `program_blobs` contains synthetic little-endian tables of current fields.
/// Retains UnityPy's property-type refusals for Python helper compatibility.
pub fn direct_stage(
    parsed: &Parsed,
    stage: &str,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, Error> {
    let root = View {
        parsed,
        value: parsed.value(),
        meter,
        is_none: false,
    };
    let value = root.field("value")?;
    let platforms = root.field("platforms")?.seq()?;
    let mut programs = MeteredVec::new(meter);
    for blob in root.field("program_blobs")?.list()? {
        let data = super::bytes(blob.value, meter)?;
        let mut decoded = unityrs_core::budget::DecodeBuf::new(
            meter,
            data.len() as u64,
            unityrs_core::limits::Limits::default().max_block_output,
            unityrs_core::LimitKind::BlockOutput,
        )?;
        decoded.extend_from_slice(&data)?;
        programs.push(Program::decoded(decoded, (2018, 1), meter)?)?;
    }
    let mut out = Output::new(meter);
    out.int_properties = false;
    match stage {
        "shader" => convert::shader(value, platforms, programs.as_slice(), &mut out, meter)?,
        "subshader" => convert::subshader(value, platforms, programs.as_slice(), &mut out, meter)?,
        "pass" => convert::pass_write(value, platforms, programs.as_slice(), &mut out, meter)?,
        "subprograms" => convert::subprograms(
            value,
            false,
            platforms,
            programs.as_slice(),
            &mut out,
            meter,
        )?,
        "state" => convert::state(value, &mut out)?,
        "properties" => convert::properties(value, &mut out)?,
        "property" => convert::property(value, &mut out)?,
        "tags" => convert::tags(
            value,
            usize::try_from(root.field("indent")?.int()?.max(0)).map_err(|_| Error::Index)?,
            &mut out,
        )?,
        "platform" => out.text(convert::platform_name(value.int()?))?,
        "usable" => out.text(
            if convert::usable(value.int()?, root.field("kind")?.int()?)? {
                "1"
            } else {
                "0"
            },
        )?,
        "program" => programs
            .as_slice()
            .first()
            .ok_or(Error::Index)?
            .write(value.int()?, &mut out)?,
        _ => return Err(Error::NotImplemented),
    }
    Ok(out.bytes)
}
