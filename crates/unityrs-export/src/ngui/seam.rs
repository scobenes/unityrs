//! The engine seam (`docs/plans/batch.md` 2.6; review round 1's G1): the
//! cutter is one phase of an attempt the engine owns. The engine registers
//! the view ([`super::register`]), makes `M_X`, the key table and the sink,
//! and hands them over in a [`Context`]; the cutter selects, plans and
//! exports inside it, and returns what it recorded ([`super::Cut`]).
//!
//! The context's checks, each at the sites 2.6 names:
//! - **the stop** ([`Hooks::stopped`]) between objects, sheet groups and
//!   outputs (step 11): a stopped attempt returns [`Failure::Stopped`] at
//!   once;
//! - **evictions** after each Environment call (step 7): past
//!   `max_attempt_evictions` X fails, `limit(attempt_evictions, <value>)`,
//!   after the call's run-level class and before any other;
//! - **export work** (step 10) at each pixel buffer's charge, before the
//!   decode that fills it ([`Hooks::export_work`]); the exception is a
//!   Sprite's render, whose intermediates are bounded by `M_X`'s charges
//!   and its decode work, and whose image is counted when the render
//!   returns it, after answer 11 and before its PNG; a file's increment is
//!   the sink's, at its claim;
//! - **the refusal probe** (2.7) around each core call on its own: a
//!   `budget(...)` refusal during which the attempt budget's refusal count
//!   did not move was no budget's, so it is `host_memory`, a stop.
//!
//! Every output is handed to the sink as it is made, and the cutter keeps
//! nothing of it, so each buffer is the sink's to drop before the next is
//! made (2.8); a sheet's decode is dropped after its group (step 6).

use std::sync::Arc;

use unityrs_core::RawStr;
use unityrs_core::env::Environment;
use unityrs_core::files::BundleFile;
use unityrs_core::limits::{Charge, Meter};

use super::classify::{Class, Reason};
use super::keys::{KeyTable, Template, TemplateKind};
use super::{Failure, Output};

/// What the engine's attempt answers for the cutter. Each method is called
/// with no core lock held; an engine answers from its atomics.
pub trait Hooks {
    /// Whether the run has stopped (2.11).
    fn stopped(&self) -> bool {
        false
    }

    /// The attempt budget's refusal count (2.7), or none when the caller
    /// counts none, which turns the probe off.
    fn refusals(&self) -> Option<u64> {
        None
    }

    /// Cumulative allocator observations for this attempt, separate from budget
    /// refusals. Once nonzero, no further core call or publication is allowed,
    /// even when optional work returned success after absorbing the failure.
    fn allocator_refusals(&self) -> u64 {
        0
    }

    /// Counts `bytes` of export work (2.6 step 10): past
    /// `max_export_work`, X's failure, `limit(export_work, <value>)`.
    fn export_work(&self, bytes: u64) -> Result<(), Reason> {
        let _ = bytes;
        Ok(())
    }

    /// Test-only substitution, invoked inside texture panic containment.
    #[cfg(feature = "test-hooks")]
    fn decoder_fault(&self) {}

    /// Test-only substitution of a registration Environment call's result,
    /// after the real call has updated its eviction counters.
    #[cfg(feature = "test-hooks")]
    fn environment_fault(&self, _env: &Environment) -> Option<unityrs_core::Error> {
        None
    }

    /// Meter sampled while the attempt holds a file, before its handle drops.
    fn meter(&self, _meter: &Meter) {}

    /// Environment figures after a call, including failed calls.
    fn environment(&self, _env: &Environment) {}

    /// A sprite group's export begins (4.4): told once for each of its
    /// Sprites, in plan order, by the Sprite's file's name and its path id,
    /// the group numbered `group` in export order. What the plan grouped,
    /// for an observer (a test, B3's review round 3, T1); nothing is asked.
    /// The name is the SerializedFile's, as an output's and a skip's are, so
    /// two files of one CAB name (one nested) tell an observer the same
    /// name: one that must tell such Sprites apart cannot from this alone
    /// (review round 4, R8), as the records cannot.
    fn sprite_group(&self, group: usize, file: &RawStr, path_id: i64) {
        let _ = (group, file, path_id);
    }
}

/// Hooks that never stop, count no refusal and bound no work: a tool's.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHooks;

impl Hooks for NoHooks {}

/// An archive's `members` entry (1.6, 4.7): its archive's index, its kind,
/// and its path's template.
#[derive(Clone, Debug)]
pub struct MemberTemplate {
    /// The archive's index in the spec.
    pub archive: usize,
    /// The kind of output it takes.
    pub kind: TemplateKind,
    /// Its path.
    pub path: Template,
}

/// The view the engine registered ([`super::register`]): the private
/// Environment, and X's own handle on `M_X`.
pub struct Registered {
    /// The private Environment, `view(X)` registered, X last.
    pub env: Environment,
    /// X's own open (2.6 step 2), on `M_X`: selection reads through it.
    pub own: Arc<BundleFile>,
    /// The charge of X's bytes to `M_X` when they were in hand
    /// (`Data::Bytes`; a staged file's read carries its own).
    pub(super) _held: Option<Charge>,
}

