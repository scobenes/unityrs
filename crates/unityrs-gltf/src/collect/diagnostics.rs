//! Renderer decisions survive writing in both the report and asset extras.
use super::{Arc, Extras, Meter, MeteredVec, Result, Scene, Text, extra, invalid, number};
pub(super) fn record(
    scene: &mut Scene,
    node: usize,
    reason: &'static str,
    count: u64,
    meter: &Arc<Meter>,
) -> Result<()> {
    scene.collection_issues.push(crate::CollectionIssue {
        node,
        reason,
        count,
    })?;
    // unresolved_skin already carries its detailed reason at the call site.
    if reason != "unresolved_skin" {
        meter.count_decode(scene.collection_issues.len() as u64)?;
        let total = scene
            .collection_issues
            .as_slice()
            .iter()
            .filter(|i| i.node == node && i.reason == reason)
            .try_fold(0u64, |sum, i| {
                sum.checked_add(i.count)
                    .ok_or_else(|| invalid("diagnostic count overflow"))
            })?;
        let extras = &mut scene
            .nodes
            .as_mut_slice()
            .get_mut(node)
            .ok_or_else(|| invalid("diagnostic node"))?
            .extras;
        meter.count_decode(extras.len() as u64 * reason.len() as u64)?;
        if let Some((_, value)) = extras
            .as_mut_slice()
            .iter_mut()
            .find(|(key, _)| key.as_bytes() == reason.as_bytes())
        {
            *value = number(meter, total)?;
        } else {
            extra(extras, meter, reason, number(meter, total)?)?;
        }
    }
    Ok(())
}
pub(super) fn degrade(
    scene: &mut Scene,
    node: usize,
    reason: &'static str,
    meter: &Arc<Meter>,
) -> Result<()> {
    renderer(scene, node, reason, "unresolved_skin", meter)
}
pub(super) fn renderer(
    scene: &mut Scene,
    node: usize,
    reason: &'static str,
    key: &str,
    meter: &Arc<Meter>,
) -> Result<()> {
    merge(
        &mut scene
            .nodes
            .as_mut_slice()
            .get_mut(node)
            .ok_or_else(|| invalid("diagnostic node"))?
            .extras,
        meter,
        key,
        reason.as_bytes(),
    )?;
    record(scene, node, reason, 1, meter)
}

/// Repeated markers form a sorted, unique semicolon-separated set. Per-renderer
/// records and per-reason counts remain separate, including repeated reasons.
pub(super) fn merge(
    extras: &mut Extras,
    meter: &Arc<Meter>,
    key: &str,
    value: &[u8],
) -> Result<()> {
    meter.count_decode(extras.len() as u64 * key.len() as u64)?;
    let Some((_, old)) = extras
        .as_mut_slice()
        .iter_mut()
        .find(|(k, _)| k.as_bytes() == key.as_bytes())
    else {
        return extra(extras, meter, key, Text(crate::array(meter, value)?));
    };
    meter.count_decode((old.as_bytes().len() as u64 + value.len() as u64) * 3)?;
    if old.as_bytes().split(|&b| b == b';').any(|v| v == value) {
        return Ok(());
    }
    let mut out = MeteredVec::new(meter);
    let mut inserted = false;
    for item in old.as_bytes().split(|&b| b == b';') {
        if !inserted && value < item {
            out.extend_from_slice(value)?;
            out.push(b';')?;
            inserted = true;
        }
        out.extend_from_slice(item)?;
        out.push(b';')?;
    }
    if inserted {
        out.truncate(out.len().saturating_sub(1));
    } else {
        out.extend_from_slice(value)?;
    }
    *old = Text(out);
    Ok(())
}

/// Bad source data may be skipped; resource refusals keep their provenance.
pub(super) fn mesh_reason(error: &unityrs_mesh::MeshError) -> Option<&'static str> {
    use unityrs_mesh::MeshError;
    match error {
        MeshError::NotMesh => Some("source_not_mesh"),
        MeshError::Field(_) => Some("malformed_mesh_field"),
        MeshError::Decode(unityrs_meshdecode::MeshError::Charge(_)) => None,
        MeshError::Parse(e) if parse_data(e) => Some("mesh_parse_failed"),
        MeshError::Core(e) if core_data(e) => Some("mesh_resource_failed"),
        MeshError::Decode(e) if decode_data(e) => Some("mesh_decode_failed"),
        _ => None,
    }
}

