//! Resume's sorted merge (batch 2.9; spec 1.6, 7.2). Publish no partial
//! matches: a failed/malformed LIST means PUT everything, with one warning.
use super::{Context, Listed, Source, Target, list_error, staged::Staged};
use crate::{
    outcomes::{Code, Outcomes},
    phase::{self, Error},
    store::{etag::Etag, list::Listing},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub present: u64,
    pub resume_list_failed: bool,
    pub stopped: bool,
}
/// Serial library drive. The scheduler can use Listing separately while its
/// transfers run; this drive requires enough phase room for its page envelope.
pub async fn run(
    cx: &Context<'_, '_>,
    source: &mut dyn Source,
    outcomes: &Outcomes,
    archives: &mut [Option<Listed>],
) -> Result<Report, Error> {
    let mut report = Report::default();
    match run_inner(cx, source, outcomes, archives, &mut report).await {
        Err(Error::Stopped) => {
            report.stopped = true;
            Ok(report)
        }
        other => other,
    }
}
async fn run_inner(
    cx: &Context<'_, '_>,
    source: &mut dyn Source,
    outcomes: &Outcomes,
    archives: &mut [Option<Listed>],
    report: &mut Report,
) -> Result<Report, Error> {
    if archives.len() > 256 {
        return Err(Error::internal());
    }
    archives.fill(None);
    if (cx.stopped)() || cx.request.tries.control.stopped() {
        return Ok(Report {
            stopped: true,
            ..Report::default()
        });
    }
    if !cx.account.admits(
        crate::store::list::RESERVATION
            .checked_add(cx.workspace.adapter_charge()?)
            .ok_or_else(Error::internal)?,
    ) {
        return Ok(Report {
            resume_list_failed: true,
            ..Report::default()
        });
    }
    let listing =
        Listing::new(cx.prefix).map_err(|e| list_error(e).err().unwrap_or_else(Error::internal))?;
    let mut staged = Staged::new(cx)?;
    loop {
        if (cx.stopped)() {
            report.stopped = true;
            return Ok(*report);
        }
        let page = match listing.page(cx.request, cx.account).await {
            Ok(Some(page)) => page,
            Ok(None) => break,
            Err(e) => {
                report.stopped = list_error(e)?;
                report.resume_list_failed = !report.stopped;
                if !report.stopped {
                    staged.discard(cx)?;
                }
                return Ok(*report);
            }
        };
        for listed in page.entries() {
            phase::check(cx.stopped)?;
            while let Some(row) = source.current()? {
                if row.key > listed.location.as_ref() {
                    break;
                }
                if row.key == listed.location.as_ref() {
                    let etag = listed
                        .e_tag
                        .as_deref()
                        .and_then(|s| Etag::new(s.as_bytes()))
                        .ok_or_else(Error::internal)?;
                    match row.target {
                        Target::Loose(_) if etag.matches(row.etag) => {
                            staged.push(cx, row.target, None)?;
                        }
                        Target::Archive(i) => {
                            if usize::try_from(i).ok().is_none_or(|i| i >= archives.len()) {
                                return Err(Error::internal());
                            }
                            staged.push(
                                cx,
                                row.target,
                                Some(Listed {
                                    etag,
                                    size: listed.size,
                                }),
                            )?;
                        }
                        Target::Loose(_) => {}
                    }
                }
                source.advance()?;
            }
        }
    }
    if (cx.stopped)() {
        report.stopped = true;
        return Ok(*report);
    }
    staged.apply(cx, |target, listed| {
        match target {
            Target::Loose(i) => {
                outcomes.set(i, Code::Present)?;
                report.present += 1;
            }
            Target::Archive(i) => {
                *archives
                    .get_mut(usize::try_from(i).map_err(|_| Error::internal())?)
                    .ok_or_else(Error::internal)? = Some(listed);
            }
        }
        Ok(())
    })?;
    Ok(*report)
}
