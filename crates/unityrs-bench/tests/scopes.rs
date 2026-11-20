//! Every scope entry counts process-wide (`work::scopes_entered`), so a
//! call of another scope during setup or the digest, where callgrind's
//! collection is off and the profile cannot show it, is seen: `unityrs-bench
//! run` fails a run whose count is not 1. Its own process, as the count is
//! the process's.

use unityrs_bench::work::{self, Op};
use unityrs_bench::{Scale, build};

#[test]
fn every_scope_entry_counts_including_one_outside_the_measured_scope() {
    let fixture = build("textures-DXT1.bundle", Scale::Gate).unwrap().unwrap();
    let input = work::setup(Op::Texture, fixture.bytes.into()).unwrap();
    assert_eq!(work::scopes_entered(), 0, "setup enters no scope");
    // A setup-time call of another scope, as a broken setup might make.
    let _ = work::run_texture_bc7(&input);
    let out = work::run_texture_dxt1(&input).unwrap();
    work::digest(&out).unwrap();
    assert_eq!(work::scopes_entered(), 2);
}
