//! Post-drain verification (spec 7.2, 7.3; batch 2.9). A successful scan
//! may find mismatches: verified is true only when every published key matches.
use super::{Context, Expected, Source, Target, list_error, staged::Staged};
use crate::{
    outcomes::{Archive, Code, Outcomes},
    phase::{self, Error},
    store::{etag, list::Listing},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub verified: Option<bool>,
    pub verified_by_size: u64,
    pub mismatches: u64,
    pub verify_list_failed: bool,
}
/// `published` counts written/present keys and archives. The caller provides
/// the drained in-flight count; verification cannot run alongside a transfer.
pub async fn run(
    cx: &Context<'_, '_>,
    source: &mut dyn Source,
    outcomes: &Outcomes,
    published: u64,
    in_flight: usize,
) -> Result<Report, Error> {
    let mut report = Report::default();
    match run_inner(cx, source, outcomes, published, in_flight, &mut report).await {
        Err(Error::Stopped) => {
            report.verified = Some(false);
            Ok(report)
        }
        other => other,
    }
}
async fn run_inner(
    cx: &Context<'_, '_>,
    source: &mut dyn Source,
    outcomes: &Outcomes,
    published: u64,
    in_flight: usize,
    report: &mut Report,
) -> Result<Report, Error> {
    if in_flight != 0 {
        return Err(Error::internal());
    }
    if published == 0 {
        return Ok(Report::default());
    }
    *report = Report {
        verified: Some(false),
        ..Report::default()
    };
    if (cx.stopped)() || cx.request.tries.control.stopped() {
        return Ok(*report);
    }
    if !cx.account.admits(
        crate::store::list::RESERVATION
            .checked_add(cx.workspace.adapter_charge()?)
            .ok_or_else(Error::internal)?,
    ) {
        report.verify_list_failed = true;
        return Ok(*report);
    }
    let listing =
        Listing::new(cx.prefix).map_err(|e| list_error(e).err().unwrap_or_else(Error::internal))?;
    let mut staged = Staged::new(cx)?;
    let mut by_size = 0;
    loop {
        if (cx.stopped)() {
            return Ok(*report);
        }
        let page = match listing.page(cx.request, cx.account).await {
            Ok(Some(page)) => page,
            Ok(None) => break,
            Err(e) => {
                let stopped = list_error(e)?;
                report.verify_list_failed = !stopped;
                if !stopped {
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
                if row.code.published() {
                    if matches(&row, listed) {
                        by_size += u64::from(row.code == Code::WrittenBySize);
                    } else {
                        staged.push(cx, row.target, None)?;
                    }
                }
                source.advance()?;
            }
        }
    }
    while let Some(row) = source.current()? {
        phase::check(cx.stopped)?;
        if row.code.published() {
            staged.push(cx, row.target, None)?;
        }
        source.advance()?;
    }
    if (cx.stopped)() {
        return Ok(*report);
    }
    report.verified_by_size = by_size;
    staged.apply(cx, |target, _| {
        match target {
            Target::Loose(i) => outcomes.set(i, Code::PublishMismatch)?,
            Target::Archive(i) => {
                outcomes.set_archive(i, &Archive::failed(Code::PublishMismatch)?)?;
            }
        }
        report.mismatches += 1;
        Ok(())
    })?;
    report.verified = Some(report.mismatches == 0);
    Ok(*report)
}
fn matches(row: &Expected<'_>, listed: &object_store::ObjectMeta) -> bool {
    if row.key != listed.location.as_ref() {
        return false;
    }
    let same_tag = listed
        .e_tag
        .as_deref()
        .is_some_and(|s| etag::matches(s.as_bytes(), row.etag));
    if row.code == Code::WrittenBySize {
        row.size == listed.size && (row.etag.is_empty() || same_tag)
    } else {
        same_tag
    }
}
