use super::{
    Budget, Code, InputLimit, InputReason, KeyRule, LeftOut, Limit, LimitClass, LimitKind,
    ObjectLimit, ObjectSkip, Outcome, OutputReason, Platform, Preflight, StopReason, Warning,
    normal_end, outcome_code,
};

fn limit(kind: LimitKind, value: u64) -> Limit {
    Limit { kind, value }
}

/// An input's `limit`.
fn il(kind: LimitKind, value: u64) -> InputLimit {
    InputLimit::new(limit(kind, value)).expect("an input's limit")
}

/// An object's `limit`.
fn ol(kind: LimitKind, value: u64) -> ObjectLimit {
    ObjectLimit::new(limit(kind, value)).expect("an object's limit")
}

#[test]
fn codes_and_their_precedence() {
    let values: Vec<u8> = Code::PRECEDENCE.iter().map(|c| c.value()).collect();
    assert_eq!(values, [78, 69, 70, 74, 7, 6, 5, 4, 3, 0]);
    for c in Code::PRECEDENCE {
        assert_eq!(Code::from_value(c.value()), Some(c));
    }
    for v in [1, 2, 64, 65, 66, 67, 68] {
        assert_eq!(Code::from_value(v), None, "{v} is never used");
    }
    assert_eq!(Code::Budget.worst(Code::Incomplete), Code::Budget);
    assert_eq!(Code::Incomplete.worst(Code::Budget), Code::Budget);
    assert_eq!(Code::Host.worst(Code::Internal), Code::Internal);
    assert_eq!(Code::Stopped.worst(Code::Superseded), Code::Superseded);
    assert_eq!(Code::Complete.status(), Some("complete"));
    assert_eq!(
        Code::CompleteWithSkips.status(),
        Some("complete_with_skips")
    );
    assert_eq!(Code::Budget.status(), Some("incomplete"));
    assert_eq!(Code::BadSpec.status(), None);
}

#[test]
fn the_limit_kind_table() {
    use unityrs_core::LimitKind as K;

    let names: Vec<&str> = LimitKind::ALL.iter().map(|k| k.name()).collect();
    assert_eq!(
        names,
        [
            "input_memory",
            "input_bytes",
            "decode_work",
            "attempt_evictions",
            "export_work",
            "block_output",
            "inflate",
            "container_depth",
            "tree_depth",
            "value_depth",
            "values_per_object",
            "zero_size_elements",
            "count",
            "texture_pixels",
        ]
    );
    let class = |k: LimitKind| k.class();
    assert_eq!(class(LimitKind::DecodeWork), LimitClass::Budget);
    assert_eq!(class(LimitKind::ExportWork), LimitClass::InputBudget);
    assert_eq!(class(LimitKind::Count), LimitClass::Data);
    assert_eq!(class(LimitKind::TexturePixels), LimitClass::ObjectData);
    for (core, ours) in [
        (K::InputMemory, LimitKind::InputMemory),
        (K::InputBytes, LimitKind::InputBytes),
        (K::DecodeWork, LimitKind::DecodeWork),
        (K::BlockOutput, LimitKind::BlockOutput),
        (K::Inflate, LimitKind::Inflate),
        (K::ContainerDepth, LimitKind::ContainerDepth),
        (K::TreeDepth, LimitKind::TreeDepth),
        (K::ValueDepth, LimitKind::ValueDepth),
        (K::ValuesPerObject, LimitKind::ValuesPerObject),
        (K::ZeroSizeElements, LimitKind::ZeroSizeElements),
        (K::Count, LimitKind::Count),
    ] {
        assert_eq!(LimitKind::from_core(core), Some(ours));
    }
}

