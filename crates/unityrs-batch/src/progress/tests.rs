//! The line's fields, in section 10's order, and its detail's cut.

use serde_json::Value;

use super::{Exit, Phase, Snapshot};
use crate::reason::Code;
use crate::spec::MAX_DETAIL_BYTES;

const KEYS: [&str; 14] = [
    "p",
    "type",
    "seq",
    "ms",
    "phase",
    "inputs",
    "transfers",
    "memory",
    "spool",
    "outputs",
    "status",
    "code",
    "reason",
    "detail",
];

fn keys(line: &str) -> Vec<String> {
    // serde_json's map sorts its keys: read them in written order instead.
    let mut out = Vec::new();
    let mut depth = 0_u32;
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            '"' => {
                let end = line[i + 1..].find('"').unwrap() + i + 1;
                if depth == 1 && line[end + 1..].starts_with(':') {
                    out.push(line[i + 1..end].to_owned());
                }
                while chars.peek().is_some_and(|&(j, _)| j <= end) {
                    chars.next();
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn an_exit_line_holds_every_field_in_order() {
    let mut s = Snapshot::validate(12);
    s.inputs.total = 3;
    s.inputs.queued = 3;
    s.memory.inflight_limit = 7;
    s.spool.size = 9;
    let exit = Exit {
        status: None,
        code: Code::Complete,
        reason: None,
        detail: Some("memory_bound 42".into()),
    };
    let line = s.line(Some(&exit));
    assert_eq!(
        line,
        "{\"p\":1,\"type\":\"exit\",\"seq\":0,\"ms\":12,\"phase\":\"validate\",\
         \"inputs\":{\"total\":3,\"complete\":0,\"failed\":0,\"running\":0,\"queued\":3,\"not_run\":0},\
         \"transfers\":{\"stages_active\":0,\"stages_done\":0,\"stages_failed\":0,\
         \"uploads_active\":0,\"uploads_done\":0,\"uploads_failed\":0},\
         \"memory\":{\"inflight_limit\":7,\"slots_reserved\":0,\"attempts_used\":0,\"phase_used\":0,\"rss\":0},\
         \"spool\":{\"used\":0,\"size\":9},\
         \"outputs\":{\"candidates\":0,\"winners\":0,\"published\":0,\"present\":0,\"failed\":0},\
         \"status\":null,\"code\":0,\"reason\":null,\"detail\":\"memory_bound 42\"}\n"
    );
    assert_eq!(keys(&line), KEYS);
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["code"], 0);
}

#[test]
fn a_snapshot_line_holds_nulls_where_an_exit_line_names_its_end() {
    let mut s = Snapshot::validate(0);
    s.seq = 4;
    s.phase = Phase::Extract;
    let line = s.line(None);
    assert_eq!(keys(&line), KEYS);
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["type"], "snapshot");
    assert_eq!(v["phase"], "extract");
    for k in ["status", "code", "reason", "detail"] {
        assert!(v[k].is_null(), "{k}");
    }
}

#[test]
fn a_detail_is_cut_to_fit_as_written() {
    let s = Snapshot::validate(0);
    let exit = |detail: String| Exit {
        status: Some("failed"),
        code: Code::Internal,
        reason: Some("bad_spec".into()),
        detail: Some(detail),
    };
    // A quote escapes to two bytes, U+0001 to six.
    for filler in ["a", "\"", "\u{1}", "\u{e9}"] {
        let line = s.line(Some(&exit(filler.repeat(2000))));
        let v: Value = serde_json::from_str(&line).unwrap();
        let written = serde_json::to_string(&v["detail"]).unwrap();
        let written = written.len() - 2;
        assert!(written <= MAX_DETAIL_BYTES, "{filler:?}");
        assert!(written > MAX_DETAIL_BYTES - 6, "{filler:?}");
        assert!(line.len() < 4096, "under PIPE_BUF");
    }
    let line = s.line(Some(&exit("é".into())));
    assert!(line.ends_with(
        "\"status\":\"failed\",\"code\":70,\"reason\":\"bad_spec\",\"detail\":\"é\"}\n"
    ));
}

/// `starttime` is read after the command name's last `)`, whatever the
/// name holds, in clock ticks.
#[test]
fn the_start_time_is_the_22nd_field_after_the_name() {
    use super::clock::start_time;
    use std::time::Duration;
    // Fields 3 to 21 as `1`, then `starttime` (22nd), then two more.
    let rest = vec!["1"; 19].join(" ");
    let stat = format!("4242 (a) (b c) {rest} 12345 7 8\n");
    assert_eq!(
        start_time(stat.as_bytes(), 100),
        Some(Duration::from_millis(123_450))
    );
    assert_eq!(start_time(stat.as_bytes(), 0), None);
    assert_eq!(start_time(b"4242 (a) 1 2", 100), None);
    assert_eq!(start_time(b"no name", 100), None);
    let stat = [
        b"4242 (a\xff) (b c) ".as_slice(),
        format!("{rest} 12345 7 8\n").as_bytes(),
    ]
    .concat();
    assert_eq!(start_time(&stat, 100), Some(Duration::from_millis(123_450)));
}

#[allow(clippy::unnecessary_wraps)] // A `Clock`'s `now`.
fn at_105_s() -> Option<std::time::Duration> {
    Some(std::time::Duration::from_secs(105))
}

fn no_boot_clock() -> Option<std::time::Duration> {
    None
}

/// `ms` counts from the start on the boot clock it is given, and falls
/// back to the `Instant` when that clock gives nothing.
#[test]
fn ms_counts_on_the_injected_boot_clock() {
    use super::clock::Clock;
    use std::time::{Duration, Instant};
    let c = Clock::on_boot_clock(Duration::from_secs(100), at_105_s, Instant::now());
    assert!(c.is_boot_clock());
    assert_eq!(c.ms(), 5000);
    let late = Clock::on_boot_clock(Duration::from_secs(200), at_105_s, Instant::now());
    assert_eq!(late.ms(), 0, "a start after now reads 0");
    let none = Clock::on_boot_clock(Duration::from_secs(100), no_boot_clock, Instant::now());
    assert!(none.ms() < 60_000);
}

/// On Linux the process's clock is the boot clock's, counting from the
/// process's own start, so it reads at least the time since a later
/// `Instant`, and no more than the test process has lived.
#[cfg(target_os = "linux")]
#[test]
fn the_process_clock_reads_the_boot_clock_from_the_process_start() {
    use super::clock::Clock;
    use std::time::{Duration, Instant};
    let later = Instant::now();
    let c = Clock::process(later);
    assert!(c.is_boot_clock());
    std::thread::sleep(Duration::from_millis(20));
    let ms = c.ms();
    let since = u64::try_from(later.elapsed().as_millis()).unwrap();
    // The start is to a tick (10 ms at 100 a second), which rounds down.
    assert!(ms + 10 >= since, "{ms} {since}");
    assert!(ms < 3_600_000, "{ms}");
}
