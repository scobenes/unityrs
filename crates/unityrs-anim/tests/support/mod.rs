use std::sync::Arc;
use unityrs_anim::{AnimError, BoundClip, Clip, RigPaths, bind_clip};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, ObjectRef, RawStr};
use unityrs_scene::{Graph, GraphOptions, Trs, graph_from_game_object};
use unityrs_testkit::ngui::Object;
use unityrs_testkit::scene3d::{self as s, Layout};

pub fn load(clip: Object, renderer: bool) -> (ObjectRef, Graph) {
    let env = Environment::new(EnvOptions::unitypy());
    let id = clip.path_id;
    let components = if renderer {
        vec![(0, 2), (0, 3)]
    } else {
        vec![(0, 2)]
    };
    let mut objects = vec![
        Object::new(1, 1, s::game_object(b"root", &components)),
        Object::new(2, 4, s::transform((0, 1), (0, 0), &[(0, 5)], false)),
        Object::new(4, 1, s::game_object(b"Child", &[(0, 5)])),
        Object::new(5, 4, s::transform((0, 4), (0, 2), &[], false)),
        clip,
    ];
    if renderer {
        objects.push(Object::new(
            3,
            137,
            s::skinned_mesh_renderer((0, 1), (0, 0), &[], &[]),
        ));
    }
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"clip-fixture"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], &objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("serialized clip fixture")
    };
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        &Meter::unlimited(),
    )
    .unwrap();
    (file.object(id).unwrap(), graph)
}

pub fn rest(graph: &Graph) -> Vec<Trs> {
    graph.nodes().iter().map(|n| n.local).collect()
}

pub fn bind<'a>(
    clip: &'a Clip,
    graph: &Graph,
    meter: &Arc<Meter>,
) -> Result<BoundClip<'a, 'static>, AnimError> {
    bind_clip(clip, &RigPaths::new(graph, 0, meter)?, &[], &[], meter)
}

pub fn bind_morph<'a, 'r>(
    clip: &'a Clip,
    graph: &Graph,
    table: &'r [unityrs_anim::RendererChannels<'r>],
    meter: &Arc<Meter>,
) -> Result<BoundClip<'a, 'r>, AnimError> {
    bind_clip(clip, &RigPaths::new(graph, 0, meter)?, &[], table, meter)
}
