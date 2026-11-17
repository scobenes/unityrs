//! Caller-owned providers share one finite memory and work account.
mod common;

use common::{Blob, stored, v2};
use std::sync::Arc;
use unityrs_core::{
    LimitKind,
    budget::{Reservation, Unbounded},
    limits::{ChargeError, Limits, Meter},
    provider::{TreeRequest, TypeTreeProvider},
};
use unityrs_tpk::{TpkError, TpkProvider};

fn meter(limits: &Limits) -> Arc<Meter> {
    Meter::for_input(limits, Arc::new(Unbounded), Reservation::Input)
}

#[test]
fn loaded_tables_use_and_release_the_callers_memory_account() {
    let bytes = stored(
        &Blob {
            strings: 4096,
            ..Blob::default()
        }
        .write()
        .0,
    );
    let mut limits = Limits::default();
    limits.max_input_memory = 4096;
    let account = meter(&limits);
    assert!(
        matches!(TpkProvider::from_bytes_metered(&bytes, &limits, &account), Err(TpkError::Resource(ChargeError::Limit(l))) if l.kind() == LimitKind::InputMemory)
    );
    assert_eq!(account.used(), 0);
    // The established provider API retains its independent parity defaults.
    assert!(TpkProvider::from_bytes(&bytes, &limits).is_ok());

    let limits = Limits::default();
    let account = meter(&limits);
    let provider = TpkProvider::from_bytes_metered(&bytes, &limits, &account).unwrap();
    assert!(account.used() > 4096);
    drop(provider);
    assert_eq!(account.used(), 0);
}

#[test]
fn lazy_tree_expansion_obeys_the_callers_memory_and_work_limits() {
    // Twelve compact records expand to 4095 rows on the first tree request.
    let nodes = (0..12_u16)
        .map(|n| if n == 11 { vec![] } else { vec![n + 1; 2] })
        .collect();
    let bytes = stored(&Blob::class_1(nodes).write().0);
    for kind in [LimitKind::InputMemory, LimitKind::DecodeWork] {
        let mut limits = Limits::default();
        if kind == LimitKind::InputMemory {
            limits.max_input_memory = 128 << 10;
        } else {
            limits.max_decode_work = 4096;
        }
        let account = meter(&limits);
        let provider = TpkProvider::from_bytes_metered(&bytes, &limits, &account).unwrap();
        let version = v2();
        let error = provider
            .tree(&TreeRequest {
                class_id: 1,
                version: &version,
                script: None,
            })
            .unwrap_err();
        assert_eq!(error.limit.unwrap().kind(), kind, "{error}");
        assert!(account.used() <= limits.max_input_memory);
        assert!(account.decoded() <= limits.max_decode_work);
        drop(provider);
        assert_eq!(account.used(), 0);
    }
}

#[test]
fn a_tree_refused_by_current_memory_pressure_can_retry_after_memory_is_released() {
    let bytes = stored(&Blob::class_1(vec![vec![]]).write().0);
    let mut limits = Limits::default();
    limits.max_input_memory = 128 << 10;
    let account = meter(&limits);
    let provider = TpkProvider::from_bytes_metered(&bytes, &limits, &account).unwrap();
    let version = v2();
    let request = TreeRequest {
        class_id: 1,
        version: &version,
        script: None,
    };
    let held = account
        .charge(limits.max_input_memory - account.used())
        .unwrap();
    let error = provider.tree(&request).unwrap_err();
    assert_eq!(error.limit.unwrap().kind(), LimitKind::InputMemory);
    drop(held);
    assert!(provider.tree(&request).unwrap().is_some());
    drop(provider);
    assert_eq!(account.used(), 0);
}
