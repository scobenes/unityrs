use super::*;

#[test]
fn the_innermost_owner_cannot_be_hidden_by_an_allowed_ancestor() {
    let trace = "REFUSED_ALLOCATION site=99 size=64\n\
        0: <controller_alloc::Refuse as unityrs_batch::alloc::Hook>::refuses\n\
        1: unityrs_batch::alloc::no_unwind::<bool, controller_alloc::Refuse>\n\
        2: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::alloc\n\
        3: <alloc::sync::Arc<unityrs_core::env::Environment>>::new\n\
        4: unityrs_batch::attempt::run_reserved\n\
        5: <unityrs_core::env::Environment>::new\n\
        END_REFUSED_ALLOCATION";
    assert_eq!(
        innermost(trace).as_deref(),
        Some("unityrs_batch::attempt::run_reserved")
    );
    assert_eq!(
        innermost(&trace.replace("unityrs_batch::attempt", "unityrs_cli::batch")).as_deref(),
        Some("unityrs_cli::batch::run_reserved")
    );
    // The CLI package's production binary is named `unityrs`.
    assert_eq!(
        innermost(&trace.replace("unityrs_batch::attempt", "unityrs::batch")).as_deref(),
        Some("unityrs::batch::run_reserved")
    );
}

#[test]
fn inline_functions_are_keys_but_std_implementations_and_type_mentions_are_not() {
    let trace = "REFUSED_ALLOCATION site=3 size=8\n\
        0: <unityrs_export::ngui::keys::Piece as <[_]>::to_vec_in::ConvertVec>::to_vec::<alloc::alloc::Global>\n\
                     at /rustc/hash/library/alloc/src/slice.rs:448:29\n\
        1: <unityrs_export::ngui::keys::Template as core::clone::Clone>::clone\n\
                     at /work/crates/unityrs-export/src/ngui/keys.rs:33:10\n\
        2: <unityrs_export::ngui::attempt::Attempt>::plan\n\
        END_REFUSED_ALLOCATION";
    assert_eq!(
        innermost(trace).as_deref(),
        Some("unityrs_export::ngui::keys::Template::clone")
    );
    assert_eq!(
        function_key(
            "unityrs_core::env::held::lock::<unityrs_core::env::registry::lru::Lru>::{closure#7}"
        )
        .as_deref(),
        Some("unityrs_core::env::held::lock")
    );
    assert_eq!(
        innermost("REFUSED_ALLOCATION site=1 size=64\n0: <unknown>\nEND_REFUSED_ALLOCATION"),
        None
    );
    assert_eq!(innermost("0: unityrs_core::env::Environment::new"), None);
}

/// The chain from our function decides a third party's admission: hyper's
/// `send_request` oneshot is listed, but the same allocation called by
/// ours (an engine-side `oneshot::channel`) is not.
#[test]
fn a_listed_allocator_called_by_ours_is_unlisted() {
    let trace = |callee: &str| {
        format!(
            "REFUSED_ALLOCATION site=22 size=64\n\
            0: <controller_alloc::Refuse as unityrs_batch::alloc::Hook>::refuses\n\
            1: unityrs_batch::alloc::no_unwind::<bool, controller_alloc::Refuse>\n\
            2: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::alloc\n\
            3: __rustc::__rust_alloc\n\
            4: <alloc::sync::Arc<tokio::sync::oneshot::Inner<u8>>>::new\n\
                     at /rustc/hash/library/alloc/src/sync.rs:1:1\n\
            5: tokio::sync::oneshot::channel::<u8>\n\
                     at /opt/cargo/registry/src/tokio/src/sync/oneshot.rs:1:1\n\
            {callee}\
            7: unityrs_batch::http::response::exchange_charged::<unityrs_batch::http::io::Stream>::{{closure#0}}\n\
                     at ./crates/unityrs-batch/src/http/response.rs:1:1\n\
            END_REFUSED_ALLOCATION"
        )
    };
    let admitted = |trace: &str| {
        INVENTORY
            .iter()
            .filter(|e| e.applies("url"))
            .find(|e| e.admits(innermost(trace).as_deref(), trace))
            .map(|e| e.name)
    };
    let hyper = trace(
        "6: <hyper::client::conn::http1::SendRequest<unityrs_batch::http::request::Payload>>::send_request\n\
                 at /opt/cargo/registry/src/hyper/src/client/conn/http1.rs:1:1\n",
    );
    assert_eq!(
        innermost(&hyper).as_deref(),
        Some("unityrs_batch::http::response::exchange_charged")
    );
    assert_eq!(admitted(&hyper), Some("hyper send_request oneshot"));
    // Ours: `black_box(tokio::sync::oneshot::channel::<u8>())`, inlined.
    let ours = trace(
        "6: core::hint::black_box::<u8>\n         at /rustc/hash/library/core/src/hint.rs:1:1\n",
    );
    assert_eq!(
        callee(&ours).as_deref(),
        Some("tokio::sync::oneshot::channel::<u8>")
    );
    assert_eq!(admitted(&ours), None, "an engine-side oneshot is UNLISTED");
}
