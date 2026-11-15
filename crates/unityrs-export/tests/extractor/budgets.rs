use super::*;
use unityrs_testkit::{
    ngui::{Object, SpriteSpec, s, st, texture, vector},
    scene3d::{Layout, file},
};

#[test]
fn atlas_search_does_not_hide_a_candidates_memory_refusal() {
    let bytes = file(
        Layout::Unity2022,
        &[],
        &[
            SpriteSpec::rect("sprite.png", [0., 0., 2., 2.], (0, 1))
                .tagged(&["wanted"])
                .object(2),
            texture(1, b"texture", 4, (2, 2), vec![100; 16]),
            Object::new(
                3,
                687_078_895,
                st(
                    "SpriteAtlas",
                    vec![
                        ("m_Name", s("other")),
                        ("unused", vector(s(""), vec![s("x"); 4096])),
                    ],
                ),
            ),
        ],
    )
    .unwrap();
    let mut options = EnvOptions::unitypy();
    let mut context = (*options.context).clone();
    context.limits.max_input_memory = 64 << 10;
    options.context = Arc::new(context);
    let env = Environment::new(options);
    env.load(Source::Bytes {
        name: RawStr::from("synthetic.assets"),
        data: Bytes::from(bytes),
    })
    .unwrap();
    let filter = Filter(vec![213]);
    let result = extractor::extract_assets(
        &env,
        &Options {
            use_container: false,
            asset_filter: Some(&filter),
            ..Options::default()
        },
        &mut Files::default(),
        &Meter::unlimited(),
    );
    match result {
        Err(e) => assert!(e.resource_refusal(), "{e:?}"),
        Ok(report) => assert!(
            report
                .failures
                .as_slice()
                .iter()
                .any(|f| f.error.resource_refusal()),
            "atlas candidate memory refusal was swallowed"
        ),
    }
}
