//! `unityrs batch` and `unityrs --version` (`docs/BATCH_SPEC.md` sections
//! 9 and 10):
//!
//! ```text
//! unityrs batch --check JOB.json   validate; exit 0 or 78, one `exit` line
//! unityrs batch --schema           the spec's JSON Schema; exit 0, 74, or 69 at step 1
//! unityrs batch JOB.json           a normal run: local or URL sources, a directory sink
//! unityrs --version                one line; exit 0, 74, or 69 at step 1
//! ```
//!
//! Each starts as `docs/plans/batch.md` 2.11 step 1 does, with SIGTTIN,
//! SIGTTOU and SIGXFSZ ignored ([`unityrs_batch::sys::ignore_job_control`]),
//! so no write of a terminal stops it; a failure there is 69
//! `startup_failed`.
//!
//! `--check` runs phase 1 (2.11 step 2: read, validate and prepare `JOB.json`) under
//! the counting allocator's measure, and writes the `exit` line with `seq`
//! 0: for a valid spec code 0 and the detail `memory_bound <bytes>`, 5.1's
//! bound (**provisional** until X12 measures its constants:
//! `unityrs_batch::bound`; on a host whose pages are not 4 KiB also
//! uncertified, which a stderr line says), else 78 and the `bad_spec`
//! detail. A bad
//! command line is 78 `usage`. `--schema` and `--version` write stdout and
//! exit 74 when that write fails.
//!
//! A run starts with 2.11 step 1's services
//! (`unityrs_batch::run::process::Startup`), then phase 1, then the process
//! host, which ends the process itself. Runs use the local normal-run
//! subset, archives and the lifetime fields (`deadline`,
//! `stop_on_stdin_eof`, `parent_pid`) included; local and URL sources are
//! supported. The object store is refused with 78 before mutation.

mod normal;

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::ExitCode;

use unityrs_batch::alloc;
use unityrs_batch::bound::{CHECK_ENV_BLOCK, Certification, SELECTED, memory_bound};
use unityrs_batch::phase1::{self, PreparedRun};
use unityrs_batch::progress::clock::Clock;
use unityrs_batch::progress::{Exit, Snapshot, early, rss};
use unityrs_batch::reason::Code;
use unityrs_batch::spec::{self, BadSpec, SPEC_VERSION};

use crate::args::option;
const CHECK_OPTION: crate::agent::options::OptionSpec = option(
    "--check",
    Some("JOB.json"),
    None,
    "Validate the job without running it.",
);
const SCHEMA_OPTION: crate::agent::options::OptionSpec = option(
    "--schema",
    None,
    None,
    "Print the existing batch job schema; see unityrs batch --schema.",
);
pub(crate) const OPTIONS: &[crate::agent::options::OptionSpec] = &[CHECK_OPTION, SCHEMA_OPTION];
pub(crate) fn options() -> Vec<crate::agent::options::OptionSpec> {
    OPTIONS.to_vec()
}

pub const USAGE: &str = "usage: unityrs batch --check JOB.json\n       \
                         unityrs batch --schema\n       \
                         unityrs batch JOB.json   (temporary normal-run subset: local and URL sources, directory sink)";

const VERSION_USAGE: &str = "usage: unityrs --version";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command<'a> {
    Check(&'a [u8]),
    Schema,
    Run(&'a [u8]),
}

/// A path argument: not an option, though `-` is a path.
fn path(a: &[u8]) -> Option<&[u8]> {
    (!a.is_empty() && (!a.starts_with(b"-") || a == b"-")).then_some(a)
}

fn command(args: &[Vec<u8>]) -> Option<Command<'_>> {
    match args {
        [o] if o == SCHEMA_OPTION.name.as_bytes() => Some(Command::Schema),
        [o, p] if o == CHECK_OPTION.name.as_bytes() => path(p).map(Command::Check),
        [p] => path(p).map(Command::Run),
        _ => None,
    }
}

