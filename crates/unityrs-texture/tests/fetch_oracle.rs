//! Replay `ResourceReader` and `Texture2D` outcomes recorded by tools/fixtures/web.py.
//! `WebFile` parents use UnityPy's registry; contiguous bundle parents retain
//! the batch engine's documented Tier C own-bundle preference.
use std::cell::Cell;

use serde_json::Value;
use unityrs_core::Bytes;
use unityrs_core::env::{EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_texture::fetch::{self, Error, Hooks, Stream};

#[path = "support/fetch.rs"]
mod support;

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[derive(Default)]
struct Observed {
    probes: Cell<usize>,
    environment: Cell<usize>,
}

impl Hooks for Observed {
    type Error = Error;

    fn probe<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.probes.set(self.probes.get() + 1);
        f()
    }

    fn env_call<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.environment.set(self.environment.get() + 1);
        f()
    }

    fn row_error(&self, error: Error) -> Error {
        error
    }
}

#[test]
fn web_parents_keep_registry_shadows_and_bundles_keep_the_tier_c_exception() {
    let cases: Vec<Value> = include_str!("fixtures/fetch.json")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let inputs = support::inputs();
    assert_eq!(cases.len(), 12);
    assert_eq!(cases.len(), inputs.len());
    for (case, input) in cases.iter().zip(inputs) {
        assert_eq!(case["input"], input);
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let mut options = EnvOptions::unitypy();
            options.load = mode;
            let env = Environment::new(options);
            let shadow = || Source::Bytes {
                name: "r.resS".into(),
                data: Bytes::from(unhex(input["shadow_hex"].as_str().unwrap())),
            };
            let shadow_first = input["shadow_first"].as_bool().unwrap();
            if shadow_first {
                env.load(shadow()).unwrap();
            }
            let root = env
                .load(Source::Bytes {
                    name: "synthetic".into(),
                    data: Bytes::from(unhex(input["hex"].as_str().unwrap())),
                })
                .unwrap();
            // Hold both containers: parents are weak references.
            let nested = root
                .container_directory()
                .iter()
                .position(|row| row.path.as_bytes() == b"nested")
                .map(|i| root.container_entry(i).unwrap());
            let inner = nested.as_ref().unwrap_or(&root);
            let File::Serialized(from) = inner.container_entry(0).unwrap() else {
                panic!("serialized fixture member");
            };
            if !shadow_first {
                env.load(shadow()).unwrap();
            }
            let stream = Stream {
                path: input["path"].as_str().unwrap().into(),
                offset: i128::from(input["offset"].as_i64().unwrap()),
                size: i128::from(input["size"].as_i64().unwrap()),
            };
            let oracle = case["resource_hex"].as_str().unwrap();
            assert_eq!(case["resource_hex"], case["image_data_hex"]);
            let resource = env.resource(&from, &stream.path, 1, 5).unwrap();
            assert_eq!(support::hex(&resource), oracle, "{} {mode:?}", input["id"]);

            let own_bundle =
                matches!(inner, File::Bundle(_)) && !input["outer_only"].as_bool().unwrap();
            let hooks = Observed::default();
            let got = fetch::fetch_with(
                &env,
                &from,
                &Bytes::new(),
                Some(&stream),
                &Meter::unlimited(),
                &hooks,
            )
            .unwrap();
            let want = if own_bundle {
                b"nner ".to_vec()
            } else {
                unhex(oracle)
            };
            assert_eq!(got.as_ref(), want, "{} {mode:?}", input["id"]);
            assert_eq!(hooks.environment.get(), usize::from(!own_bundle));
            assert!(hooks.probes.get() > 0);
        }
    }
}
