// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py. See THIRD_PARTY_LICENSES.
//! Tier C selection: unlike export/ShaderConverter.py:133-227 and
//! export/ShaderConverter.py:543-559, player lists select code records;
//! parameter indices identify separate records, never program headers.
use super::{
    Error,
    convert::first_usable,
    sort,
    view::{Seq, View},
};
use unityrs_core::limits::MeteredVec;

pub(super) const STAGES: [(&str, &str); 5] = [
    ("progVertex", "vp"),
    ("progFragment", "fp"),
    ("progGeometry", "gp"),
    ("progHull", "hp"),
    ("progDomain", "dp"),
];

pub(super) fn selected(stage: View<'_>) -> Result<(View<'_>, bool), Error> {
    if let Some(subs) = stage.optional("m_SubPrograms")?
        && !subs.empty()
    {
        return Ok((subs, false));
    }
    if let Some(player) = stage.optional("m_PlayerSubPrograms")? {
        return Ok((player, true));
    }
    Ok((stage.field("m_SubPrograms")?, false))
}

pub(super) fn modern(form: View<'_>) -> Result<bool, Error> {
    for sub in form.field("m_SubShaders")?.list()? {
        form.meter.count_decode(1)?;
        for pass in sub.field("m_Passes")?.list()? {
            form.meter.count_decode(1)?;
            for (key, _) in STAGES {
                form.meter.count_decode(1)?;
                if let Some(stage) = pass.optional(key)? {
                    if let Some(params) = stage.optional("m_ParameterBlobIndices")?
                        && !params.empty()
                    {
                        return Ok(true);
                    }
                    let legacy = stage.optional("m_SubPrograms")?.is_some_and(|s| !s.empty());
                    if !legacy
                        && let Some(player) = stage.optional("m_PlayerSubPrograms")?
                        && !player.empty()
                    {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

pub(super) struct Selection {
    pub code: MeteredVec<usize>,
    pub parameters: MeteredVec<usize>,
}
impl Selection {
    pub fn collect(
        form: View<'_>,
        platforms: Seq<'_>,
        platform_index: usize,
        capacity: usize,
    ) -> Result<Self, Error> {
        let mut result = Self {
            code: MeteredVec::new(form.meter),
            parameters: MeteredVec::new(form.meter),
        };
        for sub in form.field("m_SubShaders")?.list()? {
            for pass in sub.field("m_Passes")?.list()? {
                if matches!(pass.field("m_Type")?.int()?, 1 | 2) {
                    continue;
                }
                for (key, _) in STAGES {
                    let stage = pass.field(key)?;
                    let (subs, player) = selected(stage)?;
                    let mut add =
                        |sub: View<'_>, parameter: Option<(Seq<'_>, usize)>| -> Result<(), Error> {
                            form.meter.count_decode(1)?;
                            if first_usable(
                                platforms,
                                platform_index + 1,
                                sub.field("m_GpuProgramType")?.int()?,
                                form.meter,
                            )? == Some(platform_index)
                            {
                                let raw = sub.field("m_BlobIndex")?.int()?;
                                let code = if !player && raw < 0 {
                                    usize::try_from(capacity as i128 + i128::from(raw))
                                        .map_err(|_| Error::LayoutIndex)?
                                } else {
                                    index(raw)?
                                };
                                result.code.push(code)?;
                                if let Some((params, i)) = parameter {
                                    form.meter.count_decode(1)?;
                                    result.parameters.push(index(params.int(i)?)?)?;
                                }
                            }
                            Ok(())
                        };
                    let mut parameter_groups = stage
                        .optional("m_ParameterBlobIndices")?
                        .map(View::list)
                        .transpose()?;
                    for sub in subs.list()? {
                        form.meter.count_decode(1)?;
                        if player {
                            // Both nested lists use the same group/variant coordinates.
                            let params = parameter_groups
                                .as_mut()
                                .and_then(Iterator::next)
                                .map(View::seq)
                                .transpose()?;
                            for (i, sub) in sub.list()?.enumerate() {
                                add(sub, params.filter(|p| i < p.len()).map(|p| (p, i)))?;
                            }
                        } else {
                            add(sub, None)?;
                        }
                    }
                    if !player && let Some(params) = stage.optional("m_ParameterBlobIndices")? {
                        for group in params.list()? {
                            form.meter.count_decode(1)?;
                            let seq = group.seq()?;
                            for i in 0..seq.len() {
                                form.meter.count_decode(1)?;
                                result.parameters.push(index(seq.int(i)?)?)?;
                            }
                        }
                    }
                }
            }
        }
        for indices in [&mut result.code, &mut result.parameters] {
            sort::by_key(indices.as_mut_slice(), |i| *i, form.meter)?;
            let mut unique = 0;
            for i in 0..indices.len() {
                form.meter.count_decode(1)?;
                if unique == 0 || indices.as_slice().get(i) != indices.as_slice().get(unique - 1) {
                    indices.as_mut_slice().swap(unique, i);
                    unique += 1;
                }
            }
            indices.truncate(unique);
        }
        for i in result.code.as_slice() {
            form.meter
                .count_decode(u64::from(result.parameters.len().max(1).ilog2()) + 1)?;
            if result.parameters.as_slice().binary_search(i).is_ok() {
                return Err(Error::LayoutIndex);
            }
        }
        Ok(result)
    }
}
fn index(value: i64) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::LayoutIndex)
}