/// `unityrs batch`'s arguments after `batch`.
pub fn main(clock: &Clock, args: &[Vec<u8>]) -> ExitCode {
    if let Err(e) = unityrs_batch::sys::ignore_job_control() {
        return startup_failed(clock, &e);
    }
    match command(args) {
        Some(Command::Schema) => stdout(spec::SCHEMA),
        Some(Command::Check(p)) => match phase1(p) {
            (Ok(v), m) => checked(clock, &v, m),
            (Err(error), _) => phase1_failed(clock, error),
        },
        Some(Command::Run(p)) => {
            // 2.11 step 1's services, before phase 1.
            let mut startup = match unityrs_batch::run::process::Startup::prepare() {
                Ok(startup) => startup,
                Err(error) => return start_failed(clock, &error),
            };
            // The spares closed through phase 1's read, so a low
            // `RLIMIT_NOFILE` leaves `JOB.json` its descriptor, and opened
            // again once the spec is validated.
            startup.release_spares();
            let phase1 = phase1(p);
            startup.restore_spares();
            match phase1 {
                (Ok(prepared), measured) => normal::run(clock, startup, prepared, measured),
                (Err(Phase1Error::Descriptors(errno)), _) => {
                    drop(startup);
                    start_failed(
                        clock,
                        &unityrs_batch::run::Error::startup("open", Some(errno)),
                    )
                }
                (Err(error), _) => {
                    // Its spare descriptors closed before the lines.
                    drop(startup);
                    phase1_failed(clock, error)
                }
            }
        }
        None => refused(
            clock,
            &BadSpec::usage(),
            "unityrs batch",
            &format!("{USAGE}\n"),
        ),
    }
}

/// `unityrs --version`'s arguments after `--version`: none, else 78
/// `usage`.
pub fn version(clock: &Clock, args: &[Vec<u8>]) -> ExitCode {
    if let Err(e) = unityrs_batch::sys::ignore_job_control() {
        return startup_failed(clock, &e);
    }
    if !args.is_empty() {
        return refused(
            clock,
            &BadSpec::usage(),
            "unityrs",
            &format!("{VERSION_USAGE}\n"),
        );
    }
    let line = format!(
        "unityrs {} spec {SPEC_VERSION}\n",
        env!("CARGO_PKG_VERSION")
    );
    stdout(line.as_bytes())
}

/// `bytes` on stdout: 0, or 74 when the write fails (9.1).
fn stdout(bytes: &[u8]) -> ExitCode {
    let mut out = std::io::stdout().lock();
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => code(Code::Host),
    }
}

/// The bound's worker stack is the dump's: each container level's frame.
const _: () = assert!(
    unityrs_batch::bound::CONTAINER_LEVEL_STACK == unityrs_dump::run::CONTAINER_LEVEL_STACK as u64
);

fn code(c: Code) -> ExitCode {
    ExitCode::from(c.value())
}

enum Phase1Error {
    Validation(BadSpec),
    /// `JOB.json`'s open at a full descriptor table: 69, the host's.
    Descriptors(i32),
    Preparation(Code),
}
/// Phase 1 on `path`, measured from its read through preparation (2.7, 5.3).
fn phase1(p: &[u8]) -> (Result<PreparedRun, Phase1Error>, alloc::Measured) {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(p));
    alloc::measured(|| {
        let bytes = spec::read::read_job_on_host(path).map_err(|e| match e {
            spec::read::ReadError::Bad(bad) => Phase1Error::Validation(bad),
            spec::read::ReadError::Descriptors(errno) => Phase1Error::Descriptors(errno),
        })?;
        let validated = spec::parse(bytes).map_err(Phase1Error::Validation)?;
        // Validation used the same template parser. Its accepted templates
        // compile here before phase 1's retained-heap measurement ends.
        phase1::prepare(validated).map_err(|error| preparation_error(&error))
    })
}

fn preparation_error(error: &phase1::PreparationError) -> Phase1Error {
    let memory = matches!(error,
        phase1::PreparationError::Winners(unityrs_batch::phase::Error::Run(f))
            if f.reason == unityrs_batch::attempt::ReasonCode::HostMemory);
    Phase1Error::Preparation(if memory { Code::Host } else { Code::Internal })
}

