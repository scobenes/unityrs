//! An object's record (`dump_records.py` `object_record`): its `l1`, the
//! L2 outcome of its read, and its `l3`, the raw bytes and the resave.

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter};
use unityrs_core::provider::TreeSource;
use unityrs_core::walk::{L2Kind, ParseError, WriteRules};
use unityrs_core::{Error, ObjectRef, Parsed, RawStr};

use super::refused::{out_of_memory, read_note, write_note, write_out_of_memory};
use super::{count_hashed, hashed};
use crate::Levels;
use crate::encode::encode_into;
use crate::record::{L2, L2Error, L3, ObjectL1, ObjectRecord, Resave, first_difference, note_of};
use crate::text::Text;

/// `error_kind` on a `check_read` failure (`dump_records.py`): its regex,
/// `Expected to read (\d+) bytes, but only read (\d+) bytes`, has no sign,
/// so a negative `bytes_read` (the pure-Python reader after a negative
/// `TypelessData` length) falls through every test to `other`, with no
/// `bytes_read`, and the oracle reads no value for it
/// (`tests/fixtures/dump/l2_kinds.json` records a real read that does this).
#[must_use]
pub fn check_read_kind(bytes_read: i64) -> (L2Kind, Option<i64>) {
    if bytes_read >= 0 {
        (L2Kind::CheckRead, Some(bytes_read))
    } else {
        (L2Kind::Other, None)
    }
}

/// The L2 outcome of a read: the error, if any, and the value to encode.
fn l2_outcome(result: Result<Parsed, ParseError>) -> (Option<L2Error>, Option<ParsedValue>) {
    match result {
        Ok(p) => (None, Some(ParsedValue::Owned(p))),
        Err(e) => {
            let text = read_note(&e);
            if let ParseError::CheckRead { parsed, .. } = e {
                let (kind, bytes_read) = check_read_kind(parsed.bytes_read);
                let error = L2Error {
                    kind,
                    bytes_read,
                    note: Some(text),
                };
                let value = (kind == L2Kind::CheckRead).then_some(ParsedValue::Shared(parsed));
                return (Some(error), value);
            }
            (
                Some(L2Error {
                    kind: e.l2_kind(),
                    bytes_read: None,
                    note: Some(text),
                }),
                None,
            )
        }
    }
}

/// A read value: owned on success, shared by a `check_read` error.
enum ParsedValue {
    Owned(Parsed),
    Shared(Arc<Parsed>),
}

impl ParsedValue {
    fn get(&self) -> &Parsed {
        match self {
            Self::Owned(p) => p,
            Self::Shared(p) => p,
        }
    }
}

