//! Checked adapters over export's single recursive extraction classifier.
use super::{AttemptEnd, AttemptView, DetailCode, EntryDetail, ReasonCode, State};
use crate::{
    reason::{Code, Limit, LimitKind},
    records, spool,
};
use unityrs_export::ngui::{Failure, Reason};

fn limit(s: &str) -> Option<Limit> {
    let (kind, value) = s
        .strip_prefix("limit(")?
        .strip_suffix(')')?
        .split_once(", ")?;
    let kind = [
        LimitKind::InputMemory,
        LimitKind::InputBytes,
        LimitKind::DecodeWork,
        LimitKind::AttemptEvictions,
        LimitKind::ExportWork,
        LimitKind::BlockOutput,
        LimitKind::Inflate,
        LimitKind::ContainerDepth,
        LimitKind::TreeDepth,
        LimitKind::ValueDepth,
        LimitKind::ValuesPerObject,
        LimitKind::ZeroSizeElements,
        LimitKind::Count,
        LimitKind::TexturePixels,
    ]
    .into_iter()
    .find(|k| k.name() == kind)?;
    Some(Limit {
        kind,
        value: value.parse().ok()?,
    })
}
fn input(s: &str) -> Option<ReasonCode> {
    Some(match s {
        "not_a_bundle" => ReasonCode::NotABundle,
        "container_error" => ReasonCode::ContainerError,
        "entry_error" => ReasonCode::EntryError,
        "budget(input)" => ReasonCode::BudgetInput,
        "budget(dependency)" => ReasonCode::BudgetDependency,
        _ => {
            let l = limit(s)?;
            crate::reason::InputLimit::new(l)?;
            ReasonCode::Limit(l)
        }
    })
}
pub(super) fn failure(end: &mut AttemptEnd, failure: &Failure, view: AttemptView<'_>) {
    let (run, r) = match failure {
        Failure::Stopped => {
            end.state = State::Stopped;
            end.code = Code::Stopped;
            return;
        }
        Failure::Run(r) => (true, r),
        Failure::Input(r) => (false, r),
    };
    if run {
        end.run_failure(match r.reason.as_str() {
            "host_memory" => ReasonCode::HostMemory,
            "host_io" => ReasonCode::HostIo,
            "spool_full" => ReasonCode::SpoolFull,
            _ => ReasonCode::Internal,
        });
        end.detail = r.detail.as_deref().and_then(DetailCode::parse);
        return;
    }
    let mut name = r.reason.as_str();
    if let Some(inner) = name
        .strip_prefix("dependency_failed(")
        .and_then(|s| s.strip_suffix(')'))
    {
        // IDs are arbitrary non-control UTF-8, including comma and space.
        // Match an exact view identity with a checked inner reason; splitting
        // the wrapper at punctuation would corrupt valid IDs.
        let Some((i, reason)) = view.members.iter().enumerate().find_map(|(i, member)| {
            let reason = inner.strip_prefix(member.id)?.strip_prefix(", ")?;
            input(reason)?;
            Some((u32::try_from(i).ok()?, reason))
        }) else {
            end.run_failure(ReasonCode::Internal);
            return;
        };
        end.dependency = Some(i);
        name = reason;
    }
    let Some(reason) = input(name) else {
        end.run_failure(ReasonCode::Internal);
        return;
    };
    end.state = State::Failed;
    end.reason = Some(reason);
    end.cut = r.cut;
    end.code = match reason {
        ReasonCode::BudgetInput | ReasonCode::BudgetDependency => Code::Budget,
        ReasonCode::Limit(l) if l.kind.is_budget() => Code::Budget,
        _ => Code::Incomplete,
    };
    if matches!(reason, ReasonCode::ContainerError | ReasonCode::EntryError) {
        let Some(detail) = r.detail.as_deref() else {
            end.run_failure(ReasonCode::Internal);
            return;
        };
        let code = if reason == ReasonCode::EntryError {
            detail.split_once(' ').map(|p| p.0)
        } else {
            Some(detail)
        };
        let Some(code) = code.and_then(DetailCode::parse) else {
            end.run_failure(ReasonCode::Internal);
            return;
        };
        end.detail = Some(code);
        if reason == ReasonCode::EntryError {
            let Some(text) = EntryDetail::prepared(detail) else {
                end.run_failure(ReasonCode::Internal);
                return;
            };
            end.entry_detail = text;
        }
    } else if r.detail.is_some() {
        end.run_failure(ReasonCode::Internal);
    }
}
pub(super) fn record(e: records::Error) -> Failure {
    match e {
        records::Error::Charge(e) => unityrs_export::ngui::classify::charge(&e).into(),
        records::Error::Work(l) => Failure::Input(Reason::plain(l.to_string())),
        records::Error::Spool(e) => match e {
            spool::Error::Stopped => Failure::Stopped,
            spool::Error::Full(f) => Failure::Run(Reason {
                reason: "spool_full".into(),
                detail: Some(f.detail().into()),
                cut: false,
            }),
            spool::Error::HostIo(_) => Failure::Run(Reason::plain("host_io")),
            spool::Error::Internal => internal(),
        },
        records::Error::Internal => internal(),
    }
}
pub(super) fn internal() -> Failure {
    Failure::Run(Reason::plain("internal"))
}
/// Validate every skip vocabulary before recording it; unknown future mappings
/// stop internal. This matches strings only, never recursively matches errors.
pub(super) fn skip_code(s: &str) -> Option<Code> {
    Some(match s {
        "empty_image"
        | "empty_rect"
        | "duplicate_name"
        | "reference_atlas"
        | "ngui_layout_unsupported"
        | "unsupported_platform(switch)"
        | "unsupported_platform(xbox360)" => Code::Complete,
        "decode_error" | "decoder_panic" | "bad_pointer" | "duplicate_key" | "unnamed_atlas"
        | "sheet_not_found" | "no_typetree" | "json_unwritable" => Code::CompleteWithSkips,
        "budget(input)" => Code::Budget,
        _ if s.starts_with("outside_view(") && s.ends_with(')') => Code::CompleteWithSkips,
        _ if s.starts_with("unwritable_key(") && s.ends_with(')') => {
            let arg = s.strip_prefix("unwritable_key(")?.strip_suffix(')')?;
            if !crate::reason::KeyRule::ALL
                .into_iter()
                .any(|r| r.name() == arg)
            {
                return None;
            }
            Code::CompleteWithSkips
        }
        _ if s.starts_with("unsupported_format(") => {
            let arg = s.strip_prefix("unsupported_format(")?.strip_suffix(')')?;
            if arg.parse::<i128>().is_ok() {
                Code::CompleteWithSkips
            } else if unityrs_export::ngui::classify::known_texture_format(arg) {
                Code::Complete
            } else {
                return None;
            }
        }
        _ => {
            let l = limit(s)?;
            crate::reason::ObjectLimit::new(l)?;
            if l.kind.is_budget() {
                Code::Budget
            } else {
                Code::CompleteWithSkips
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attempt::{AttemptView, StagedBundle};
    #[test]
    fn adapters_reject_unknown_vocabulary_and_wrong_sites() {
        for text in [
            "new_reason",
            "limit(new_kind, 3)",
            "limit(export_work, 3)",
            "limit(attempt_evictions, 2)",
            "unsupported_format(new_format)",
            "unwritable_key(new_rule)",
        ] {
            assert_eq!(skip_code(text), None, "{text}");
        }
        assert_eq!(
            skip_code("unsupported_format(ETC2_RGBA8)"),
            Some(Code::Complete)
        );
        assert_eq!(
            skip_code("unsupported_format(100)"),
            Some(Code::CompleteWithSkips)
        );
        assert_eq!(skip_code("decoder_panic"), Some(Code::CompleteWithSkips));
        let name = unityrs_core::RawStr::from_static(b"x");
        let view = AttemptView {
            input_index: 0,
            input: StagedBundle {
                id: "x",
                name: &name,
                path: std::path::Path::new("unused"),
            },
            members: &[],
        };
        for text in [
            "new_reason",
            "dependency_failed(absent, not_a_bundle)",
            "limit(texture_pixels, 1)",
        ] {
            let mut end = AttemptEnd::new(0);
            failure(&mut end, &Failure::Input(Reason::plain(text)), view);
            assert_eq!(end.reason, Some(ReasonCode::Internal));
        }
    }
    #[test]
    fn run_failure_reduction_preserves_code_precedence() {
        let mut end = AttemptEnd::new(0);
        end.run_failure(ReasonCode::HostIo);
        end.run_failure(ReasonCode::SpoolFull);
        assert_eq!(end.reason, Some(ReasonCode::HostIo));
        end.run_failure(ReasonCode::Internal);
        end.run_failure(ReasonCode::HostMemory);
        assert_eq!(end.reason, Some(ReasonCode::Internal));
    }
}
