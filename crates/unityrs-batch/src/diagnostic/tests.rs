use super::*;

#[cfg(target_os = "linux")]
#[test]
fn startup_warning_retry_child() {
    if std::env::var_os("UNITYRS_DIAGNOSTIC_WARNING_CHILD").is_none() {
        return;
    }
    let writer = Writer::prepare().unwrap();
    let credentials = OnceLock::new();
    let secrets = Secrets {
        values: &[],
        credentials: &credentials,
    };
    let mut buffer = [0; LINE_BYTES];
    let started = std::time::Instant::now();
    let wait = crate::progress::early::OWN_WAIT;
    writer
        .pending_line()
        .within("warning\n", &secrets, &mut buffer, wait);
    assert!(started.elapsed() >= wait, "gave up before the retry budget");
    assert_eq!(writer.dropped(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn startup_warning_returns_at_its_budget_even_without_a_finisher_to_exit() {
    use std::io::{ErrorKind, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::{Command, Stdio};
    use unityrs_testkit::child;

    child::scene(|scene| {
        let (_read, write) = std::io::pipe()?;
        let mut fill = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(format!("/proc/self/fd/{}", write.as_raw_fd()))?;
        for bytes in [&[b'x'; 4096][..], b"x"] {
            loop {
                match fill.write(bytes) {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e),
                }
            }
        }
        drop(fill);
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args([
            "--exact",
            "diagnostic::tests::startup_warning_retry_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("UNITYRS_DIAGNOSTIC_WARNING_CHILD", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::from(write));
        let started = std::time::Instant::now();
        let child = child::live(scene, &mut cmd, false)?;
        drop(cmd);
        // Scene::wait uses DEADLOCK_GUARD; this assertion checks the
        // production budget plus process startup/teardown slack.
        let status = scene.wait(child)?;
        let budget = crate::progress::early::OWN_WAIT;
        let elapsed = started.elapsed();
        assert!(
            elapsed >= budget,
            "the child did not exercise the retry budget"
        );
        assert!(elapsed < budget + budget * 9 / 10, "{elapsed:?}");
        assert!(status.success(), "{status}");
        Ok(())
    })
    .unwrap();
}
#[test]
fn masks_short_overlapping_and_cross_boundary_secrets_before_cut() {
    let credentials = OnceLock::new();
    credentials.set(["token", "", ""]).unwrap();
    let secrets = Secrets {
        values: &["abc", "bcd", "é", "x", ""],
        credentials: &credentials,
    };
    let mut output = [0; 128];
    let n = secrets.mask("abcd é token x safe", &mut output);
    assert_eq!(&output[..n], b"*** *** *** *** safe");
    let mut short = [0; 4];
    let n = secrets.mask("ab token", &mut short);
    assert_eq!(&short[..n], b"ab ");
    let mut short = [0; 4];
    let n = secrets.mask("a abcdef", &mut short);
    assert_eq!(&short[..n], b"a ");
}
#[test]
fn a_cut_pending_line_keeps_its_newline_on_a_char_boundary() {
    // A 1,024-byte id (two-byte characters) and a 2,000-byte payload.
    let id = "é".repeat(512);
    let payload = "p".repeat(2_000);
    for (who, text) in [
        (id.as_str(), payload.as_str()),
        ("x", "é".repeat(1_000).as_str()),
    ] {
        let mut line = [0_u8; pending::LINE];
        let len = pending::fit(
            format_args!("unityrs batch: input {who}: internal: {text}\n"),
            &mut line,
        );
        assert!(len <= pending::LINE);
        let written = std::str::from_utf8(&line[..len]).unwrap();
        assert!(written.ends_with('\n'));
        assert_eq!(written.matches('\n').count(), 1);
        assert!(written.starts_with("unityrs batch: input "));
    }
    // A line that fits is left as it is.
    let mut line = [0_u8; pending::LINE];
    let len = pending::fit(format_args!("unityrs batch: internal: x\n"), &mut line);
    assert_eq!(&line[..len], b"unityrs batch: internal: x\n");
}
#[test]
fn the_pending_ring_refuses_past_its_room_and_reclaims_drained_slots() {
    let mut queue = Box::new(pending::Queue::EMPTY);
    for i in 0..pending::LINES {
        assert!(queue.push(format_args!("line {i}\n")));
    }
    assert!(!queue.push(format_args!("one too many\n")));
    let mut line = [0_u8; pending::LINE];
    for i in 0..3 {
        let len = queue.pop(&mut line).unwrap();
        assert_eq!(&line[..len], format!("line {i}\n").as_bytes());
    }
    // The three slots drained take three more, in order after the rest.
    for i in 0..3 {
        assert!(queue.push(format_args!("again {i}\n")));
    }
    assert!(!queue.push(format_args!("full again\n")));
    let mut seen = Vec::new();
    while let Some(len) = queue.pop(&mut line) {
        seen.push(String::from_utf8(line[..len].to_vec()).unwrap());
    }
    let want: Vec<String> = (3..pending::LINES)
        .map(|i| format!("line {i}\n"))
        .chain((0..3).map(|i| format!("again {i}\n")))
        .collect();
    assert_eq!(seen, want);
}
/// Eight pushers and two drains at once, one waiting for the other as long
/// as it writes and one never: every line is written or counted dropped,
/// and each pusher's lines come out in the order it queued them.
#[test]
fn concurrent_drains_lose_and_reorder_nothing() {
    use std::sync::{Mutex, atomic::Ordering::SeqCst};
    const PUSHERS: usize = 8;
    const EACH: usize = 2_000;
    let ring = Box::new(pending::Ring::new());
    let written = Mutex::new(Vec::<(usize, usize)>::new());
    let counted = std::sync::atomic::AtomicU64::new(0);
    let done = &std::sync::atomic::AtomicBool::new(false);
    let take = |until: Option<std::time::Instant>| {
        ring.drain(until, |line| {
            if let Some(rest) = line.strip_prefix("p ") {
                let mut it = rest.trim_end().split(' ');
                let p = it.next().unwrap().parse().unwrap();
                let i = it.next().unwrap().parse().unwrap();
                written.lock().unwrap().push((p, i));
            } else {
                let n = line
                    .strip_prefix("unityrs batch: ")
                    .and_then(|l| l.strip_suffix(" more internal lines dropped\n"))
                    .unwrap_or_else(|| panic!("{line:?}"));
                counted.fetch_add(n.parse().unwrap(), SeqCst);
            }
        });
    };
    std::thread::scope(|scope| {
        let pushers: Vec<_> = (0..PUSHERS)
            .map(|p| {
                let ring = &ring;
                scope.spawn(move || {
                    for i in 0..EACH {
                        ring.line(format_args!("p {p} {i}\n"));
                    }
                })
            })
            .collect();
        for bounded in [false, true] {
            scope.spawn(move || {
                while !done.load(SeqCst) {
                    take(bounded.then(std::time::Instant::now));
                }
            });
        }
        for pusher in pushers {
            pusher.join().unwrap();
        }
        done.store(true, SeqCst);
    });
    // What the drains left after the last push.
    take(None);
    let written = written.into_inner().unwrap();
    assert_eq!(
        written.len() as u64 + ring.unwritten(),
        (PUSHERS * EACH) as u64,
        "a line neither written nor counted"
    );
    assert_eq!(
        counted.load(SeqCst),
        ring.unwritten(),
        "a count in no count line"
    );
    let mut last = [None::<usize>; PUSHERS];
    for (p, i) in written {
        assert!(
            last[p].is_none_or(|l| l < i),
            "pusher {p}: {i} after {last:?}"
        );
        last[p] = Some(i);
    }
}
/// A drain held in its write (as writeback holds a regular file's) and a
/// second drain with a bound: the second returns at its bound without
/// writing, its lines counted, and the first, once its write goes on,
/// writes that count after its own line.
#[test]
fn a_drain_past_its_bound_counts_the_lines_another_drain_holds() {
    use std::{
        sync::{Mutex, mpsc},
        time::{Duration, Instant},
    };
    use unityrs_testkit::guard;
    let ring = Box::new(pending::Ring::new());
    ring.line(format_args!("first\n"));
    let written = Mutex::new(Vec::<String>::new());
    let (held_tx, held_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let (ring, written) = (&*ring, &written);
        let holder = scope.spawn(move || {
            let mut first = true;
            ring.drain(None, |line| {
                if std::mem::take(&mut first) {
                    held_tx.send(()).unwrap();
                    guard::hold(&go_rx, "the test to let the first drain's write go");
                }
                written.lock().unwrap().push(line.to_owned());
            });
        });
        guard::wait(&held_rx, "the first drain in its write");
        ring.line(format_args!("second\n"));
        ring.line(format_args!("third\n"));
        let bound = Duration::from_millis(50);
        let started = Instant::now();
        ring.drain(Some(started + bound), |line| {
            panic!("written while another drain held the ring: {line:?}")
        });
        assert!(started.elapsed() >= bound);
        assert_eq!(ring.unwritten(), 2);
        go_tx.send(()).unwrap();
        holder.join().unwrap();
    });
    assert_eq!(
        written.into_inner().unwrap(),
        ["first\n", "unityrs batch: 2 more internal lines dropped\n"]
    );
    assert_eq!(ring.unwritten(), 2);
}

/// Pushed past the ring's room, a drain held in its write of the count
/// line, and a drain past its bound counting a line queued meanwhile: the
/// holder's look after that write finds the new count and writes it.
#[test]
fn a_count_made_while_the_holder_writes_its_count_line_gets_one_of_its_own() {
    use std::sync::{Mutex, mpsc};
    use unityrs_testkit::guard;
    let ring = Box::new(pending::Ring::new());
    for i in 0..=pending::LINES {
        ring.line(format_args!("line {i}\n"));
    }
    let written = Mutex::new(Vec::<String>::new());
    let (held_tx, held_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let (ring, written) = (&*ring, &written);
        let holder = scope.spawn(move || {
            let mut first = true;
            ring.drain(None, |line| {
                if line.contains(" more internal lines dropped") && std::mem::take(&mut first) {
                    held_tx.send(()).unwrap();
                    guard::hold(&go_rx, "the test to let the count line's write go");
                }
                written.lock().unwrap().push(line.to_owned());
            });
        });
        guard::wait(&held_rx, "the holder in its count line's write");
        ring.line(format_args!("late\n"));
        ring.drain(Some(std::time::Instant::now()), |line| {
            panic!("written while another drain held the ring: {line:?}")
        });
        assert_eq!(ring.unwritten(), 2);
        go_tx.send(()).unwrap();
        holder.join().unwrap();
    });
    let count = "unityrs batch: 1 more internal lines dropped\n".to_owned();
    let want: Vec<String> = (0..pending::LINES)
        .map(|i| format!("line {i}\n"))
        .chain([count.clone(), count])
        .collect();
    assert_eq!(written.into_inner().unwrap(), want);
    assert_eq!(ring.unwritten(), 2);
}

/// A drain past its bound discards a line as the holder's write returns
/// and the holder looks at the ring for the last time (the seam held a
/// while, so a holder that need not wait would end first): the count is
/// the holder's to write, never left behind its last look.
#[test]
fn a_discard_as_the_holder_looks_last_lands_in_its_count_line() {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering::SeqCst},
        mpsc,
    };
    use std::time::{Duration, Instant};
    use unityrs_testkit::guard;
    /// How long the seam gives a holder that need not wait to end.
    const ENDS: Duration = Duration::from_millis(200);
    let ring = Box::new(pending::Ring::new());
    ring.line(format_args!("first\n"));
    let written = Mutex::new(Vec::<String>::new());
    let ended = AtomicBool::new(false);
    let (held_tx, held_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let (ring, written, ended) = (&*ring, &written, &ended);
        let holder = scope.spawn(move || {
            let mut first = true;
            ring.drain(None, |line| {
                if std::mem::take(&mut first) {
                    held_tx.send(()).unwrap();
                    guard::hold(&go_rx, "the discard to let the first write go");
                }
                written.lock().unwrap().push(line.to_owned());
            });
            ended.store(true, SeqCst);
        });
        guard::wait(&held_rx, "the holder in its first write");
        ring.line(format_args!("second\n"));
        ring.drain_at(
            Some(Instant::now()),
            |line| panic!("written while another drain held the ring: {line:?}"),
            || {
                go_tx.send(()).unwrap();
                let started = Instant::now();
                while !ended.load(SeqCst) && started.elapsed() < ENDS {
                    std::thread::sleep(Duration::from_millis(1));
                }
            },
        );
        holder.join().unwrap();
    });
    assert_eq!(
        written.into_inner().unwrap(),
        ["first\n", "unityrs batch: 1 more internal lines dropped\n"]
    );
    assert_eq!(ring.unwritten(), 1);
}

#[test]
fn formatted_secrets_cross_writes_and_are_masked_before_capture() {
    use std::fmt;
    struct Pieces<'a>(&'a str, usize);
    impl fmt::Display for Pieces<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for chunk in self.0.as_bytes().chunks(self.1) {
                f.write_str(std::str::from_utf8(chunk).unwrap())?;
            }
            Ok(())
        }
    }
    let long = "s".repeat(1024);
    let credentials = OnceLock::new();
    let secrets = Secrets {
        values: &[&long, "abc", "bcd", "bc", "abcdef"],
        credentials: &credentials,
    };
    for width in [1, 2, 17, 1024] {
        let input = format!("safe {long} abcd abcdeX safe");
        let mut out = [0; 32];
        let (n, cut) = secrets.format(format_args!("{}", Pieces(&input, width)), &mut out);
        assert!(!cut);
        assert_eq!(&out[..n], b"safe *** *** ***eX safe");
    }
    let mut out = [0; 8];
    let (n, cut) = secrets.format(format_args!("safe {long} tail"), &mut out);
    assert!(cut);
    assert_eq!(&out[..n], b"safe ***");
}
