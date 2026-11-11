//! `unityrs-meshdecode`'s `MeshHandler::process` and `get_triangles` on a
//! Mesh's or a Sprite's value, as `unityrs_fuzz::meshdecode` describes. The
//! input is `unityrs_testkit::fuzz::MeshCase`: the flags, the meter, the
//! version, the resource, then the value's tree and bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| unityrs_fuzz::meshdecode::check(data));