// Only audited data errors are recoverable. Unknown variants, all limits,
// budgets, providers and allocator failures retain their original errors.
fn parse_data(error: &unityrs_core::walk::ParseError) -> bool {
    use unityrs_core::walk::{ParseError, ReadDetail};
    match error {
        ParseError::NoTypeTree { .. } | ParseError::CheckRead { .. } => true,
        ParseError::Tree(e) => tree_data(e),
        ParseError::Deref(e) => deref_data(e),
        ParseError::Read(e) => match &e.detail {
            ReadDetail::Tree(e) => tree_data(e),
            ReadDetail::Eof { .. }
            | ReadDetail::NegativeLength(_)
            | ReadDetail::ArrayChildren
            | ReadDetail::PairChildren
            | ReadDetail::NoRefTypes
            | ReadDetail::RefTypeNotFound
            | ReadDetail::RefTypeNoTree
            | ReadDetail::Python(_)
            | ReadDetail::Stream(_) => true,
            _ => false,
        },
        _ => false,
    }
}
fn core_data(error: &unityrs_core::Error) -> bool {
    use unityrs_core::Error;
    match error {
        Error::OutOfBounds { .. }
        | Error::Malformed { .. }
        | Error::Unsupported(_)
        | Error::Decompress { .. }
        | Error::Version(_)
        | Error::TypeIndex { .. }
        | Error::UnityCnKeyLength { .. }
        | Error::UnityCnKeyMissing
        | Error::UnityCnVectorLength { .. }
        | Error::UnityCnSignature
        | Error::NotFound(_) => true,
        Error::Detect(e) => matches!(
            e,
            unityrs_core::detect::DetectError::Unterminated
                | unityrs_core::detect::DetectError::Gzip(_)
        ),
        Error::Tree(e) => tree_data(e),
        Error::Deref(e) => deref_data(e),
        Error::Io(e) => e.0.kind() == std::io::ErrorKind::NotFound,
        Error::AssetBundle(e) => parse_data(e),
        Error::Entry { source, .. } => core_data(source),
        _ => false,
    }
}

fn tree_data(error: &unityrs_core::typetree::TreeError) -> bool {
    use unityrs_core::typetree::TreeError;
    matches!(error, TreeError::Empty | TreeError::LevelUnderflow { .. })
}
fn deref_data(error: &unityrs_core::env::DerefError) -> bool {
    use unityrs_core::env::DerefError;
    match error {
        DerefError::Null
        | DerefError::FileIdPastEnd { .. }
        | DerefError::FileIdBelowStart { .. }
        | DerefError::NotFound { .. }
        | DerefError::RawTarget { .. }
        | DerefError::MissingObject { .. } => true,
        DerefError::Load(e) => core_data(e),
        _ => false,
    }
}
fn decode_data(error: &unityrs_meshdecode::MeshError) -> bool {
    use unityrs_meshdecode::MeshError;
    match error {
        MeshError::Resource(e) => core_data(e),
        MeshError::Assertion(_)
        | MeshError::NoAttribute(_)
        | MeshError::UnknownMeshType
        | MeshError::VersionType(_)
        | MeshError::InvalidFormat { .. }
        | MeshError::UnknownChannel(_)
        | MeshError::NoChannels
        | MeshError::OutOfBounds
        | MeshError::Struct(_)
        | MeshError::Index(_)
        | MeshError::Unpack { .. }
        | MeshError::StopIteration
        | MeshError::Overflow
        | MeshError::ByteRange
        | MeshError::Topology
        | MeshError::WrappedAccess
        | MeshError::Unsupported(_)
        | MeshError::Malformed(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::mesh_reason;
    use unityrs_core::{
        Error, LimitError, LimitKind, NotFound,
        limits::ChargeError,
        walk::{ParseError, ReadDetail, ReadError, ReadErrorKind},
    };
    use unityrs_mesh::MeshError;
    #[test]
    fn source_data_errors_recover_but_nested_work_limits_remain_fatal() {
        let limit = || LimitError::refused(LimitKind::DecodeWork, 2, 1);
        let parse = |detail| {
            ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail,
            })
        };
        for error in [
            MeshError::Charge(ChargeError::Limit(limit())),
            MeshError::Core(Error::Limit(limit())),
            MeshError::Parse(parse(ReadDetail::Limit(limit()))),
            MeshError::Decode(unityrs_meshdecode::MeshError::Resource(Error::Limit(
                limit(),
            ))),
        ] {
            assert_eq!(mesh_reason(&error), None);
        }
        assert_eq!(
            mesh_reason(&MeshError::Parse(parse(ReadDetail::Eof { need: 1 }))),
            Some("mesh_parse_failed")
        );
        assert_eq!(
            mesh_reason(&MeshError::Core(Error::NotFound(NotFound::Source(7)))),
            Some("mesh_resource_failed")
        );
    }
}
