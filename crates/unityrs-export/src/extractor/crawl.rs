// Ported from UnityPy 1.25.2 (MIT): tools/extractor.py. See THIRD_PARTY_LICENSES.
//! Corrected tools/extractor.py:284-304 and tools/extractor.py:327-368:
//! iterative DFS keyed by (file, path id), null pointers skipped, lists,
//! tuples and dicts opened in insertion order; each reachable object once.
use super::{Error, Failure, Options, Report, Sink, contains, one, paths, was_exported};
use crate::simple;
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, Parsed, Value,
    env::{Environment, PPtr},
    limits::{Meter, MeteredVec},
};
fn pointers(parsed: &Parsed, meter: &Arc<Meter>) -> Result<MeteredVec<PPtr>, Error> {
    let mut stack = MeteredVec::new(meter);
    let mut out = MeteredVec::new(meter);
    stack.push(parsed.value())?;
    while let Some(v) = stack.pop() {
        meter.count_decode(1)?;
        match v {
            Value::Dict(m) => {
                if let (Some(file), Some(path)) = (
                    parsed.attr(m, b"m_FileID").and_then(Value::as_i64),
                    parsed.attr(m, b"m_PathID").and_then(Value::as_i64),
                ) {
                    if path != 0 {
                        out.push(PPtr {
                            file_id: i32::try_from(file).map_err(|_| Error::Field("m_FileID"))?,
                            path_id: path,
                        })?;
                    }
                } else {
                    for (_, value) in m.entries().iter().rev() {
                        stack.push(value)?;
                    }
                }
            }
            Value::List(v) => {
                for value in v.iter().rev() {
                    stack.push(value)?;
                }
            }
            Value::Tuple(v) => {
                for value in v.iter().rev() {
                    stack.push(value)?;
                }
            }
            _ => {}
        }
    }
    Ok(out)
}
pub(super) fn export(
    env: &Environment,
    root: &ObjectRef,
    stem: &[u8],
    sink: &mut dyn Sink,
    meter: &Arc<Meter>,
    report: &mut Report,
) -> Result<(), Error> {
    sink.directory(stem)?;
    let mut pending = MeteredVec::new(meter);
    let mut seen = MeteredVec::new(meter);
    let exported_start = report.exported.len();
    pending.push(root.clone())?;
    let options = Options {
        append_path_id: true,
        ..Options::default()
    };
    while let Some(obj) = pending.pop() {
        if contains(seen.as_slice(), &obj, meter)? {
            continue;
        }
        seen.push(obj.clone())?;
        if obj.class_id() == 1 {
            report.exported.push((&obj).into())?;
        } else if !was_exported(
            report
                .exported
                .as_slice()
                .get(exported_start..)
                .unwrap_or_default(),
            &obj,
            meter,
        )? && let Err(error) =
            one(env, &obj, stem, true, &options, sink, meter, report, true)
        {
            report.failures.push(Failure {
                object: obj.clone(),
                path: paths::suffix(stem, b"", meter)?,
                error,
            })?;
        }
        let parsed = match simple::read(&obj, env) {
            Ok(p) => p,
            Err(e) => {
                let error = Error::from(e);
                // This is a fresh read after export, with additional input
                // work. Its refusal must remain visible for every class.
                if error.resource_refusal() {
                    return Err(error);
                }
                // The export already records non-GameObject read errors.
                if obj.class_id() == 1 {
                    report.failures.push(Failure {
                        object: obj.clone(),
                        path: paths::suffix(stem, b"", meter)?,
                        error,
                    })?;
                }
                continue;
            }
        };
        let refs = pointers(&parsed, meter)?;
        for &ptr in refs.as_slice().iter().rev() {
            match env.deref(obj.file(), ptr) {
                Ok(target) => pending.push(target)?,
                Err(error) => report.failures.push(Failure {
                    object: obj.clone(),
                    path: paths::suffix(stem, b"", meter)?,
                    error: error.into(),
                })?,
            }
        }
    }
    Ok(())
}