#[test]
fn reasons_render_as_name_and_arguments() {
    let dep = InputReason::DependencyFailed {
        id: "shared/a".into(),
        reason: Box::new(InputReason::Limit(il(LimitKind::InputBytes, 1_048_576))),
    };
    assert_eq!(
        dep.to_string(),
        "dependency_failed(shared/a, limit(input_bytes, 1048576))"
    );
    assert_eq!(InputReason::NotABundle.to_string(), "not_a_bundle");
    assert_eq!(
        InputReason::Budget(Budget::Dependency).to_string(),
        "budget(dependency)"
    );
    assert_eq!(ObjectSkip::BudgetInput.to_string(), "budget(input)");
    assert_eq!(
        ObjectSkip::UnsupportedFormat("ETC2_RGBA8".into()).to_string(),
        "unsupported_format(ETC2_RGBA8)"
    );
    assert_eq!(
        ObjectSkip::UnsupportedPlatform(Platform::Xbox360).to_string(),
        "unsupported_platform(xbox360)"
    );
    assert_eq!(
        ObjectSkip::UnwritableKey(KeyRule::DotSegment).to_string(),
        "unwritable_key(dot_segment)"
    );
    assert_eq!(
        ObjectSkip::OutsideView("CAB-0123".into()).to_string(),
        "outside_view(CAB-0123)"
    );
    assert_eq!(
        ObjectSkip::Limit(ol(LimitKind::TexturePixels, 268_435_456)).to_string(),
        "limit(texture_pixels, 268435456)"
    );
    assert_eq!(
        OutputReason::UnwritableKey(KeyRule::PrefixConflict).to_string(),
        "unwritable_key(prefix_conflict)"
    );
    assert_eq!(
        LeftOut::UnwritablePath(KeyRule::TooLong).to_string(),
        "unwritable_path(too_long)"
    );
    assert_eq!(StopReason::ParentDied.to_string(), "parent_died");
}

#[test]
fn the_closed_sets_by_name() {
    let names = |v: &[String]| v.join(" ");
    let preflight: Vec<String> = Preflight::ALL.iter().map(ToString::to_string).collect();
    assert_eq!(
        names(&preflight),
        "spool_not_dedicated spool_in_memory spool_overlaps_sink spool_locked \
         spool_no_space spool_unusable sink_root_unusable output_locked \
         manifest_unreadable bucket_missing sink_unreachable previous_attempt_live \
         credentials_missing credentials_invalid header_env_missing header_env_too_long \
         header_env_invalid credentials_expire deadline_too_close rlimit_too_low \
         overcommit_strict map_count_too_low memory_too_small cgroup_unreadable \
         procfs_unreadable startup_failed thread_spawn_failed kernel_too_old \
         mount_crossing mount_check_unavailable"
    );
    let warnings: Vec<String> = Warning::ALL.iter().map(ToString::to_string).collect();
    assert_eq!(
        names(&warnings),
        "resume_list_failed verify_list_failed etag_not_md5 stderr_dropped abort_failed \
         stdin_watch_ended"
    );
    let input: Vec<String> = [
        InputReason::DownloadFailed,
        InputReason::RedirectRefused,
        InputReason::SourceMissing,
        InputReason::SourceNotRegular,
        InputReason::SourceChanged,
        InputReason::IntegrityMismatch,
        InputReason::NotABundle,
        InputReason::ContainerError,
        InputReason::EntryError,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        names(&input),
        "download_failed redirect_refused source_missing source_not_regular \
         source_changed integrity_mismatch not_a_bundle container_error entry_error"
    );
    let objects: Vec<String> = [
        ObjectSkip::BadPointer,
        ObjectSkip::DecodeError,
        ObjectSkip::DecoderPanic,
        ObjectSkip::EmptyImage,
        ObjectSkip::DuplicateName,
        ObjectSkip::DuplicateKey,
        ObjectSkip::EmptyRect,
        ObjectSkip::UnnamedAtlas,
        ObjectSkip::ReferenceAtlas,
        ObjectSkip::SheetNotFound,
        ObjectSkip::NguiLayoutUnsupported,
        ObjectSkip::NoTypetree,
        ObjectSkip::JsonUnwritable,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        names(&objects),
        "bad_pointer decode_error decoder_panic empty_image duplicate_name duplicate_key \
         empty_rect unnamed_atlas reference_atlas sheet_not_found ngui_layout_unsupported \
         no_typetree json_unwritable"
    );
    let outputs: Vec<String> = [
        OutputReason::Withheld,
        OutputReason::UnitConflict,
        OutputReason::UnitLost,
        OutputReason::PublishIncomplete,
        OutputReason::PublishFailed,
        OutputReason::PublishMismatch,
        OutputReason::CredentialsExpired,
        OutputReason::ArchiveTooLarge,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        names(&outputs),
        "withheld unit_conflict unit_lost publish_incomplete publish_failed \
         publish_mismatch credentials_expired archive_too_large"
    );
    let left: Vec<String> = [LeftOut::DuplicateMember, LeftOut::PrefixConflict]
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(names(&left), "duplicate_member prefix_conflict");
    let rules: Vec<&str> = KeyRule::ALL.iter().map(|r| r.name()).collect();
    assert_eq!(
        rules.join(" "),
        "empty not_utf8 control_char leading_slash trailing_slash empty_segment \
         dot_segment segment_too_long too_long reserved_prefix reserved_name \
         prefix_conflict archive_conflict"
    );
}