/// `object_record`, its value's text charged to `meter`: the record, and
/// the charge for that text, which must live as long as the record does.
/// A refusal of the text's growth fails the input, and so does a read or a
/// resave the budget or the allocator refuses ([`out_of_memory`]).
pub(super) fn object_record(
    obj: &ObjectRef,
    levels: Levels,
    meter: &Arc<Meter>,
) -> Result<(ObjectRecord, Option<Charge>), Error> {
    let info = obj.info();
    let l1 = levels.l1.then_some(ObjectL1 {
        type_id: info.type_id,
        class_id: info.class_id,
        byte_start: info.byte_start,
        byte_size: info.byte_size,
        is_destroyed: info.is_destroyed,
        is_stripped: info.is_stripped,
    });
    let mut l2 = None;
    let mut l3 = None;
    // The value a full parse gave, kept for the resave (`parsed_ok`).
    let mut resavable: Option<Parsed> = None;
    let mut held = None;
    if levels.l2 {
        let mut out = L2 {
            root_type: None,
            source: None,
            error: None,
            value: None,
        };
        // `_get_typetree_node()`: the type record's own tree when it stores
        // one, else the provider's; every failure to find a tree is
        // `no_typetree`, but a refusal of memory, which is not the input's.
        match obj.tree(None) {
            Ok(Some(t)) => {
                out.root_type = Some(RawStr::from(t.tree.root_type()));
                out.source = Some(match t.source {
                    TreeSource::Embedded => "embedded",
                    TreeSource::Provider(name) => name,
                    TreeSource::Explicit => "explicit",
                });
                let parsed = obj.parse_as_dict(true);
                if let Err(e) = &parsed {
                    out_of_memory(e)?;
                }
                let (error, value) = l2_outcome(parsed);
                out.error = error;
                if let Some(v) = value {
                    let mut text = Text::on(meter);
                    let encoded = encode_into(&mut text, v.get());
                    // A refused growth fails the input, whatever the encoder said.
                    let (text, charge) = text.finish()?;
                    match encoded {
                        Ok(()) => {
                            out.value = Some(text);
                            held = Some(charge);
                            if let ParsedValue::Owned(p) = v
                                && out.error.is_none()
                            {
                                resavable = Some(p);
                            }
                        }
                        Err(e) => {
                            // Only a producer under test writes `encode` (section 4.3).
                            out.error = Some(L2Error {
                                kind: L2Kind::Encode,
                                bytes_read: None,
                                note: Some(note_of(&e)),
                            });
                            out.value = None;
                        }
                    }
                }
            }
            Ok(None) => {
                out.error = Some(L2Error {
                    kind: L2Kind::NoTypetree,
                    bytes_read: None,
                    note: Some(format!("no typetree for class {}", info.class_id)),
                });
            }
            Err(e) => {
                out_of_memory(&e)?;
                // A tree the type record stores is the one asked for, and
                // its compile failed (deeper than `max_tree_depth`, the
                // meter's bound): UnityPy holds the node all the same, so
                // the record names its root type and its source, and the
                // read's outcome is the compile's error, `other`
                // (`PARITY.md` section 19.3, "typetree depth").
                let embedded = obj.serialized_type().and_then(|st| st.nodes.as_deref());
                if let Some(nodes) = embedded {
                    out.root_type = Some(RawStr::from(nodes.type_name(nodes.root())));
                    out.source = Some("embedded");
                }
                out.error = Some(L2Error {
                    kind: if embedded.is_some() {
                        e.l2_kind()
                    } else {
                        L2Kind::NoTypetree
                    },
                    bytes_read: None,
                    note: Some(read_note(&e)),
                });
            }
        }
        l2 = Some(out);
    }
    if levels.l3 {
        l3 = Some(l3_of(obj, resavable.as_ref(), meter)?);
    }
    Ok((
        ObjectRecord {
            path_id: info.path_id,
            l1,
            l2,
            l3,
        },
        held,
    ))
}

/// The object's `l3`: its raw bytes, and the resave of `resavable`, the
/// value a full parse gave. A resave the budget or the allocator refuses
/// fails the input ([`write_out_of_memory`]). The raw bytes and the resave's
/// are counted as the open's work before they are hashed or compared
/// ([`count_hashed`]): objects may overlap, so their raw bytes together are
/// not bounded by the file's.
fn l3_of(obj: &ObjectRef, resavable: Option<&Parsed>, meter: &Meter) -> Result<L3, Error> {
    // The dump reads bytes (the memory reader), which refuse no position.
    let raw = obj.raw_data()?;
    let raw_sha256 = hashed(meter, &raw)?;
    let resave = match resavable {
        None => Resave::Skipped,
        Some(p) => match obj.write_typetree(p, None, WriteRules::UnityPy) {
            Ok(bytes) => {
                count_hashed(meter, &bytes)?;
                if bytes == raw {
                    Resave::Equal
                } else {
                    Resave::Differs {
                        size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                        sha256: crate::json::sha256(&bytes),
                        first_diff: first_difference(&bytes, &raw),
                    }
                }
            }
            Err(e) => {
                write_out_of_memory(&e)?;
                Resave::Error {
                    note: Some(write_note(&e)),
                }
            }
        },
    };
    Ok(L3 {
        raw_size: u64::try_from(raw.len()).unwrap_or(u64::MAX),
        raw_sha256,
        resave,
    })
}
