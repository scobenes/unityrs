//! Explicit library export units. Dependencies remain in the Environment;
//! a scene unit reads only its selected file's root table.
use crate::{Graph, GraphOptions, SceneError, graph_from_game_object, graph_from_scene_file};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, container::ContainerRow, env::Environment, files::SerializedFile, limits::Meter,
};

/// One complete scene file, or one prefab `GameObject` subtree.
/// A `SceneAsset` pointer is a marker selecting its owning file as a scene.
#[derive(Clone, Debug)]
pub enum Unit {
    Scene(Arc<SerializedFile>),
    Prefab(ObjectRef),
}

impl Unit {
    /// The selected input's meter, including its lazy object reads.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        match self {
            Self::Scene(file) => file.meter(),
            Self::Prefab(root) => root.file().meter(),
        }
    }

    /// Build the selected graph. Scene roots retain object-table order;
    /// prefab roots retain only the chosen subtree and its stored child order.
    pub fn graph(
        &self,
        env: &Environment,
        options: &GraphOptions,
        meter: &Arc<Meter>,
    ) -> Result<Graph, SceneError> {
        match self {
            Self::Scene(file) => graph_from_scene_file(env, file, options, meter),
            Self::Prefab(root) => graph_from_game_object(env, root, options, meter),
        }
    }
}

/// Select a prefab root or a `SceneAsset`'s complete owning scene file.
/// This reads no marker payload and does not guess a scene from a file name.
pub fn unit_from_object(object: &ObjectRef) -> Result<Unit, SceneError> {
    match object.class_id() {
        1 => Ok(Unit::Prefab(object.clone())),
        1032 => Ok(Unit::Scene(object.file().clone())),
        class => Err(SceneError::WrongClass(class)),
    }
}

/// Select one container entry, resolving cross-file pointers normally.
/// Non-`GameObject`/`SceneAsset` entries return `None`. A broken selected
/// pointer returns its error; resource refusals retain their provenance.
/// Each entry is a unit even when several entries target the same root.
pub fn unit_from_container(
    env: &Environment,
    file: &Arc<SerializedFile>,
    row: &ContainerRow,
) -> Result<Option<Unit>, SceneError> {
    let pointer = row
        .asset()
        .ok_or(SceneError::Malformed("container asset pointer"))?;
    let object = env.deref(file, pointer)?;
    match object.class_id() {
        1 | 1032 => unit_from_object(&object).map(Some),
        _ => Ok(None),
    }
}

const _: () = crate::send_sync::<Unit>();
