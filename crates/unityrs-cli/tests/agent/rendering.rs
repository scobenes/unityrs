use super::support::{fixture, json, run};
use unityrs_core::{
    env::{EnvOptions, Environment, Source},
    files::File,
};
use unityrs_dump::encode::encode_value_into;
use unityrs_testkit::ngui::{self, Object, V, s, st, vector};

fn refuses_rendering_work(name: &str, value: V, cap: &str) {
    let tmp = fixture();
    let file = format!("{name}.assets");
    std::fs::write(
        tmp.join(&file),
        ngui::file(&[], &[Object::new(1, 114, value)], true).unwrap(),
    )
    .unwrap();
    for mode in ["natural", "canonical"] {
        let error = json(
            &tmp,
            &[
                "show",
                &file,
                "--path-id",
                "1",
                "--bytes",
                "full",
                "--max-bytes",
                cap,
                "--max-decode-work",
                "4096",
                "--value",
                mode,
            ],
            1,
        );
        assert_eq!(error["code"], "budget", "{name}/{mode}: {error}");
        let success = json(
            &tmp,
            &[
                "show",
                &file,
                "--path-id",
                "1",
                "--bytes",
                "full",
                "--max-bytes",
                cap,
                "--value",
                mode,
            ],
            0,
        );
        assert_eq!(success["truncated"]["cuts"], 0, "{name}/{mode}");
    }
}

fn canonical_fixtures() -> Vec<Object> {
    let scalar_values = vec![
        V::Bool(true),
        V::Int(-3),
        V::UInt(4),
        V::UInt8(5),
        V::Int64(i64::MIN),
        V::UInt64(u64::MAX),
        V::Float(1.5),
        V::Double(-0.0),
        s("a\n\"é"),
        V::Str(vec![0xff; 65]),
        V::Str(vec![0xff; 64]),
        V::Bytes(vec![7; 64]),
        V::Bytes(vec![7; 65]),
    ];
    let mut objects = vec![Object::new(
        1,
        114,
        st(
            "Kinds",
            scalar_values
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, v)| (format!("field{i}"), v))
                .collect::<Vec<_>>()
                .iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect(),
        ),
    )];
    for (i, value) in scalar_values.into_iter().enumerate() {
        objects.push(Object::new(
            10 + i64::try_from(i).unwrap(),
            114,
            vector(value.clone(), vec![value.clone(), value]),
        ));
    }
    objects.push(Object::new(
        30,
        114,
        vector(V::Float(0.0), vec![V::Float(1.5), V::Float(2.5)]),
    ));
    objects.push(Object::new(31, 114, vector(V::UInt8(0), vec![])));
    objects.push(Object::new(32, 114, vector(V::Float(0.0), vec![])));
    objects.push(Object::new(33, 114, ngui::pair(s("key"), V::Int(2))));
    objects.push(Object::new(
        34,
        114,
        V::Vector(
            "map".into(),
            Box::new(ngui::pair(s(""), V::Int(0))),
            vec![ngui::pair(s("key"), V::Int(2))],
        ),
    ));
    objects
}

#[test]
fn canonical_values_match_dump_encoding_for_every_fixture_kind() {
    let tmp = fixture();
    let objects = canonical_fixtures();
    std::fs::write(
        tmp.join("kinds.assets"),
        ngui::file(&[], &objects, true).unwrap(),
    )
    .unwrap();
    for filename in ["synthetic.assets", "rich.assets", "kinds.assets"] {
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env.load(Source::Path(tmp.join(filename))).unwrap() else {
            panic!()
        };
        for obj in file.objects_by_path() {
            let parsed = obj.parse_as_dict(false).unwrap();
            let mut expected = String::new();
            encode_value_into(&mut expected, &parsed, parsed.value()).unwrap();
            let output = run(
                &tmp,
                &[
                    "show",
                    filename,
                    "--path-id",
                    &obj.path_id().to_string(),
                    "--value",
                    "canonical",
                    "--max-bytes",
                    "100000",
                ],
            );
            assert!(output.status.success(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            let actual = text
                .split_once(",\"value\":")
                .unwrap()
                .1
                .rsplit_once(",\"truncated\":")
                .unwrap()
                .0;
            assert_eq!(actual, expected, "{filename}/{}", obj.path_id());
            let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(doc["truncated"]["cuts"], 0);
            let schema =
                serde_json::from_slice(&run(&tmp, &["describe", "--schema", "show"]).stdout)
                    .unwrap();
            assert!(super::support::valid(&doc, &schema, &schema), "{doc}");
        }
    }
}

#[test]
fn full_hex_refuses_excess_rendering_work() {
    refuses_rendering_work("bytes", V::Bytes(vec![0xab; 1 << 20]), "2200000");
}
#[test]
fn string_scanning_refuses_excess_rendering_work() {
    refuses_rendering_work("string", s(&"x".repeat(10_000)), "100000");
}
#[test]
fn string_escaping_refuses_excess_rendering_work() {
    refuses_rendering_work("escaped", s(&"\n".repeat(10_000)), "100000");
}
