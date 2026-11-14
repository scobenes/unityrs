//! UnityCN failures keep the existing per-input refusal vocabulary.
use std::sync::Arc;

use unityrs_core::env::EnvOptions;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, Error, FileContext, RawStr, Unsupported};
use unityrs_export::ngui::{self, Class, Data, Failure, NoHooks, Reason};
use unityrs_testkit::bundle::{BundleSpec, build};

#[path = "../../unityrs-core/tests/support/unitycn.rs"]
#[allow(dead_code)]
mod support;

#[test]
fn every_unitycn_error_is_a_data_refusal() {
    for (error, code) in [
        (Error::UnityCnKeyMissing, "unsupported"),
        (Error::UnityCnKeyLength { length: 15 }, "unsupported"),
        (Error::UnityCnSignature, "malformed"),
        (Error::UnityCnVectorLength { length: 15 }, "malformed"),
        (Error::Unsupported(Unsupported::UnityCnWalk), "unsupported"),
    ] {
        let want = Class::Skip(Reason::decode_error(code));
        assert_eq!(ngui::classify::error(&error), want, "{error}");
        let wrapped = Error::Entry {
            name: RawStr::from("inner.bundle"),
            source: Arc::new(error),
        };
        assert_eq!(ngui::classify::error(&wrapped), want);
    }
}

#[test]
fn keyless_and_wrong_key_inputs_are_refused_and_the_next_input_runs() {
    let encrypted = Bytes::from(support::bundle(
        b"2022.3.0f1",
        7,
        0x1000,
        true,
        false,
        false,
    ));
    let clear = Bytes::from(build(&BundleSpec::default()));
    for (key, detail) in [
        (None, "unsupported"),
        (Some(b"synthetic-key-02"), "malformed"),
    ] {
        let mut context = FileContext::default();
        if let Some(key) = key {
            context.set_assetbundle_decrypt_key(key).unwrap();
        }
        let context = Arc::new(context);
        let env = EnvOptions::batch(context.clone());
        let mut completed = 0;
        for (name, data) in [("encrypted.bundle", &encrypted), ("clear.bundle", &clear)] {
            let name_raw = RawStr::from(name);
            let meter = Meter::unlimited();
            let peek =
                ngui::peek(&name_raw, Data::Bytes(data), &context, &meter, &NoHooks).map(|_| ());
            // Attempts can open a bundle again after the peek barrier.
            let attempt = ngui::Registration::open(
                ngui::Staged {
                    id: name,
                    name: &name_raw,
                    data: Data::Bytes(data),
                },
                &[],
                ngui::Admit {
                    env: &env,
                    meter: &meter,
                    hooks: &NoHooks,
                    max_evictions: u64::MAX,
                },
            )
            .map(|_| ());
            assert_eq!(attempt, peek, "peek and attempt must agree");
            match peek {
                Ok(()) => completed += 1,
                Err(Failure::Input(reason)) => {
                    assert_eq!(name, "encrypted.bundle");
                    assert_eq!(reason.reason, "container_error");
                    assert_eq!(reason.detail.as_deref(), Some(detail));
                }
                Err(other) => panic!("a UnityCN input must not stop the run: {other:?}"),
            }
        }
        assert_eq!(completed, 1, "the next input must run after the refusal");
    }
}
