mod stage_support;
use serde_json::json;
use unityrs_batch::{
    alloc::{self, Counting, NoHook},
    bound, job,
    peek::word::Outcome,
    phase1::{self, InputPhase},
};

#[global_allocator]
static ALLOC: Counting<NoHook> = Counting(NoHook);

#[test]
fn retained_tables_words_and_walk_scratch_are_inside_phase1_measurement() {
    let v = stage_support::from_bundles(
        &[json!({"id":"z","path":"/z","deps":["a"]})],
        &[json!({"id":"a","path":"/a"})],
    );
    let (prepared, measure) = alloc::measured(|| phase1::prepare(v).unwrap());
    let mut p = prepared;
    assert_eq!(p.peek_words.len(), 2);
    assert_eq!(p.bundles.len(), 2);
    assert_eq!(p.inputs.len(), 1);
    assert_eq!(p.active.len(), 2);
    assert_eq!(p.uncommitted.get(0), Some(false));
    assert_eq!(p.uncommitted.get(1), None);
    assert_eq!(p.inputs[0].phase, InputPhase::Pending);
    assert_eq!(p.peek_words[0].load(), Ok(Outcome::Pending));
    assert!(
        p.bundles
            .iter()
            .all(|b| b.stage.is_none() && b.peek.is_none())
    );
    assert!(p.active.iter().all(|s| s.job.is_none()
        && s.dispatched.is_none()
        && s.directory.is_none()
        && s.peek_file.is_none()));
    assert_eq!(p.stages_active.len(), p.spec.download_concurrency as usize);
    assert!(
        p.stages_active
            .iter()
            .all(|s| s.bundle.is_none() && s.file.is_none())
    );
    assert!(size_of::<phase1::BundleState>() <= 256);
    assert!(p.peek_queue.capacity() >= 2 && p.peek_queue.is_empty());
    assert!(p.ready.capacity() >= 1 && p.ready.is_empty());
    assert!(p.failures.capacity() >= 1 && p.failures.is_empty());
    let minimum = (2 * size_of::<phase1::BundleState>()
        + 2 * size_of::<phase1::ActiveSlot>()
        + 2 * size_of::<unityrs_batch::peek::word::Word>()) as u64;
    assert!(measure.kept >= minimum, "{measure:?}, minimum={minimum}");
    assert!(measure.peak >= measure.kept);
    let (len, walk) = alloc::measured(|| p.walker.view(&p.graph, 0, 2).unwrap().len());
    assert_eq!(len, 2);
    assert_eq!(walk.kept, 0, "first walk grew retained scratch");
    p.peek_words[0].publish(Outcome::Clear(3)).unwrap();
    assert!(
        p.bundles[0].peek.is_none(),
        "publication must not invent a received end"
    );
    assert_eq!(p.uncommitted.get(0), Some(false));
    let b = bound::memory_bound(&p.spec, measure, 0, &bound::PROVISIONAL);
    assert_eq!(b.c_spec, measure.kept + 2 * bound::PER_BUNDLE);
}

#[test]
fn bit_boundaries_do_not_grow_or_alias_and_end_slots_cover_the_actual_union() {
    let mut bits = phase1::Bits::new(65);
    for index in [0, 63, 64] {
        assert_eq!(bits.set(index, true), Some(false));
        assert_eq!(bits.set(index, true), Some(true));
        assert_eq!(bits.get(index), Some(true));
        assert_eq!(bits.set(index, false), Some(true));
    }
    assert_eq!(bits.set(65, true), None);
    assert_eq!(bits.get(64), Some(false));
    assert!(bound::PROVISIONAL.m_end >= (size_of::<job::End>() + bound::END_SLOT_OVERHEAD) as u64);
    assert!(size_of::<job::End>() >= size_of::<unityrs_batch::attempt::AttemptEnd>());
    assert!(size_of::<job::End>() >= size_of::<unityrs_batch::peek::PeekEnd>());
}