#[test]
fn the_limits_9_3_forbids_cannot_be_made() {
    for k in LimitKind::ALL {
        let l = limit(k, 1);
        assert_eq!(
            InputLimit::new(l).is_some(),
            k != LimitKind::TexturePixels,
            "{}",
            k.name()
        );
        assert_eq!(
            ObjectLimit::new(l).is_some(),
            !matches!(k, LimitKind::AttemptEvictions | LimitKind::ExportWork),
            "{}",
            k.name()
        );
    }
}

/// The core's `LimitKind` variants, read from its source: B1 maps each,
/// and a kind the core adds fails this test until the table has it.
#[test]
fn from_core_maps_every_core_kind() {
    use unityrs_core::LimitKind as K;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../unityrs-core/src/error.rs");
    let src = std::fs::read_to_string(path).expect("the core's error.rs");
    let body = src
        .split_once("pub enum LimitKind {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("the core's LimitKind");
    let variants: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
        .map(|l| l.trim_end_matches(','))
        .collect();
    let mapped = [
        ("InputMemory", K::InputMemory),
        ("InputBytes", K::InputBytes),
        ("BlockOutput", K::BlockOutput),
        ("DecodeWork", K::DecodeWork),
        ("Inflate", K::Inflate),
        ("ContainerDepth", K::ContainerDepth),
        ("TreeDepth", K::TreeDepth),
        ("ValueDepth", K::ValueDepth),
        ("ValuesPerObject", K::ValuesPerObject),
        ("ZeroSizeElements", K::ZeroSizeElements),
        ("Count", K::Count),
    ];
    let names: Vec<&str> = mapped.iter().map(|(n, _)| *n).collect();
    assert_eq!(variants, names, "the core's kinds changed: map them (9.3)");
    for (name, k) in mapped {
        let ours = LimitKind::from_core(k).expect(name);
        assert_eq!(format!("{k:?}"), name);
        assert!(LimitKind::ALL.contains(&ours));
    }
}

#[test]
fn stops_take_their_codes() {
    use StopReason as S;
    for (s, c) in [
        (S::Deadline, 6),
        (S::Sigterm, 6),
        (S::Sigint, 6),
        (S::Sighup, 6),
        (S::ParentDied, 6),
        (S::StdinEof, 6),
        (S::SpoolFull, 5),
        (S::HostIo, 74),
        (S::HostMemory, 74),
        (S::Internal, 70),
    ] {
        assert_eq!(s.code().value(), c, "{s}");
    }
}

fn code(o: Outcome<'_>) -> Option<u8> {
    outcome_code(o).map(Code::value)
}

fn input(r: &InputReason) -> Option<u8> {
    code(Outcome::InputFailed(r))
}

#[test]
#[allow(clippy::too_many_lines)]
fn outcomes_map_to_codes() {
    // Objects.
    assert_eq!(
        code(Outcome::ObjectSkipped(&ObjectSkip::BudgetInput)),
        Some(5)
    );
    for k in [
        LimitKind::InputMemory,
        LimitKind::InputBytes,
        LimitKind::DecodeWork,
    ] {
        assert_eq!(
            code(Outcome::ObjectSkipped(&ObjectSkip::Limit(ol(k, 1)))),
            Some(5)
        );
    }
    for k in [LimitKind::ValueDepth, LimitKind::TexturePixels] {
        assert_eq!(
            code(Outcome::ObjectSkipped(&ObjectSkip::Limit(ol(k, 1)))),
            Some(3)
        );
    }
    assert_eq!(
        code(Outcome::ObjectSkipped(&ObjectSkip::DecodeError)),
        Some(3)
    );
    // Inputs.
    assert_eq!(input(&InputReason::Budget(Budget::Dependency)), Some(5));
    assert_eq!(
        input(&InputReason::Limit(il(LimitKind::ExportWork, 9))),
        Some(5)
    );
    assert_eq!(
        input(&InputReason::Limit(il(LimitKind::AttemptEvictions, 9))),
        Some(5)
    );
    assert_eq!(input(&InputReason::Limit(il(LimitKind::Count, 9))), Some(4));
    assert_eq!(input(&InputReason::DownloadFailed), Some(4));
    assert_eq!(input(&InputReason::NotABundle), Some(4));
    let dep = |r: InputReason| InputReason::DependencyFailed {
        id: "d".into(),
        reason: Box::new(r),
    };
    assert_eq!(
        input(&dep(InputReason::Limit(il(LimitKind::InputBytes, 1)))),
        Some(5)
    );
    assert_eq!(
        input(&dep(InputReason::Budget(Budget::Dependency))),
        Some(5)
    );
    assert_eq!(
        input(&dep(InputReason::Limit(il(LimitKind::ExportWork, 1)))),
        Some(4)
    );
    assert_eq!(input(&dep(InputReason::IntegrityMismatch)), Some(4));
    assert_eq!(
        code(Outcome::BundleFailed(&InputReason::SourceMissing)),
        Some(4)
    );
    assert_eq!(
        code(Outcome::BundleFailed(&InputReason::Limit(il(
            LimitKind::InputBytes,
            1
        )))),
        Some(5)
    );
    // Outputs, members and archives.
    for r in [
        OutputReason::PublishFailed,
        OutputReason::PublishMismatch,
        OutputReason::CredentialsExpired,
        OutputReason::ArchiveTooLarge,
    ] {
        assert_eq!(code(Outcome::Output(&r)), Some(4), "{r}");
    }
    assert_eq!(code(Outcome::NotWritten), Some(4));
    for r in [
        OutputReason::UnitConflict,
        OutputReason::UnwritableKey(KeyRule::PrefixConflict),
        OutputReason::UnwritableKey(KeyRule::ArchiveConflict),
    ] {
        assert_eq!(code(Outcome::Output(&r)), Some(3), "{r}");
    }
    assert_eq!(
        code(Outcome::MemberLeftOut(&LeftOut::DuplicateMember)),
        Some(3)
    );
    assert_eq!(
        code(Outcome::MemberLeftOut(&LeftOut::UnwritablePath(
            KeyRule::Empty
        ))),
        Some(3)
    );
    assert_eq!(
        code(Outcome::MemberLeftOut(&LeftOut::PrefixConflict)),
        Some(3)
    );
    assert_eq!(code(Outcome::Output(&OutputReason::UnitLost)), None);
    assert_eq!(
        code(Outcome::Output(&OutputReason::PublishIncomplete)),
        None
    );
    assert_eq!(code(Outcome::Output(&OutputReason::Withheld)), None);
    assert_eq!(code(Outcome::ArchiveEmpty), None);
}

#[test]
fn a_normal_end_takes_the_worst_code() {
    assert_eq!(normal_end([]), Code::Complete);
    assert_eq!(normal_end([Outcome::ArchiveEmpty]), Code::Complete);
    let skip = ObjectSkip::EmptyImage;
    let failed = InputReason::EntryError;
    let budget = ObjectSkip::BudgetInput;
    assert_eq!(
        normal_end([Outcome::ObjectSkipped(&skip)]),
        Code::CompleteWithSkips
    );
    assert_eq!(
        normal_end([
            Outcome::ObjectSkipped(&skip),
            Outcome::InputFailed(&failed),
            Outcome::ObjectSkipped(&skip),
        ]),
        Code::Incomplete
    );
    assert_eq!(
        normal_end([
            Outcome::InputFailed(&failed),
            Outcome::ObjectSkipped(&budget),
        ]),
        Code::Budget
    );
}
