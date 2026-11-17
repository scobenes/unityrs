use super::*;
use unityrs_core::limits::Meter;
use unityrs_testkit::{
    ngui::{Object, s, st},
    scene3d::{self, Layout},
};

struct OldFilter(Vec<i32>);
impl AssetFilter for OldFilter {
    fn accepts(&self, object: &ObjectRef, _: Option<&Parsed>) -> bool {
        self.0.contains(&object.class_id())
    }
}
#[derive(Default)]
struct MemorySink(Vec<(Vec<u8>, Vec<u8>)>);
impl Sink for MemorySink {
    fn directory(&mut self, _: &[u8]) -> Result<(), extractor::Error> {
        Ok(())
    }
    fn file(&mut self, name: &[u8], bytes: &[u8]) -> Result<(), extractor::Error> {
        self.0.push((name.to_vec(), bytes.to_vec()));
        Ok(())
    }
}
fn fixture() -> Environment {
    let objects = [
        (1, 49, "a"),
        (2, 49, "b"),
        (3, 128, "font"),
        (4, 114, "script"),
    ]
    .map(|(id, class, name)| {
        Object::new(
            id,
            class,
            st(
                "Synthetic",
                vec![("m_Name", s(name)), ("m_Script", s("payload"))],
            ),
        )
    });
    let env = Environment::new(EnvOptions::unitypy());
    env.load(Source::Bytes {
        name: "synthetic.assets".into(),
        data: scene3d::file(Layout::Unity2022, &[], &objects)
            .unwrap()
            .into(),
    })
    .unwrap();
    env
}
fn filter(ids: &[i32], meter: &Arc<Meter>) -> Filter {
    let mut values = MeteredVec::new(meter);
    values.extend_from_slice(ids).unwrap();
    Filter::new(values, meter).unwrap()
}
#[test]
fn filter_lookups_refuse_work_even_when_every_object_is_excluded() {
    let env = fixture();
    let meter = Meter::unlimited();
    let old = OldFilter(vec![-3, -2, -1]);
    let options = Options {
        use_container: false,
        asset_filter: Some(&old),
        ..Options::default()
    };
    extractor::extract_assets(&env, &options, &mut MemorySink::default(), &meter).unwrap();
    let mut limits = unityrs_core::limits::Limits::default();
    limits.max_decode_work = meter.decoded();
    let meter = limits::extraction(&limits);
    // Build the filter separately so the remaining allowance isolates lookup work.
    let filter = filter(&[-3, -2, -1], &Meter::unlimited());
    let options = Options {
        asset_filter: Some(&filter),
        ..options
    };
    let result = extractor::extract_assets(&env, &options, &mut MemorySink::default(), &meter);
    assert!(matches!(result, Err(ref e) if e.resource_refusal()));
}
#[test]
fn several_filters_and_duplicates_match_old_loop_reports_and_files() {
    let env = fixture();
    for ids in [
        vec![49, 128, 114, 28],
        vec![128, 49, 49, 114, 128],
        vec![28, -1, 49, 28],
        vec![-3, -2, -1],
    ] {
        let meter = Meter::unlimited();
        let old = OldFilter(ids.clone());
        let options = Options {
            use_container: false,
            asset_filter: Some(&old),
            ..Options::default()
        };
        let mut old_sink = MemorySink::default();
        let old_report = extractor::extract_assets(&env, &options, &mut old_sink, &meter).unwrap();
        let filter = filter(&ids, &meter);
        let options = Options {
            asset_filter: Some(&filter),
            ..options
        };
        let mut new_sink = MemorySink::default();
        let new_report = extractor::extract_assets(&env, &options, &mut new_sink, &meter).unwrap();
        assert_eq!(
            report::json(&old_report, &meter, false).unwrap().as_bytes(),
            report::json(&new_report, &meter, false).unwrap().as_bytes()
        );
        assert_eq!(old_sink.0, new_sink.0);
    }
}

#[test]
fn duplicate_filters_use_the_same_lookup_work_as_their_union() {
    let env = fixture();
    let mut work = Vec::new();
    for ids in [vec![-3, -2, -1], vec![-1, -2, -3, -1, -3, -2]] {
        let meter = Meter::unlimited();
        let filter = filter(&ids, &meter);
        let before = meter.decoded();
        let options = Options {
            use_container: false,
            asset_filter: Some(&filter),
            ..Options::default()
        };
        let report =
            extractor::extract_assets(&env, &options, &mut MemorySink::default(), &meter).unwrap();
        assert!(report.written.is_empty());
        work.push(meter.decoded() - before);
    }
    assert_eq!(work[0], work[1]);
}
