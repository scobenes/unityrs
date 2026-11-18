//! Prefab selections append to the sealed 2D plan and use its key table.
pub(super) mod failure;
use std::{io, io::Write as _, sync::Arc};
use unityrs_core::{
    ObjectRef, RawStr,
    container::ContainerRow,
    env::PPtr,
    files::SerializedFile,
    limits::{ChargeError, Meter, MeteredVec},
};
use unityrs_gltf::{Clips, Profile, UnlitExtras};
use unityrs_scene::{GraphOptions, Unit};

use super::{
    Context, Cut, Failure, GlbDiagnostic, Options, Output, OutputKind, PrefabRow, PrefabSkip, Skip,
    SkipOutput, Template,
    attempt::Attempt,
    classify::{self, Class, Reason},
    export::md5,
    keys::Var,
    seam::Seam,
    table,
};

/// One input-scoped override, sorted by key. Its ordinal is spec order.
pub struct Selection<'a> {
    pub key: &'a str,
    pub clips: &'a [u8],
    pub ordinal: u32,
    pub found: bool,
}

pub struct Prefabs<'a, 'spec> {
    pub template: &'a Template,
    pub profile: &'a [u8],
    pub selections: &'a mut [Selection<'spec>],
    pub files: &'a crate::gltf::FileIndex,
    pub missing: &'a mut Option<u32>,
}

pub fn cut_prefabs(
    ctx: Context<'_>,
    options: &Options,
    prefabs: Prefabs<'_, '_>,
) -> Result<Cut, Failure> {
    super::cut_inner(ctx, options, Some(prefabs))
}

pub(super) struct Planned {
    index: u64,
    row: PrefabRow,
    key: String,
    source_file: Arc<SerializedFile>,
    pointer: PPtr,
    selection: Option<usize>,
}

fn internal() -> Class {
    Class::Run(Reason::plain("internal"))
}

