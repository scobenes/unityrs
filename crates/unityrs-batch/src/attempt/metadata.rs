//! Own metadata and the charged top-level registration name table.
use super::{
    AttemptView,
    classify::internal,
    hooks::{Engine, probe},
};
use crate::records::{self, DiagnosticKind, Record, Text, Writer};
use std::sync::Arc;
use unityrs_core::{
    RawStr, Value,
    files::BundleFile,
    limits::{Meter, MeteredVec},
    pystr::simplify_name,
};
use unityrs_export::ngui::{self, Class, Failure, Reason, read::attr};

fn charged(e: &unityrs_core::limits::ChargeError) -> Class {
    ngui::classify::charge(e)
}
fn container(c: Class) -> Class {
    match c {
        Class::Skip(r) if r.reason == "decode_error" => Class::Input(Reason {
            reason: "container_error".into(),
            ..r
        }),
        c => c,
    }
}
pub(super) fn dependencies(
    own: &BundleFile,
    view: AttemptView<'_>,
    meter: &Arc<Meter>,
    engine: &Engine<'_>,
) -> Result<MeteredVec<RawStr>, Failure> {
    let files = ngui::select_own(own, meter, engine, &mut |_| Ok(()))?;
    let mut names = MeteredVec::new(meter);
    for file in files.as_slice() {
        engine.check()?;
        probe(engine, || {
            file.container()
                .map_err(|e| container(ngui::classify::error(&e)))
        })?;
        let Some(obj) = file.objects().find(|o| o.class_id() == 142) else {
            continue;
        };
        let parsed = probe(engine, || {
            obj.read_typetree(None, true)
                .map_err(|e| container(ngui::classify::parse(&e)))
        })?;
        let Some(value) = attr(&parsed, parsed.value(), b"m_Dependencies") else {
            continue;
        };
        let values = match value {
            Value::List(v) => v.as_slice(),
            Value::Prims(p) if p.is_empty() => &[],
            _ => {
                return Err(Failure::Input(Reason {
                    reason: "container_error".into(),
                    detail: Some("asset_bundle".into()),
                    cut: false,
                }));
            }
        };
        for value in values {
            let Value::Str(name) = value else {
                return Err(Failure::Input(Reason {
                    reason: "container_error".into(),
                    detail: Some("asset_bundle".into()),
                    cut: false,
                }));
            };
            if name.as_bytes() == view.input.id.as_bytes()
                || view
                    .members
                    .binary_search_by(|m| m.id.as_bytes().cmp(name.as_bytes()))
                    .is_ok()
            {
                continue;
            }
            let name = probe(engine, || {
                RawStr::charged(name.as_bytes(), meter).map_err(|e| charged(&e))
            })?;
            probe(engine, || names.push(name).map_err(|e| charged(&e)))?;
        }
        engine.sample(file.meter());
    }
    Ok(names)
}
struct Name {
    key: RawStr,
    path: RawStr,
    holder: usize,
    row: usize,
}
pub(super) struct Names {
    rows: MeteredVec<Name>,
}
impl Names {
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            rows: MeteredVec::new(meter),
        }
    }
    pub fn loaded(
        &mut self,
        holder: usize,
        bundle: &BundleFile,
        meter: &Arc<Meter>,
        engine: &Engine<'_>,
    ) -> Result<(), Failure> {
        engine.sample(bundle.meter());
        for (row, entry) in bundle.directory().iter().enumerate() {
            let key = probe(engine, || {
                simplify_name(&entry.path, meter).map_err(|e| charged(&e))
            })?;
            // Copy paths; retaining a dependency's bytes would pin its charge
            // past its LRU lifetime and change later reads' budget state.
            let path = probe(engine, || {
                RawStr::charged(entry.path.as_bytes(), meter).map_err(|e| charged(&e))
            })?;
            probe(engine, || {
                self.rows
                    .push(Name {
                        key,
                        path,
                        holder,
                        row,
                    })
                    .map_err(|e| charged(&e))
            })?;
        }
        Ok(())
    }
    pub fn write(
        &mut self,
        view: AttemptView<'_>,
        writer: &mut Writer,
        engine: &Engine<'_>,
    ) -> Result<u64, Failure> {
        self.rows.as_mut_slice().sort_unstable_by(|a, b| {
            a.key
                .cmp(&b.key)
                .then(a.holder.cmp(&b.holder))
                .then(a.row.cmp(&b.row))
        });
        let mut count = 0;
        let mut at = 0;
        let rows = self.rows.as_slice();
        while let Some(first) = rows.get(at) {
            engine.check()?;
            let len = rows
                .get(at..)
                .ok_or_else(internal)?
                .iter()
                .take_while(|r| r.key == first.key)
                .count();
            let last = rows.get(at + len - 1).ok_or_else(internal)?;
            if first.holder != last.holder {
                let bundle = if last.holder == view.members.len() {
                    view.input.id
                } else {
                    view.members.get(last.holder).ok_or_else(internal)?.id
                };
                diagnostic(
                    writer,
                    engine,
                    view.input.id,
                    DiagnosticKind::SameNamedCab,
                    Some(bundle),
                    &last.path,
                )?;
                count += 1;
            }
            at += len;
        }
        Ok(count)
    }
}
pub(super) fn diagnostic(
    writer: &mut Writer,
    engine: &Engine<'_>,
    input: &str,
    kind: DiagnosticKind,
    bundle: Option<&str>,
    name: &RawStr,
) -> Result<(), Failure> {
    engine.check()?;
    engine.records(|work| {
        writer.append(
            Record::Diagnostic(records::Diagnostic {
                kind,
                input,
                bundle,
                name: Text::Raw(name.as_bytes()),
                cut: false,
            }),
            work,
        )
    })
}
