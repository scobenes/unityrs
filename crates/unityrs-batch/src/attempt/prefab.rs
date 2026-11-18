//! Input-local overrides and lightweight file addresses for the private view.
use std::sync::Arc;
use unityrs_core::{
    files::{BundleFile, File},
    limits::{Meter, MeteredVec},
};
use unityrs_export::{
    gltf::FileIndex,
    ngui::{self, Context, Failure, Hooks, Prefabs, Selection, Template},
};

use super::{
    classify,
    hooks::{Engine, probe},
};
use crate::spec::model::Glb;

pub(super) type Options<'a> = (&'a Template, &'a Glb);

pub(super) struct Attempt<'a> {
    options: Option<Options<'a>>,
    selections: MeteredVec<Selection<'a>>,
    files: FileIndex,
    explicit: bool,
}
impl<'a> Attempt<'a> {
    pub(super) fn new(
        options: Option<Options<'a>>,
        input: &str,
        meter: &Arc<Meter>,
        engine: &Engine<'_>,
    ) -> Result<Self, Failure> {
        let mut selections = MeteredVec::new(meter);
        let mut explicit = false;
        if let Some((_, glb)) = options {
            for (i, row) in glb
                .overrides
                .iter()
                .enumerate()
                .filter(|(_, row)| row.input == input)
            {
                engine.check()?;
                let ordinal = u32::try_from(i).map_err(|_| classify::internal())?;
                probe(engine, || {
                    selections
                        .push(Selection {
                            key: &row.key,
                            clips: &row.clips,
                            ordinal,
                            found: false,
                        })
                        .map_err(|e| ngui::classify::charge(&e))
                })?;
                explicit |= row.clips != b"[]";
            }
            selections
                .as_mut_slice()
                .sort_unstable_by(|a, b| a.key.cmp(b.key));
        }
        Ok(Self {
            options,
            selections,
            files: FileIndex::new(meter),
            explicit,
        })
    }

    pub(super) fn loaded(
        &mut self,
        own: bool,
        registered: bool,
        bundle: &Arc<BundleFile>,
        attempt: &Arc<Meter>,
        engine: &Engine<'_>,
    ) -> Result<(), Failure> {
        if self.explicit {
            let meter = attempt.work_scope(engine.export_work_remaining());
            let result = probe(engine, || {
                self.files
                    .add(File::Bundle(bundle.clone()), own, registered, &meter)
                    .map_err(ngui::classify_gltf)
            });
            if !matches!(result, Err(Failure::Run(_))) {
                engine
                    .export_work(meter.attempted_work())
                    .map_err(Failure::Input)?;
            }
            result?;
        }
        Ok(())
    }

    pub(super) fn cut(
        &mut self,
        context: Context<'_>,
        options: &ngui::Options,
        missing: &mut Option<u32>,
    ) -> Result<ngui::Cut, Failure> {
        match self.options {
            Some((template, policy)) => ngui::cut_prefabs(
                context,
                options,
                Prefabs {
                    template,
                    profile: &policy.profile,
                    selections: self.selections.as_mut_slice(),
                    files: &self.files,
                    missing,
                },
            ),
            None => ngui::cut(context, options),
        }
    }
}