impl Attempt<'_> {
    pub(super) fn plan_prefabs(
        &mut self,
        files: &[Arc<SerializedFile>],
        mut index: u64,
        prefabs: &mut Prefabs<'_, '_>,
    ) -> Result<MeteredVec<Planned>, Failure> {
        let seam = self.seam;
        let mut planned = MeteredVec::new(seam.meter);
        for (file_index, file) in files.iter().enumerate() {
            seam.stop()?;
            let rows =
                seam.env_call(|| file.container().map_err(|error| classify::error(&error)))?;
            for (ordinal, source) in rows.rows().iter().enumerate() {
                let identity = (file_index, ordinal);
                if let Some(row) = self.plan_prefab(file, source, identity, index, prefabs)? {
                    self.charged(|| table::push(&mut planned, row))?;
                    index = index.checked_add(1).ok_or_else(internal)?;
                }
            }
        }
        *prefabs.missing = prefabs
            .selections
            .iter()
            .filter(|row| !row.found)
            .map(|row| row.ordinal)
            .min();
        Ok(planned)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Preserve the ordered row, expansion and ownership decisions"
    )]
    fn plan_prefab(
        &mut self,
        file: &'_ Arc<SerializedFile>,
        source: &ContainerRow,
        (file_index, ordinal): (usize, usize),
        index: u64,
        prefabs: &mut Prefabs<'_, '_>,
    ) -> Result<Option<Planned>, Failure> {
        let seam = self.seam;
        seam.stop()?;
        seam.work(1)?;
        let mut row = PrefabRow {
            file_index: u32::try_from(file_index).map_err(|_| internal())?,
            ordinal: u32::try_from(ordinal).map_err(|_| internal())?,
            serialized_file: file.name().clone(),
            path: source.path.clone(),
            root: None,
        };
        let path_id = source.path_id.and_then(|id| i64::try_from(id).ok());
        let pointer = source.asset();
        let (root, mut skip) = match seam.env_call(|| {
            let pointer = pointer.ok_or_else(|| Class::Skip(Reason::plain("bad_pointer")))?;
            seam.env()
                .deref(file, pointer)
                .map_err(|e| classify::deref(&e))
        }) {
            Ok(root) if root.class_id() == 1 => {
                row.root = Some((root.file().name().clone(), root.path_id()));
                (Some(root), None)
            }
            Ok(_) => return Ok(None),
            Err(Class::Skip(reason)) => (None, Some(reason)),
            Err(fatal) => return Err(fatal.into()),
        };
        let template = prefabs.template;
        let name = if let Some(root) = root.as_ref().filter(|_| template.uses(Var::Name)) {
            match self.reader.peek_name(&seam, root) {
                Ok(name) => Some(name),
                Err(Class::Skip(reason)) => {
                    skip = Some(reason);
                    None
                }
                Err(fatal) => return Err(fatal.into()),
            }
        } else {
            None
        };
        let missing = (template.uses(Var::Name) && name.is_none())
            || (template.uses(Var::PathId) && path_id.is_none())
            || (template.uses(Var::Type) && root.is_none())
            || (template.uses(Var::ContainerPath) && row.path.is_none());
        if missing {
            self.prefab_skip(
                row,
                path_id,
                skip.unwrap_or_else(|| Reason::plain("bad_pointer")),
            )?;
            return Ok(None);
        }
        expand(
            template,
            self.input,
            path_id,
            name.as_ref(),
            row.path.as_ref(),
            &mut self.scratch.key,
        )?;
        seam.work(self.scratch_key().len() as u64)?;
        if let Err(rule) = self.check_key() {
            self.prefab_skip(
                row,
                path_id,
                skip.unwrap_or_else(|| super::attempt::unwritable(rule)),
            )?;
            return Ok(None);
        }
        let key = std::str::from_utf8(&self.scratch.key).map_err(|_| internal())?;
        if !seam.probe(|| self.keys.take(key))? {
            self.prefab_skip(row, path_id, Reason::plain("duplicate_key"))?;
            return Ok(None);
        }
        let selection = prefabs
            .selections
            .binary_search_by(|entry| entry.key.cmp(key))
            .ok();
        if let Some(selection) = selection {
            prefabs
                .selections
                .get_mut(selection)
                .ok_or_else(internal)?
                .found = true;
        }
        if let Some(reason) = skip {
            self.prefab_skip(row, path_id, reason)?;
            return Ok(None);
        }
        let pointer = pointer.ok_or_else(internal)?;
        self.plan_strings
            .grow(table::string(key.len()))
            .map_err(|e| classify::charge(&e))?;
        let key = copy_key(key)?;
        Ok(Some(Planned {
            index,
            row,
            key,
            source_file: file.clone(),
            pointer,
            selection,
        }))
    }

    pub(super) fn export_prefabs(
        &mut self,
        prefabs: &Prefabs<'_, '_>,
        planned: MeteredVec<Planned>,
    ) -> Result<(), Failure> {
        let seam = self.seam;
        if planned.is_empty() {
            return Ok(());
        }
        let mut profile = scoped(seam, |meter| {
            Profile::parse(prefabs.profile, meter).map_err(failure::gltf)
        })?;
        let (planned, _plan_charge) = planned.into_parts();
        for planned in planned {
            seam.stop()?;
            let path_id = planned.pointer.path_id;
            let result = scoped(seam, |meter| {
                let (root, _owner) = seam
                    .env()
                    .deref_held(&planned.source_file, planned.pointer, meter)
                    .map_err(|error| classify::deref(&error))?;
                let old = if let Some(index) = planned.selection {
                    let selected = prefabs.selections.get(index).ok_or_else(internal)?;
                    let clips = unityrs_gltf::parse_clip_requests(selected.clips, meter)
                        .map(Clips::Explicit)
                        .map_err(failure::gltf)?;
                    Some(std::mem::replace(&mut profile.clips, clips))
                } else {
                    None
                };
                let result = render(seam, meter, prefabs.files, &profile, &root);
                if let Some(old) = old {
                    profile.clips = old;
                }
                result
            });
            seam.stop()?;
            let (bytes, issues) = match result {
                Ok(value) => value,
                Err(Class::Skip(reason)) => {
                    self.prefab_skip(planned.row, Some(path_id), reason)?;
                    continue;
                }
                Err(fatal) => return Err(fatal.into()),
            };
            let (bytes, mut charge) = bytes.into_parts();
            let cost = table::string(planned.key.len()).saturating_mul(2);
            match seam.probe(|| charge.grow(cost).map_err(|e| classify::charge(&e))) {
                Ok(()) => {}
                Err(Class::Skip(reason)) => {
                    self.prefab_skip(planned.row, Some(path_id), reason)?;
                    continue;
                }
                Err(fatal) => return Err(fatal.into()),
            }
            for issue in issues
                .as_slice()
                .iter()
                .filter(|issue| failure::degraded(issue.reason))
            {
                let records = &mut self.records;
                seam.probe(|| {
                    records
                        .strings
                        .grow(table::string(planned.key.len()))
                        .map_err(|e| classify::charge(&e))?;
                    table::push(
                        &mut records.glb_diagnostics,
                        GlbDiagnostic {
                            resource: issue.resource,
                            key: copy_key(&planned.key)?,
                            node: issue.node,
                            reason: issue.reason,
                        },
                    )
                })?;
            }
            let (serialized_file, path_id) = planned.row.root.clone().ok_or_else(internal)?;
            self.emit(Output {
                prefab: Some(planned.row),
                plan_index: planned.index,
                unit: Some(copy_key(&planned.key)?),
                key: planned.key,
                kind: OutputKind::Glb,
                serialized_file,
                path_id,
                sprite: None,
                md5: md5(&bytes),
                bytes,
                picture: None,
                members: Vec::new(),
                _charge: Some(charge),
            })?;
        }
        Ok(())
    }

    fn prefab_skip(
        &mut self,
        row: PrefabRow,
        path_id: Option<i64>,
        reason: Reason,
    ) -> Result<(), Failure> {
        let seam = self.seam;
        let records = &mut self.records;
        let skips = if self.exporting {
            &mut records.prefab_export_skips
        } else {
            &mut records.prefab_plan_skips
        };
        let cost = table::string("GameObject".len())
            .saturating_add(table::string(reason.reason.len()))
            .saturating_add(table::string(reason.detail.as_ref().map_or(0, String::len)));
        seam.probe(|| {
            records
                .strings
                .grow(cost)
                .map_err(|e| classify::charge(&e))?;
            table::push(
                skips,
                PrefabSkip {
                    skip: Skip {
                        serialized_file: row.serialized_file.clone(),
                        path_id: path_id.unwrap_or_default(),
                        class: "GameObject".to_owned(),
                        output: SkipOutput::Glb,
                        sprite: None,
                        reason,
                    },
                    row,
                },
            )
        })?;
        #[cfg(feature = "test-hooks")]
        seam.hooks.skipped(self.exporting);
        Ok(())
    }
}

