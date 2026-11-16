use super::*;

#[test]
fn locationless_std_frames_do_not_mask_dependency_formatters() {
    let frame = "<serde_core::de::Unexpected as core::fmt::Display>::fmt";
    let via = "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::complete_multipart::{closure#0}";
    for source in [
        "",
        "             at /rustc/hash/library/core/src/fmt/mod.rs:1:1\n",
    ] {
        let trace = format!(
            "REFUSED_ALLOCATION site=424 size=100\n\
            0: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::realloc\n\
            1: __rustc::__rust_realloc\n\
            2: alloc::alloc::realloc_nonnull\n\
            3: core::fmt::write\n{source}\
            4: {frame}\n\
            5: core::fmt::write\n{source}\
            6: {via}\n\
            7: std::future::poll_fn\n\
            8: <unityrs_batch::store::multipart::Transfer>::complete::{{closure#0}}\n\
            END_REFUSED_ALLOCATION"
        );
        assert_eq!(allocating(&trace).as_deref(), Some(frame));
        assert_eq!(callee(&trace).as_deref(), Some(via));
        assert!(inventory().any(|e| {
            e.applies("store-multipart") && e.admits(innermost(&trace).as_deref(), &trace)
        }));
        let ours = trace.replace(frame, "unityrs_batch::store::multipart::allocate");
        assert!(!inventory().any(|e| {
            e.applies("store-multipart") && e.admits(innermost(&ours).as_deref(), &ours)
        }));
    }
}

#[test]
fn third_party_trait_implementations_on_alloc_types_are_allocating_frames() {
    let frame =
        "<alloc::borrow::Cow<str> as core::convert::From<serde_urlencoded::ser::key::Key>>::from";
    let trace = format!(
        "REFUSED_ALLOCATION site=1 size=32\n\
        0: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::alloc\n\
        1: {frame}\n\
                 at /opt/.cargo/registry/src/serde_urlencoded/src/ser/key.rs:1:1\n\
        2: <object_store::aws::AmazonS3 as object_store::list::PaginatedListStore>::list_paginated\n\
        3: <unityrs_batch::store::list::Listing>::page\n\
        END_REFUSED_ALLOCATION"
    );
    assert_eq!(allocating(&trace).as_deref(), Some(frame));
    assert!(inventory().any(|e| e.admits(innermost(&trace).as_deref(), &trace)));
}

#[test]
fn store_entries_name_the_public_attribute_and_abort_entry_points() {
    for (owner, frame, via) in [
        (
            "unityrs_batch::store::put::options",
            "hashbrown::alloc::inner::do_alloc::<alloc::alloc::Global>",
            "<object_store::attributes::Attributes>::insert",
        ),
        (
            "unityrs_batch::store::abort::upload",
            "<tokio::runtime::io::registration_set::RegistrationSet>::deregister",
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::abort_multipart",
        ),
    ] {
        let trace = format!(
            "REFUSED_ALLOCATION site=1 size=32\n\
            0: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::alloc\n\
            1: {frame}\n\
            2: {via}\n\
            3: {owner}\n\
            END_REFUSED_ALLOCATION"
        );
        let admitted = |trace: &str| {
            inventory().any(|e| e.applies("store") && e.admits(innermost(trace).as_deref(), trace))
        };
        assert!(admitted(&trace), "{trace}");
        assert!(!admitted(&trace.replace(via, "unlisted::call")));
    }
}

#[test]
fn darwin_socket_waiters_require_the_exact_mutex_owner_and_dependency_callee() {
    let trace = "REFUSED_ALLOCATION site=117 size=64\n\
        0: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::alloc\n\
        1: <tokio::loom::std::mutex::Mutex<tokio::runtime::io::scheduled_io::Waiters>>::lock\n\
        2: <tokio::io::async_fd::AsyncFd<unityrs_batch::sys::reset::Socket>>::async_io\n\
                 at /opt/.cargo/registry/src/tokio/src/io/async_fd.rs:1:1\n\
        3: <unityrs_batch::sys::reset::RegisteredSocket>::connected\n\
        END_REFUSED_ALLOCATION";
    let admitted = |trace: &str| inventory().find(|e| e.admits(innermost(trace).as_deref(), trace));
    let entry = admitted(trace).expect("per-registration Darwin waiters are inventoried");
    assert!(matches!(entry.platform, Platform::Darwin));
    assert_eq!(entry.applies("store"), cfg!(target_vendor = "apple"));
    for other in [
        trace.replace("scheduled_io::Waiters", "registration_set::Synced"),
        trace.replace("::async_io", "::new"),
        trace.replace("::async_io", "::async_io_mut"),
        trace.replace("::lock\n", "::lock_other\n"),
        trace.replace("::connected", "::register"),
    ] {
        assert!(admitted(&other).is_none(), "unlisted stack: {other}");
    }
}

#[test]
fn dependency_methods_ignore_type_arguments_but_keep_the_exact_method() {
    let entry = Exclusion {
        scopes: &["store"],
        functions: &["unityrs_batch::store::put::Request::run_observed"],
        platform: Platform::All,
        frame: Some("tokio::runtime::scheduler::defer::Defer>::defer"),
        via: &["tokio::time::timeout::Timeout::poll"],
        name: "deferred wake through a timeout poll",
    };
    for argument in [
        "u8",
        "alloc::vec::Vec<alloc::string::String>",
        "<unityrs_batch::store::attempt::Tries>::run_observed<object_store::list::PaginatedListResult, <unityrs_batch::store::list::Listing>::page::{closure#0}::{closure#1}, <unityrs_batch::store::list::Listing>::page::{closure#0}::{closure#1}::{closure#0}>::{closure#0}",
    ] {
        let trace = format!(
            "REFUSED_ALLOCATION site=1 size=64\n\
            0: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::alloc\n\
            1: <tokio::runtime::scheduler::defer::Defer>::defer\n\
            2: <tokio::time::timeout::Timeout<{argument}> as core::future::future::Future>::poll\n\
            3: <unityrs_batch::store::put::Request>::run_observed\n\
            END_REFUSED_ALLOCATION"
        );
        assert!(entry.admits(innermost(&trace).as_deref(), &trace));
        let other = trace.replace("::poll\n", "::new\n");
        assert!(!entry.admits(innermost(&other).as_deref(), &other));
    }
}

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
        inventory()
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
