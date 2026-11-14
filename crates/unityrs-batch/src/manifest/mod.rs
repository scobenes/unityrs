//! Bounded manifest recognition and assembly. Normal head publication and
//! finishing order are owned by the orchestration layer.
pub mod chunks;
pub mod counts;
pub mod figures;
pub mod head;
mod received;
pub mod rows;
use crate::phase::Error;
use counts::Counts;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    Publish,
    Phase(Error),
}
impl From<Error> for WriteError {
    fn from(e: Error) -> Self {
        Self::Phase(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Family {
    Bundles,
    Inputs,
    Skips,
    Diagnostics,
    Outputs,
    Collisions,
    Archives,
    ArchiveMembers,
}
impl Family {
    pub const ALL: [Self; 8] = [
        Self::Bundles,
        Self::Inputs,
        Self::Skips,
        Self::Diagnostics,
        Self::Outputs,
        Self::Collisions,
        Self::Archives,
        Self::ArchiveMembers,
    ];
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bundles => "bundles",
            Self::Inputs => "inputs",
            Self::Skips => "skips",
            Self::Diagnostics => "diagnostics",
            Self::Outputs => "outputs",
            Self::Collisions => "collisions",
            Self::Archives => "archives",
            Self::ArchiveMembers => "archive_members",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub rows: [Option<u64>; 8],
    pub counts: Counts,
    pub run: figures::Figures,
    pub failure: Option<crate::job::RunFailure>,
    pub publication_failed: bool,
    pub filesystem_sync_failed: bool,
}
impl Report {
    /// A final sync failure invalidates every published row-derived fact.
    /// Received run figures are stored separately and remain available.
    pub fn sync_failed(&mut self) {
        self.filesystem_sync_failed = true;
        self.rows = [None; 8];
        self.counts = Counts::default();
        crate::job::RunFailure::reduce(
            &mut self.failure,
            crate::job::RunFailure::new(crate::attempt::ReasonCode::HostIo),
        );
    }
}
/// The caller supplies the finisher's stop/try-budget query. The canonical
/// phase-7 stop has already been frozen: B9 can supply its own finish snapshot.
pub fn assemble(
    cx: &rows::Context<'_>,
    destination: &mut dyn chunks::Destination,
) -> Result<Report, Error> {
    if !cx.account.admits(crate::publish::WORKING_SET) {
        return Err(Error::internal());
    }
    let mut report = Report::default();
    cx.account
        .publication(|| assemble_into(cx, destination, &mut report))?;
    Ok(report)
}
/// Normal finishing retains acknowledged families even if a later parser or
/// allocation fails. Scratch drops before the audit; the head can still name
/// earlier complete families and the exact run failure.
pub(crate) fn finishing(
    cx: &rows::Context<'_>,
    destination: &mut dyn chunks::Destination,
) -> Report {
    let mut report = Report {
        run: figures::received(cx.prepared).unwrap_or_default(),
        ..Report::default()
    };
    if let Err(error) = cx
        .account
        .publication(|| assemble_into(cx, destination, &mut report))
    {
        let failure = match error {
            Error::Run(f) => f,
            Error::Stopped => crate::job::RunFailure::new(crate::attempt::ReasonCode::Internal),
        };
        crate::job::RunFailure::reduce(&mut report.failure, failure);
    }
    report
}
fn assemble_into(
    cx: &rows::Context<'_>,
    destination: &mut dyn chunks::Destination,
    report: &mut Report,
) -> Result<(), Error> {
    let _overhead = cx.account.meter().charge(crate::publish::OVERHEAD)?;
    report.run = figures::complete(cx)?;
    for family in Family::ALL {
        let mut counts = Counts::default();
        let result = (|| {
            let mut chunks = chunks::Chunks::new(destination, family, cx.account)?;
            rows::generate(family, cx, &mut chunks, &mut counts)?;
            chunks.finish()
        })();
        match result {
            Ok(n) => {
                *report
                    .rows
                    .get_mut(family as usize)
                    .ok_or_else(Error::internal)? = Some(n);
                report.counts.add(counts)?;
            }
            Err(WriteError::Publish) => {
                report.publication_failed = true;
            }
            Err(WriteError::Phase(e)) => return Err(e),
        }
    }
    Ok(())
}

fn directory_error(e: crate::sink::Error) -> WriteError {
    match e.class {
        crate::sink::Class::HostMemory => WriteError::Phase(Error::memory()),
        crate::sink::Class::InvalidPath => WriteError::Phase(Error::internal()),
        _ => WriteError::Publish,
    }
}
