//! Phase 6 steps 1–4: external unit/key decisions, then a single prefix pass.
//! The fixed handoff owns capabilities, never candidate or path inventories.
pub mod codec;
mod keys;
mod prefix;
mod select;
mod source;
pub mod stream;
mod units;
use crate::{
    phase::{Account, Error, check},
    sort::{self, Hooks, Options, Point, Source},
    spec::model::Spec,
    spool::workspace::{Sealed, Workspace},
};
pub use codec::{Row, Rule, State};
pub use source::Inputs;
use stream::{By, retire};

/// Spec-only indices, constructed in phase 1 inside `alloc::measured`.
/// Nothing is spilled: no spec-derived preference disk term is needed.
pub struct Prepared {
    preferences: Vec<u32>,
    archives: Vec<u32>,
    inputs: Vec<u32>,
}
impl Prepared {
    pub fn from_spec(spec: &Spec) -> Result<Self, Error> {
        fn indices(n: usize) -> Result<Vec<u32>, Error> {
            let mut out = Vec::new();
            out.try_reserve_exact(n).map_err(|_| Error::memory())?;
            for i in 0..n {
                out.push(u32::try_from(i).map_err(|_| Error::internal())?);
            }
            Ok(out)
        }
        let mut preferences = indices(spec.prefer_count())?;
        preferences.sort_unstable_by(|&a, &b| {
            spec.prefer_at(a)
                .map(|p| p.0)
                .cmp(&spec.prefer_at(b).map(|p| p.0))
        });
        let mut archives = indices(spec.archives.len())?;
        archives.sort_unstable_by(|&a, &b| {
            sort::slash_first(
                spec.archives
                    .get(usize::try_from(a).unwrap_or(usize::MAX))
                    .map_or(&[], |a| a.key.as_bytes()),
                spec.archives
                    .get(usize::try_from(b).unwrap_or(usize::MAX))
                    .map_or(&[], |a| a.key.as_bytes()),
            )
        });
        let mut inputs = indices(spec.input_count())?;
        inputs.sort_unstable_by_key(|&i| {
            spec.bundle(usize::try_from(i).unwrap_or(usize::MAX))
                .map(|b| b.id())
        });
        Ok(Self {
            preferences,
            archives,
            inputs,
        })
    }
}
#[derive(Debug)]
pub struct Artifacts {
    pub units: Option<Sealed>,
    pub keys: Option<Sealed>,
    pub winners: Option<Sealed>,
    pub publication_blocked: bool,
    pub counts: Counts,
}
/// Counts are keys, including flips later refused or withheld.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub keys: u64,
    pub collisions: u64,
    pub flips: u64,
    pub winners: u64,
}
impl Artifacts {
    /// Publication overlay; structural decisions and membership stay intact.
    #[must_use]
    pub fn publication_state(&self, state: State) -> State {
        if self.publication_blocked && state == State::Publish {
            State::Incomplete
        } else {
            state
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    Roots,
    Claims,
    Keepers,
    UnitConflicts,
    Reservations,
    Keys,
    Prefix,
    Propagation,
    Winners,
}
/// Bounded test seam; callbacks must not allocate inside a counted phase.
pub trait Observer {
    fn pass(&self, _pass: Pass, _claimed: u64) {}
    fn sort(&self, _retained: u64, _encoded: u64, _runs: u64) {}
}
pub struct NoObserver;
impl Observer for NoObserver {}

pub struct Context<'a> {
    pub spec: &'a Spec,
    pub prepared: &'a Prepared,
    pub inputs: &'a dyn Inputs,
    pub workspace: &'a mut Workspace,
    pub account: &'a Account,
    pub options: Options,
    pub stopped: &'a dyn Fn() -> bool,
    pub hooks: &'a dyn Hooks,
    pub observer: &'a dyn Observer,
}
impl Context<'_> {
    fn boundary(&self, pass: Pass) -> Result<(), Error> {
        check(self.stopped)?;
        self.observer.pass(pass, self.workspace.claimed());
        self.hooks.point(Point::Pass);
        check(self.stopped)
    }
    fn sort(&mut self, source: &mut dyn Source, by: By) -> Result<Option<Sealed>, Error> {
        let retained = self.workspace.claimed();
        let report = sort::sort(
            source,
            &by,
            self.workspace,
            self.account,
            self.options,
            self.stopped,
            self.hooks,
        )?;
        self.observer.sort(
            retained,
            report
                .file
                .as_ref()
                .map_or(0, |f| f.length().saturating_sub(16)),
            report.runs,
        );
        Ok(report.file)
    }
    fn reorder_unordered(
        &mut self,
        file: Option<Sealed>,
        from: By,
        to: By,
    ) -> Result<Option<Sealed>, Error> {
        let mut cursor = stream::Cursor::unordered(file.as_ref(), &from, self)?;
        let result = self.sort(&mut cursor, to)?;
        drop(cursor);
        retire(self.workspace, file, self.stopped)?;
        Ok(result)
    }
    fn reorder(&mut self, file: Option<Sealed>, from: By, to: By) -> Result<Option<Sealed>, Error> {
        let mut cursor = stream::Cursor::new(file.as_ref(), &from, self)?;
        let result = self.sort(&mut cursor, to)?;
        drop(cursor);
        retire(self.workspace, file, self.stopped)?;
        Ok(result)
    }
}
/// Audited steps 1–3 seam for X6. Prefix semantics are tested independently.
pub fn steps123(cx: &mut Context<'_>) -> Result<Artifacts, Error> {
    let result = cx.account.execute(|| structural(cx));
    notify(cx, result)
}
fn structural(cx: &mut Context<'_>) -> Result<Artifacts, Error> {
    let _overhead = cx.account.meter().charge(64 * 1024)?;
    let units = units::roots(cx)?;
    let (units, reserved) = units::reserve(cx, units)?;
    let (keys, counts) = keys::decide(cx, reserved)?;
    cx.boundary(Pass::Keys)?;
    Ok(Artifacts {
        units,
        keys,
        winners: None,
        publication_blocked: publication_blocked(cx)?,
        counts,
    })
}
fn publication_blocked(cx: &Context<'_>) -> Result<bool, Error> {
    if cx.spec.publish_incomplete {
        return Ok(false);
    }
    for i in 0..cx.spec.input_count() {
        check(cx.stopped)?;
        if cx
            .inputs
            .failed(u32::try_from(i).map_err(|_| Error::internal())?)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
/// One serial blocking task. On any error partial files keep their claims;
/// callers never advance to publication without this returning successfully.
pub fn run(cx: &mut Context<'_>) -> Result<Artifacts, Error> {
    let result = cx.account.execute(|| {
        let artifacts = structural(cx)?;
        let _overhead = cx.account.meter().charge(64 * 1024)?;
        let artifacts = prefix::finish(cx, artifacts)?;
        sort::Control {
            stopped: cx.stopped,
            hooks: cx.hooks,
        }
        .at(Point::BeforeReturn)?;
        Ok(artifacts)
    });
    notify(cx, result)
}
fn notify(cx: &Context<'_>, result: Result<Artifacts, Error>) -> Result<Artifacts, Error> {
    if let Err(Error::Run(failure)) = &result
        && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cx.hooks.request_stop(*failure);
        }))
        .is_err()
    {
        return Err(Error::internal());
    }
    result
}
