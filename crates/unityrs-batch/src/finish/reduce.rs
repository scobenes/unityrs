use crate::{
    manifest::rows::{Context, Selected},
    outcomes,
    phase::Error,
    reason::Code,
    stage::{DataFailure, Failure},
    winners::rowread::{File, Reader, Row},
};

pub(super) fn normal(cx: &Context<'_>) -> Result<Code, Error> {
    let sel = cx.selected.ok_or_else(Error::internal)?;
    let mut code = Code::Complete;
    for bundle in &cx.facts.bundles {
        if let Some(Failure::Data(failure)) = bundle.stage.and_then(|s| s.failure) {
            code = code.worst(if matches!(failure, DataFailure::InputBytes(_)) {
                Code::Budget
            } else {
                Code::Incomplete
            });
        }
    }
    for state in &cx.facts.inputs {
        if let Some(received) = state.received {
            code = code.worst(received.code);
        }
        if let Some(bundle) = state.failure_bundle {
            let facts = cx
                .facts
                .bundles
                .get(usize::try_from(bundle).map_err(|_| Error::internal())?)
                .ok_or_else(Error::internal)?;
            let budget = facts.peek.and_then(|p| p.outcome).is_some_and(|p| matches!(p, crate::peek::word::Outcome::Failed(crate::peek::word::FailureCode::Limit(k)) if k.is_budget()));
            code = code.worst(if budget {
                Code::Budget
            } else {
                Code::Incomplete
            });
        }
    }
    let mut outputs = Reader::open(
        sel.workspace,
        sel.handoff.outputs.as_ref(),
        File::Outputs,
        cx.account.meter(),
        cx.stopped,
    )?;
    while let Some(row) = outputs.next(cx.stopped)? {
        let Row::Output(row) = row else {
            return Err(Error::internal());
        };
        if row.status == "unwritable_key"
            || row
                .reason
                .is_some_and(|r| !matches!(r, "unit_lost" | "publish_incomplete"))
        {
            code = code.worst(Code::CompleteWithSkips);
        }
    }
    if !sel.handoff.publication_blocked {
        for index in 0..sel.handoff.counts.winners {
            crate::phase::check(cx.stopped)?;
            if !matches!(
                sel.outcomes.loose(index)?,
                outcomes::Code::Written | outcomes::Code::Present
            ) {
                code = code.worst(Code::Incomplete);
            }
        }
    }
    // Read each family separately so its charged buffers are released before
    // the next stream; archive outcomes use declared indices, not key order.
    drop(outputs);
    Ok(code.worst(archives(cx, sel)?).worst(members(cx, sel)?))
}

fn archives(cx: &Context<'_>, sel: Selected<'_>) -> Result<Code, Error> {
    let mut rows = Reader::open(
        sel.workspace,
        sel.handoff.archives.as_ref(),
        File::Archives,
        cx.account.meter(),
        cx.stopped,
    )?;
    let mut metadata = sel
        .handoff
        .metadata
        .as_ref()
        .map(|m| {
            crate::winners::archives::Reader::open(sel.workspace, m, cx.account.meter(), cx.stopped)
        })
        .transpose()?;
    let mut code = Code::Complete;
    while let Some(row) = rows.next(cx.stopped)? {
        let Row::Archive(row) = row else {
            return Err(Error::internal());
        };
        let archive = metadata
            .as_mut()
            .ok_or_else(Error::internal)?
            .next(cx.stopped)?
            .ok_or_else(Error::internal)?;
        if (archive.key, archive.members, archive.left_out) != (row.key, row.members, row.left_out)
        {
            return Err(Error::internal());
        }
        if row.status == "not_written"
            && !matches!(
                sel.outcomes.archive(u64::from(archive.index))?.code,
                outcomes::Code::Written | outcomes::Code::Present
            )
        {
            code = code.worst(Code::Incomplete);
        }
    }
    if metadata
        .as_mut()
        .map(|m| m.next(cx.stopped))
        .transpose()?
        .flatten()
        .is_some()
    {
        return Err(Error::internal());
    }
    Ok(code)
}

fn members(cx: &Context<'_>, sel: Selected<'_>) -> Result<Code, Error> {
    let mut rows = Reader::open(
        sel.workspace,
        sel.handoff.archive_members.as_ref(),
        File::Members,
        cx.account.meter(),
        cx.stopped,
    )?;
    let mut code = Code::Complete;
    while let Some(row) = rows.next(cx.stopped)? {
        let Row::Member(row) = row else {
            return Err(Error::internal());
        };
        if !row.included {
            code = code.worst(Code::CompleteWithSkips);
        }
    }
    Ok(code)
}