fn copy_key(key: &str) -> Result<String, Class> {
    String::from_utf8(table::copied(key.as_bytes())?).map_err(|_| internal())
}

fn expand(
    template: &Template,
    input: &str,
    path_id: Option<i64>,
    name: Option<&RawStr>,
    path: Option<&RawStr>,
    key: &mut Vec<u8>,
) -> Result<(), Class> {
    let mut digits = [0; 24];
    let mut write = digits.as_mut_slice();
    write!(write, "{}", path_id.unwrap_or_default()).map_err(|_| internal())?;
    let len = 24 - write.len();
    template.expand(
        |variable| match variable {
            Var::Input => input.as_bytes(),
            Var::PathId => digits.get(..len).unwrap_or_default(),
            Var::Type => b"GameObject",
            Var::Name => name.map_or(&[], RawStr::as_bytes),
            Var::ContainerPath => path.map_or(&[], RawStr::as_bytes),
            _ => &[],
        },
        key,
    );
    Ok(())
}

fn scoped<T>(
    seam: Seam<'_>,
    run: impl FnOnce(&Arc<Meter>) -> Result<T, Class>,
) -> Result<T, Class> {
    let meter = seam.meter.work_scope(seam.hooks.export_work_remaining());
    let result = seam.env_call(|| run(&meter));
    if !matches!(result, Err(Class::Run(_))) {
        seam.work(meter.attempted_work())?;
    }
    result
}

type Glb = (MeteredVec<u8>, MeteredVec<unityrs_gltf::CollectionIssue>);
fn render(
    seam: Seam<'_>,
    meter: &Arc<Meter>,
    files: &crate::gltf::FileIndex,
    profile: &Profile,
    root: &ObjectRef,
) -> Result<Glb, Class> {
    let graph = Unit::Prefab(root.clone())
        .graph(seam.env(), &GraphOptions::default(), meter)
        .map_err(|e| failure::scene(&e))?;
    let collected = crate::gltf::collect_with_view(
        seam.env(),
        crate::gltf::Addresses::Indexed(files),
        &graph,
        profile,
        &UnlitExtras,
        meter,
    )
    .map_err(failure::export)?;
    let scene = collected.scene;
    if scene.meshes.is_empty()
        && let Some(issue) = scene
            .collection_issues
            .as_slice()
            .iter()
            .find(|issue| failure::required_renderer(issue.reason))
    {
        return Err(failure::skip("invalid_glb", Some(issue.reason)));
    }
    drop(collected.clips);
    drop(collected.discovery);
    drop(graph);
    let mut output = Buffer {
        bytes: MeteredVec::new(meter),
        error: None,
        hooks: seam.hooks,
    };
    let result = unityrs_gltf::write_glb(&scene, &profile.options(), meter, &mut output);
    if let Some(error) = output.error {
        return Err(classify::charge(&error));
    }
    if seam.hooks.stopped() {
        // The caller checks the stop before considering this result.
        return Err(internal());
    }
    result.map_err(failure::gltf)?;
    meter
        .count_decode(output.bytes.len() as u64)
        .map_err(|e| classify::charge(&e))?;
    Ok((output.bytes, scene.collection_issues))
}

struct Buffer<'a> {
    bytes: MeteredVec<u8>,
    error: Option<ChargeError>,
    hooks: &'a dyn super::Hooks,
}
impl io::Write for Buffer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.hooks.stopped() {
            return Err(io::ErrorKind::Other.into());
        }
        self.bytes.extend_from_slice(bytes).map_err(|error| {
            self.error = Some(error);
            io::Error::from(io::ErrorKind::Other)
        })?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