fn phase1_failed(clock: &Clock, error: Phase1Error) -> ExitCode {
    let c = match error {
        Phase1Error::Validation(bad) => return refused(clock, &bad, "unityrs batch", ""),
        // `--check`'s: a run's exception alone (`BATCH_SPEC.md` section
        // 10), so `--check` keeps 78 `bad_spec unreadable`.
        Phase1Error::Descriptors(errno) => {
            let bad = spec::read::ReadError::Descriptors(errno).into_bad_spec();
            return refused(clock, &bad, "unityrs batch", "");
        }
        Phase1Error::Preparation(code) => code,
    };
    let reason = if c == Code::Host {
        "host_memory"
    } else {
        "internal"
    };
    let exit = Exit {
        status: None,
        code: c,
        reason: Some(reason),
        detail: None,
    };
    early::write(
        Snapshot::validate(clock.ms()).line(Some(&exit)).as_bytes(),
        if c == Code::Host {
            b"unityrs batch: host_memory\n"
        } else {
            b"unityrs batch: internal\n"
        },
    );
    code(c)
}

/// In-process check: same validator, preparation and memory-bound report as
/// the CLI. No startup services, file reads, environment lookup or execution.
pub(crate) fn check_document(
    bytes: Vec<u8>,
    memory: u64,
    work: u64,
) -> Result<String, (&'static str, String)> {
    use unityrs_core::limits::heap_cost;
    // Byte-proportional parser/template tables plus fixed per-bundle contexts
    // and numeric slot counts. Refuse before preparation allocates its tables.
    let base = (bytes.len() as u64)
        .saturating_mul(64)
        .saturating_add(65_536);
    if base > memory || base > work {
        return Err((
            "budget",
            "Batch validation reservation exceeds server memory/work ceiling".into(),
        ));
    }
    let (prepared, measured) = alloc::measured(|| {
        let validated = spec::parse_with(
            bytes,
            (work.saturating_sub(base) / 2).min(spec::fields::VIEW_WORK),
        )
        .map_err(|bad| {
            (
                if bad.rule == spec::Rule::ViewSize
                    && (work.saturating_sub(base) / 2) < spec::fields::VIEW_WORK
                {
                    "budget"
                } else {
                    "usage"
                },
                bad.detail(),
            )
        })?;
        let s = &validated.spec;
        let table = |count: u64, size: usize| heap_cost(count.saturating_mul(size as u64));
        let reservation = base
            .saturating_add((s.bundle_count() as u64).saturating_mul(16_384))
            .saturating_add(table(
                u64::from(s.threads),
                std::mem::size_of::<phase1::ActiveSlot>(),
            ))
            .saturating_add(table(
                u64::from(s.threads),
                std::mem::size_of::<Option<phase1::Dispatched>>(),
            ))
            .saturating_add(table(
                u64::from(s.download_concurrency),
                std::mem::size_of::<phase1::StageSlot>(),
            ));
        if reservation > memory || reservation > work.saturating_sub(base) / 2 {
            return Err((
                "budget",
                "Batch preparation reservation exceeds server memory/work ceiling".into(),
            ));
        }
        phase1::prepare(validated)
            .map_err(|_| ("budget", "Phase-one preparation refused".to_owned()))
    });
    let prepared = prepared?;
    Ok(check_line(0, &prepared, measured))
}
fn check_line(ms: u64, v: &PreparedRun, m: alloc::Measured) -> String {
    let s = &v.spec;
    let bound = memory_bound(s, m, CHECK_ENV_BLOCK, &SELECTED);
    let mut snap = Snapshot::validate(ms);
    let inputs = s.input_count() as u64;
    snap.inputs.total = inputs;
    snap.inputs.queued = inputs;
    snap.memory.inflight_limit = s.max_inflight_uncompressed_bytes;
    snap.memory.rss = rss();
    snap.spool.size = s.spool.size_bytes;
    let exit = Exit {
        status: None,
        code: Code::Complete,
        reason: None,
        detail: Some(format!("memory_bound {}", bound.total()).into()),
    };
    snap.line(Some(&exit))
}

/// `--check`'s line for a valid spec: code 0, `memory_bound <bytes>`.
fn checked(clock: &Clock, v: &PreparedRun, m: alloc::Measured) -> ExitCode {
    // Uncertified off 4 KiB pages (`unityrs_batch::bound`): a stderr line.
    let note = Certification::of(unityrs_batch::sys::page_size()).note("unityrs batch");
    early::write(
        check_line(clock.ms(), v, m).as_bytes(),
        note.as_deref().unwrap_or_default().as_bytes(),
    );
    code(Code::Complete)
}

