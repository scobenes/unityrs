use super::publication_done;
use crate::{reason::Code, test_alloc::REFUSE};
#[test]
fn done_path_refusal_preserves_the_acknowledged_running_status() {
    REFUSE.with(|r| r.set(1));
    let result = publication_done(std::path::Path::new("/spool/runs/r"));
    assert!(
        REFUSE.with(|r| r.replace(0)) == 0,
        "path allocation was not reached"
    );
    let error = result.unwrap_err();
    assert_eq!(error.code, Code::Host);
    assert_eq!(error.reason, "host_memory");
    assert_eq!(error.status, Some("running"));
}

#[test]
fn work_limit_reason_refusal_does_not_abort() {
    let before = crate::alloc::refusals();
    REFUSE.with(|r| r.set(1));
    let failure =
        crate::attempt::classify::record(crate::records::Error::Work(crate::reason::Limit {
            kind: crate::reason::LimitKind::ExportWork,
            value: u64::MAX,
        }));
    assert!(
        REFUSE.with(|r| r.replace(0)) == 0,
        "reason did not allocate"
    );
    assert!(crate::alloc::refusals() > before);
    // The allocation-free sentinel propagates to the attempt's final refusal
    // audit, which overrides the outcome with host_memory.
    let unityrs_export::ngui::Failure::Input(reason) = failure else {
        panic!("input limit");
    };
    assert_eq!(reason.reason, "");
}

#[test]
fn peek_disk_long_path_refusal_is_host_memory() {
    let meter = unityrs_core::limits::Meter::unlimited();
    let path = std::path::PathBuf::from("x".repeat(1024));
    REFUSE.with(|r| r.set(1));
    let result = crate::peek::load_bits(0, Some(&path), &meter, &unityrs_export::ngui::NoHooks);
    assert!(REFUSE.with(|r| r.replace(0)) == 0, "path did not allocate");
    let unityrs_export::ngui::Failure::Run(reason) = result.unwrap_err() else {
        panic!("run failure");
    };
    assert_eq!(reason.reason, "host_memory");
}

#[test]
fn batch_reasons_keep_the_limit_and_detail_without_heap_formatting() {
    let failure =
        crate::attempt::classify::record(crate::records::Error::Work(crate::reason::Limit {
            kind: crate::reason::LimitKind::ExportWork,
            value: u64::MAX,
        }));
    let unityrs_export::ngui::Failure::Input(reason) = failure else {
        panic!("input limit");
    };
    assert_eq!(reason.reason, "limit(export_work, 18446744073709551615)");
    assert_eq!(reason.detail, None);
    let reason = crate::attempt::classify::reason("container_error", Some("asset_bundle"));
    assert_eq!(reason.reason, "container_error");
    assert_eq!(reason.detail.as_deref(), Some("asset_bundle"));
}

#[test]
fn either_owned_reason_field_can_refuse_without_a_fallback_allocation() {
    for ordinal in [1, 2] {
        let before = crate::alloc::refusals();
        REFUSE.with(|r| r.set(ordinal));
        let reason = crate::attempt::classify::reason("container_error", Some("asset_bundle"));
        assert_eq!(REFUSE.with(|r| r.replace(0)), 0);
        assert!(crate::alloc::refusals() > before);
        assert_eq!(reason.reason, "");
        assert_eq!(reason.detail, None);
    }
}
