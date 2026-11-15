//! Path operations and serialized extraction, with no filesystem access.
use std::sync::Arc;
use unityrs_core::{
    Bytes, RawStr,
    budget::{Reservation, Unbounded},
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_export::extractor::{self, Error, Options, Sink, paths};

#[derive(Default)]
struct MemorySink {
    writes: usize,
}
impl Sink for MemorySink {
    fn directory(&mut self, path: &[u8]) -> Result<(), Error> {
        assert!(paths::validate(path).is_ok(), "unconfined directory");
        Ok(())
    }
    fn file(&mut self, path: &[u8], _: &[u8]) -> Result<(), Error> {
        assert!(paths::validate(path).is_ok(), "unconfined file");
        assert!(!path.is_empty());
        self.writes += 1;
        Ok(())
    }
}
/// Runs both interpretations. The write count lets tests require actual export.
pub fn check(data: &[u8]) -> usize {
    if data.len() > 8 << 20 {
        return 0;
    }
    let m = Meter::with_decode_work(1 << 16, 1 << 20, Arc::new(Unbounded), Reservation::Input);
    let ignore = i64::from(data.first().copied().unwrap_or(0) as i8);
    let a = paths::container(data, ignore, &m);
    let b = paths::container(data, ignore, &m);
    match (&a, &b) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a, b);
            assert!(paths::validate(a.as_bytes()).is_ok());
        }
        (Err(_), Err(_)) => {}
        _ => panic!("nondeterministic path outcome"),
    }
    let (stem, extension) = paths::split(data);
    assert_eq!(stem.len() + extension.len(), data.len());
    assert!(data.starts_with(stem) && data.ends_with(extension));
    drop((a, b));
    assert_eq!(m.used(), 0);

    let mut options = EnvOptions::unitypy();
    let mut context = (*options.context).clone();
    context.limits = crate::limits();
    context.limits.max_input_memory = 8 << 20;
    options.context = Arc::new(context);
    let env = Environment::new(options);
    if env
        .load(Source::Bytes {
            name: RawStr::from("synthetic.assets"),
            data: Bytes::copy_from_slice(data),
        })
        .is_err()
    {
        return 0;
    }
    let mut writes = 0;
    for use_container in [false, true] {
        let m = Meter::with_decode_work(1 << 16, 1 << 20, Arc::new(Unbounded), Reservation::Input);
        let mut sink = MemorySink::default();
        let result = extractor::extract_assets(
            &env,
            &Options {
                use_container,
                export_unknown_as_typetree: true,
                ..Options::default()
            },
            &mut sink,
            &m,
        );
        if let Ok(report) = &result {
            for w in report.written.as_slice() {
                assert!(paths::validate(w.path.as_bytes()).is_ok());
            }
        }
        assert!(m.used() <= 1 << 16);
        drop(result);
        assert_eq!(m.used(), 0, "leaked extraction charge");
        writes += sink.writes;
    }
    writes
}

#[cfg(test)]
mod tests {
    #[test]
    fn extraction_reaches_builders_and_releases_charges_on_mutations() {
        let mut writes = 0;
        for (_, bytes) in unityrs_testkit::mutate::builder_outputs() {
            writes += super::check(&bytes);
            for cut in [0, 1, bytes.len() / 2, bytes.len().saturating_sub(1)] {
                super::check(&bytes[..cut.min(bytes.len())]);
            }
        }
        assert!(writes > 0, "serialized builder seeds must reach extraction");
        for path in [
            b"../escape".as_slice(),
            b"a/../../escape",
            b"link/file",
            b"a\0b",
            b"a\xff.sgi",
        ] {
            super::check(path);
        }
    }
}