/// A 78: the `exit` line, then the stderr line `<prog>: bad_spec <detail>`
/// and `then` (section 10).
fn refused(clock: &Clock, bad: &BadSpec, prog: &str, then: &str) -> ExitCode {
    let detail = bad.detail();
    let err = format!("{prog}: bad_spec {detail}\n{then}");
    end(clock, Code::BadSpec, "bad_spec", detail, &err)
}

/// A 69 at 2.11 step 1. Signal setup may have left SIGTTOU at its default
/// disposition: writing a terminal could stop us before we can exit.
/// Suppress every character device, since a denied terminal-detection
/// ioctl can make a terminal look like another character device.
fn startup_failed(clock: &Clock, e: &std::io::Error) -> ExitCode {
    startup_failed_on(
        clock,
        e,
        [std::io::stdout().as_fd(), std::io::stderr().as_fd()],
        early::write,
    )
}

/// Step-1 failure on the given descriptors; the writer receives the
/// surviving lines and applies their shared budget.
fn startup_failed_on(
    clock: &Clock,
    e: &std::io::Error,
    streams: [BorrowedFd<'_>; 2],
    write: impl FnOnce(&[u8], &[u8]),
) -> ExitCode {
    let (out, err) = startup_failure_lines(clock.ms(), e);
    let [out_char, err_char] = streams.map(|fd| {
        fd.try_clone_to_owned()
            .ok()
            .and_then(|dup| File::from(dup).metadata().ok())
            .is_some_and(|meta| meta.file_type().is_char_device())
    });
    write(
        if out_char { b"" } else { out.as_bytes() },
        if err_char { b"" } else { err.as_bytes() },
    );
    code(Code::CouldNotStart)
}

/// Pure step-1 failure lines: no procfs reads or stream operations. Known
/// errnos use their names, unnamed ones their decimal value; only an
/// error with no errno gives the call alone.
fn startup_failure_lines(ms: u64, e: &std::io::Error) -> (String, String) {
    let detail = match e.raw_os_error() {
        Some(libc::EINVAL) => "signal EINVAL".to_owned(),
        Some(libc::EPERM) => "signal EPERM".to_owned(),
        Some(libc::EFAULT) => "signal EFAULT".to_owned(),
        Some(libc::ENOSYS) => "signal ENOSYS".to_owned(),
        Some(n) => format!("signal {n}"),
        None => "signal".to_owned(),
    };
    let err = format!("unityrs: startup_failed {detail}\n");
    let exit = Exit {
        status: None,
        code: Code::CouldNotStart,
        reason: Some("startup_failed"),
        detail: Some(detail.into()),
    };
    (Snapshot::validate(ms).line(Some(&exit)), err)
}

/// A 69 from a run's step 1 services (`unityrs_batch::run::process`),
/// before phase 1: the `exit` line, then `unityrs batch: <reason> <detail>`.
fn start_failed(clock: &Clock, error: &unityrs_batch::run::Error) -> ExitCode {
    let detail = error.detail.clone().unwrap_or_default();
    let err = format!("unityrs batch: {} {detail}\n", error.reason);
    end(clock, error.code, error.reason, detail, &err)
}

/// The `exit` line of an end in phase 1, and `err` on stderr.
fn end(clock: &Clock, c: Code, reason: &'static str, detail: String, err: &str) -> ExitCode {
    let mut snap = Snapshot::validate(clock.ms());
    snap.memory.rss = rss();
    let exit = Exit {
        status: None,
        code: c,
        reason: Some(reason),
        detail: Some(detail.into()),
    };
    early::write(snap.line(Some(&exit)).as_bytes(), err.as_bytes());
    code(c)
}

#[cfg(test)]
mod tests {
    use super::{Command, command};
    #[test]
    fn preparation_memory_refusal_is_host_memory() {
        let error =
            unityrs_batch::phase1::PreparationError::Winners(unityrs_batch::phase::Error::memory());
        assert!(matches!(
            super::preparation_error(&error),
            super::Phase1Error::Preparation(unityrs_batch::reason::Code::Host)
        ));
    }

    /// Removing either suppression or changing the returned status must
    /// fail, including for a character device that is not a terminal.
    #[test]
    fn signal_setup_failure_suppresses_character_devices_and_returns_69() {
        use std::fs::OpenOptions;
        use std::io::{IsTerminal, Read, Write};
        use std::os::fd::AsFd;
        use std::time::{Duration, Instant};

        let clock = super::Clock::on_boot_clock(
            Duration::ZERO,
            || Some(Duration::from_millis(42)),
            Instant::now(),
        );
        let e = std::io::Error::from_raw_os_error(libc::EPERM);
        let device = OpenOptions::new().write(true).open("/dev/null").unwrap();
        assert!(!device.is_terminal());
        for (out_char, err_char) in [(true, false), (false, true), (true, true), (false, false)] {
            let (mut out_r, mut out_w) = std::io::pipe().unwrap();
            let (mut err_r, mut err_w) = std::io::pipe().unwrap();
            // Duplicate so the writer closure can borrow the real pipes
            // mutably while classification borrows their descriptors.
            let out_fd = out_w.try_clone().unwrap();
            let err_fd = err_w.try_clone().unwrap();
            let streams = [
                if out_char {
                    device.as_fd()
                } else {
                    out_fd.as_fd()
                },
                if err_char {
                    device.as_fd()
                } else {
                    err_fd.as_fd()
                },
            ];
            let status = super::startup_failed_on(&clock, &e, streams, |out, err| {
                out_w.write_all(out).unwrap();
                err_w.write_all(err).unwrap();
            });
            assert_eq!(status, std::process::ExitCode::from(69));
            drop((out_w, err_w, out_fd, err_fd));
            let mut out = String::new();
            let mut err = String::new();
            out_r.read_to_string(&mut out).unwrap();
            err_r.read_to_string(&mut err).unwrap();
            if out_char {
                assert!(out.is_empty(), "character-device stdout was written");
            } else {
                let v: serde_json::Value = serde_json::from_str(&out).unwrap();
                assert_eq!(v["type"], "exit");
                assert_eq!(v["code"], 69);
                assert_eq!(v["ms"], 42);
                assert_eq!(v["reason"], "startup_failed");
                assert_eq!(v["detail"], "signal EPERM");
            }
            if err_char {
                assert!(err.is_empty(), "character-device stderr was written");
            } else {
                assert_eq!(err, "unityrs: startup_failed signal EPERM\n");
            }
        }
    }

    #[test]
    fn a_signal_setup_failure_has_a_pure_exit_line() {
        for (errno, detail) in [
            (Some(libc::EINVAL), "signal EINVAL"),
            (Some(libc::EPERM), "signal EPERM"),
            (Some(libc::EFAULT), "signal EFAULT"),
            (Some(libc::ENOSYS), "signal ENOSYS"),
            (Some(123_456), "signal 123456"),
            (None, "signal"),
        ] {
            let e = errno.map_or_else(
                || std::io::Error::other("no errno"),
                std::io::Error::from_raw_os_error,
            );
            let (out, err) = super::startup_failure_lines(42, &e);
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["type"], "exit");
            assert_eq!(v["seq"], 0);
            assert_eq!(v["ms"], 42);
            assert_eq!(v["phase"], "validate");
            assert_eq!(v["code"], 69);
            assert_eq!(v["reason"], "startup_failed");
            assert_eq!(v["detail"], detail);
            assert_eq!(v["memory"]["rss"], 0);
            assert_eq!(err, format!("unityrs: startup_failed {detail}\n"));
            assert_eq!(super::startup_failure_lines(42, &e), (out, err));
        }
    }

    fn args(s: &[&str]) -> Vec<Vec<u8>> {
        s.iter().map(|a| a.as_bytes().to_vec()).collect()
    }

    #[test]
    fn the_command_lines() {
        let a = args(&["--check", "j.json"]);
        assert_eq!(command(&a), Some(Command::Check(b"j.json")));
        let a = args(&["--check", "-"]);
        assert_eq!(command(&a), Some(Command::Check(b"-")));
        assert_eq!(command(&args(&["--schema"])), Some(Command::Schema));
        let a = args(&["j.json"]);
        assert_eq!(command(&a), Some(Command::Run(b"j.json")));
        for bad in [
            &[][..],
            &["--check"],
            &["--check", "--schema"],
            &["--check", "a", "b"],
            &["--schema", "x"],
            &["--frobnicate"],
            &["a", "b"],
            &[""],
            &["--check", ""],
        ] {
            assert_eq!(command(&args(bad)), None, "{bad:?}");
        }
    }
}
