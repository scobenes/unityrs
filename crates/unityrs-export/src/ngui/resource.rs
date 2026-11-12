//! A texture's data (section 5): its `image_data` when not empty, else the
//! resource its `m_StreamData` names, read by 4.3's own-chain procedure
//! (`docs/plans/batch.md` 2.6 step 8), then through the Environment. Each
//! core call (a candidate's join, each `simplify_name`, a row's `entry`
//! and its `entry_range`, the Environment's `resource`) is under the
//! refusal probe on its own (2.7; review round 3's P1).

use std::sync::Arc;

use unityrs_core::env::resource_candidates;
use unityrs_core::files::{BundleFile, File, SerializedFile};
use unityrs_core::pystr::simplify_name;
use unityrs_core::{Bytes, RawStr};

use super::classify::{self, Class, Reason};
use super::read::{Span, Stream, Texture, parent_bundle, python_slice};
use super::seam::Seam;

/// What one own row answers: the bytes, a failure, or nothing (a bundle
/// row, passed over).
enum Row {
    Read(Bytes),
    Failed(Class),
    Passed,
}

fn read_row(seam: &Seam<'_>, b: &BundleFile, i: usize, s: &Stream) -> Row {
    match seam.probe(|| b.entry(i).map_err(|e| classify::error(&e))) {
        Err(c) => Row::Failed(c),
        Ok(File::Raw(_)) => {
            let (offset, size, slice) = read_span(s);
            let range = offset..offset.saturating_add(size);
            match seam.probe(|| b.entry_range(i, range).map_err(|e| classify::error(&e))) {
                Ok(bytes) => Row::Read(sliced(bytes, slice)),
                Err(c) => Row::Failed(c),
            }
        }
        // The core's `not_a_reader`: a SerializedFile has no bytes to read.
        Ok(File::Serialized(_)) => Row::Failed(Class::Skip(Reason::decode_error("malformed"))),
        Ok(File::Web(_)) => Row::Failed(Class::Skip(Reason::decode_error("unsupported"))),
        Ok(File::Bundle(_)) => Row::Passed,
    }
}

/// The offset and size the core reads for `s`, and the Python slice taken
/// of what it gives, when the span is not in `u64` (the whole resource then).
fn read_span(s: &Stream) -> (u64, u64, Option<(i128, i128)>) {
    match s.span() {
        Span::Range(offset, size) => (offset, size, None),
        Span::Slice(start, stop) => (0, u64::MAX, Some((start, stop))),
    }
}

/// `bytes`, or Python's slice of them ([`python_slice`]).
fn sliced(bytes: Bytes, slice: Option<(i128, i128)>) -> Bytes {
    match slice {
        None => bytes,
        Some((start, stop)) => bytes.slice(python_slice(bytes.len(), start, stop)),
    }
}

/// The own-chain procedure: for each candidate in order, each container
/// holding `from`, innermost first, its rows of the candidate's key from
/// last to first. `None` when no own row answers.
fn own_chain(seam: &Seam<'_>, from: &SerializedFile, s: &Stream) -> Result<Option<Bytes>, Class> {
    let meter = seam.meter;
    let simplified =
        |name: &RawStr| seam.probe(|| simplify_name(name, meter).map_err(|e| classify::charge(&e)));
    for candidate in &resource_candidates(&s.path) {
        let name = seam.probe(|| candidate.joined(meter).map_err(|e| classify::charge(&e)))?;
        let key = simplified(&name)?;
        let mut container = parent_bundle(from.parent().map(|p| &p.kind));
        while let Some(b) = container {
            for (i, row) in b.directory().iter().enumerate().rev() {
                if simplified(&row.path)? != key {
                    continue;
                }
                match read_row(seam, &b, i, s) {
                    Row::Read(bytes) => return Ok(Some(bytes)),
                    Row::Failed(c) => return Err(c),
                    Row::Passed => {}
                }
            }
            container = parent_bundle(b.parent().map(|p| &p.kind));
        }
    }
    Ok(None)
}

/// The texture's data: empty when it has neither `image_data` nor a
/// `m_StreamData` with a path (section 3's answer 5 then follows).
pub(super) fn data(
    seam: &Seam<'_>,
    from: &Arc<SerializedFile>,
    t: &Texture,
) -> Result<Bytes, Class> {
    if !t.image_data.is_empty() {
        return Ok(t.image_data.clone());
    }
    let Some(s) = &t.stream else {
        return Ok(Bytes::new());
    };
    if s.path.is_empty() {
        return Ok(Bytes::new());
    }
    if let Some(bytes) = own_chain(seam, from, s)? {
        return Ok(bytes);
    }
    let (offset, size, slice) = read_span(s);
    seam.env_call(|| {
        seam.env()
            .resource(from, &s.path, offset, size)
            .map(|bytes| sliced(bytes, slice))
            .map_err(|e| classify::resource(&e, &s.path))
    })
}