/// The engine's attempt, as the cutter sees it.
pub struct Context<'a> {
    /// The registered view.
    pub registered: &'a Registered,
    /// `M_X`.
    pub meter: &'a Arc<Meter>,
    /// X's spec id (`{input}`).
    pub input: &'a str,
    /// The stop flag, the refusal count and export work.
    pub hooks: &'a dyn Hooks,
    /// `max_attempt_evictions` (1.1).
    pub max_evictions: u64,
    /// The sink's key prefix (2.3), checked after the key.
    pub prefix: &'a str,
    /// Every archive's `members` entries.
    pub members: &'a [MemberTemplate],
    /// The attempt's one key table, for every kind's stage.
    pub keys: &'a mut KeyTable,
    /// Where each output goes as it is made (2.8's copy-out).
    pub sink: &'a mut dyn FnMut(Output) -> Result<(), Failure>,
}

/// The context's shared part: what every site reads.
#[derive(Clone, Copy)]
pub(super) struct Seam<'a> {
    pub registered: &'a Registered,
    pub meter: &'a Arc<Meter>,
    pub hooks: &'a dyn Hooks,
    pub max_evictions: u64,
    pub prefix: &'a str,
    pub members: &'a [MemberTemplate],
}

impl<'a> Seam<'a> {
    pub fn env(&self) -> &'a Environment {
        &self.registered.env
    }

    /// The stop check: [`Failure::Stopped`] once the run stopped.
    pub fn stop(&self) -> Result<(), Failure> {
        allocator_check(self.hooks).map_err(Failure::from)?;
        if self.hooks.stopped() {
            Err(Failure::Stopped)
        } else {
            Ok(())
        }
    }

    /// The eviction check, after an Environment call (2.6 step 7).
    pub fn evictions(&self) -> Result<(), Class> {
        evictions(self.env(), self.max_evictions)
    }

    /// `f`, one core call (or one charge), under the refusal probe (2.7).
    pub fn probe<T>(&self, f: impl FnOnce() -> Result<T, Class>) -> Result<T, Class> {
        probe(self.hooks, f)
    }

    /// An Environment call: the probe, then 2.6 step 7's order: a
    /// run-level class first, then the evictions, then the rest.
    pub fn env_call<T>(&self, f: impl FnOnce() -> Result<T, Class>) -> Result<T, Class> {
        let out = self.probe(f);
        self.hooks.environment(self.env());
        if let Err(Class::Run(_)) = &out {
            return out;
        }
        self.evictions()?;
        out
    }

    /// Export work for a buffer of `bytes` pixel bytes, before it is made.
    pub fn work(&self, bytes: u64) -> Result<(), Class> {
        self.hooks.export_work(bytes).map_err(Class::Input)
    }
}

/// [`Seam::evictions`] on `env`: registration's too.
pub(super) fn evictions(env: &Environment, max: u64) -> Result<(), Class> {
    if env.stats().dependencies_evicted > max {
        let reason = format!("limit(attempt_evictions, {max})");
        return Err(Class::Input(Reason::plain(reason)));
    }
    Ok(())
}

/// [`Seam::probe`] on `hooks`.
pub(super) fn probe<T>(
    hooks: &dyn Hooks,
    f: impl FnOnce() -> Result<T, Class>,
) -> Result<T, Class> {
    allocator_check(hooks)?;
    let before = hooks.refusals();
    let out = f();
    allocator_check(hooks)?;
    out.map_err(|c| probed(hooks, before, c))
}

/// A sticky allocator audit also rejects successful calls that absorbed an
/// optional failure. Check before running another call as well as afterwards.
pub(super) fn allocator_check(hooks: &dyn Hooks) -> Result<(), Class> {
    if hooks.allocator_refusals() != 0 {
        Err(Class::Run(Reason::plain("host_memory")))
    } else {
        Ok(())
    }
}

/// The probe's rule for one core call's class `c`, the refusal count
/// having been `before` when the call began: a `budget(...)` refusal
/// during which the count did not move is `host_memory`.
pub(super) fn probed(hooks: &dyn Hooks, before: Option<u64>, c: Class) -> Class {
    if let Err(c) = allocator_check(hooks) {
        return c;
    }
    if let Some(before) = before
        && is_refusal(&c)
        && hooks.refusals() == Some(before)
    {
        return Class::Run(Reason::plain("host_memory"));
    }
    c
}

/// Whether `c` is a budget's refusal (`budget(input)`, `budget(dependency)`).
fn is_refusal(c: &Class) -> bool {
    let (Class::Skip(r) | Class::Input(r) | Class::Run(r)) = c;
    r.reason.starts_with("budget(")
}

#[cfg(test)]
mod allocator_tests {
    use super::*;
    use std::cell::Cell;
    #[derive(Default)]
    struct Audit {
        allocator: Cell<u64>,
        budget: Cell<u64>,
    }
    impl Hooks for Audit {
        fn refusals(&self) -> Option<u64> {
            Some(self.budget.get())
        }
        fn allocator_refusals(&self) -> u64 {
            self.allocator.get()
        }
    }
    #[test]
    fn absorbed_allocator_refusal_wins_even_after_a_budget_refusal() {
        let hooks = Audit::default();
        let result = probe(&hooks, || {
            hooks.budget.set(1);
            hooks.allocator.set(1);
            Ok(())
        });
        assert!(matches!(result, Err(Class::Run(r)) if r.reason == "host_memory"));
        let mut called = false;
        let _ = probe(&hooks, || {
            called = true;
            Ok(())
        });
        assert!(!called, "a poisoned attempt must make no further call");
    }
    #[test]
    fn an_absorbed_budget_refusal_alone_can_still_succeed() {
        let hooks = Audit::default();
        assert!(
            probe(&hooks, || {
                hooks.budget.set(1);
                Ok(())
            })
            .is_ok()
        );
    }
}
