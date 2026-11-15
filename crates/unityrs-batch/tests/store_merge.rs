//! Late LIST failure cannot publish partial merge decisions.
#![cfg(target_os = "linux")]
mod store_support;
use std::{fmt::Write, sync::Arc, time::Duration};
use store_support::{Harness, Reply, run};
use unityrs_batch::{
    outcomes::{Archive, Code, Outcomes},
    phase::{Account, Error},
    publish::store::{self, Expected, Source, Target, resume, verify},
    spool::{Counter, Host, workspace::Workspace},
    store::{etag::Etag, put::Request},
};
use unityrs_testkit::scratch::Scratch;

#[test]
fn interrupted_merge_has_no_list_warning_and_idle_reservation_refusal_does() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct StopSource<'a> {
        row: Rows,
        stop: &'a AtomicBool,
    }
    impl Source for StopSource<'_> {
        fn current(&self) -> Result<Option<Expected<'_>>, Error> {
            self.stop.store(true, Ordering::Release);
            self.row.current()
        }
        fn advance(&mut self) -> Result<(), Error> {
            self.row.advance()
        }
    }
    for verifying in [false, true] {
        for stopping in [false, true] {
            let h = Harness::new([page(
                &[("a", if verifying { "wrong" } else { "abc" }, 1)],
                "",
            )]);
            let scratch = Scratch::new("merge-stop-refusal");
            let counter = Counter::new(1 << 24, 0, 4096).unwrap();
            let workspace = Workspace::create(
                scratch.path().join("merge"),
                counter.clone(),
                Arc::new(Host),
            )
            .unwrap();
            let out = Outcomes::create(scratch.path(), 1, 0, &counter, &Host, &[0; 128], &|| false)
                .unwrap();
            let code = if verifying {
                Code::Written
            } else {
                Code::NotWritten
            };
            out.set(0, code).unwrap();
            let stop = AtomicBool::new(false);
            let mut rows = Rows {
                rows: vec![("a", Target::Loose(0), code, b"abc", 1)],
                at: 0,
            };
            let account = Account::new(if stopping { 64 << 20 } else { (40 << 20) - 1 });
            run(async {
                let tries = h.tries();
                let request = Request::new(
                    &h.store,
                    &tries,
                    &h.slot,
                    tokio::time::Instant::now() + Duration::from_secs(5),
                );
                let cx = store::Context {
                    request: &request,
                    account: &account,
                    workspace: &workspace,
                    prefix: "",
                    stopped: &|| stop.load(Ordering::Acquire),
                };
                let mut stopping_rows = StopSource {
                    row: Rows {
                        rows: rows.rows.clone(),
                        at: 0,
                    },
                    stop: &stop,
                };
                let source: &mut dyn Source = if stopping {
                    &mut stopping_rows
                } else {
                    &mut rows
                };
                if verifying {
                    let report = verify::run(&cx, source, &out, 1, 0).await.unwrap();
                    assert_eq!(report.verified, Some(false));
                    assert_eq!(report.verify_list_failed, !stopping);
                } else {
                    let report = resume::run(&cx, source, &out, &mut []).await.unwrap();
                    assert_eq!(report.stopped, stopping);
                    assert_eq!(report.resume_list_failed, !stopping);
                }
            });
            assert_eq!(out.loose(0).unwrap(), code);
            assert_eq!(account.snapshot().input.used, 0);
        }
    }
}

struct Rows {
    rows: Vec<(&'static str, Target, Code, &'static [u8], u64)>,
    at: usize,
}
impl Source for Rows {
    fn current(&self) -> Result<Option<Expected<'_>>, Error> {
        Ok(self
            .rows
            .get(self.at)
            .map(|&(key, target, code, etag, size)| Expected {
                key,
                target,
                code,
                etag,
                size,
            }))
    }
    fn advance(&mut self) -> Result<(), Error> {
        self.at += 1;
        Ok(())
    }
}
fn page(entries: &[(&str, &str, u64)], token: &str) -> Reply {
    let mut body = String::from("<ListBucketResult>");
    write!(body, "<IsTruncated>{}</IsTruncated>", !token.is_empty()).unwrap();
    for (key, etag, size) in entries {
        write!(body,"<Contents><Key>{key}</Key><LastModified>2026-01-01T00:00:00Z</LastModified><ETag>{etag}</ETag><Size>{size}</Size></Contents>").unwrap();
    }
    body.push_str(token);
    body.push_str("</ListBucketResult>");
    Reply::status(200, "", body.as_bytes())
}
#[test]
fn resume_compares_expected_tags_and_keeps_archives_for_a_hash_pass() {
    let h = Harness::new([page(
        &[
            ("a", "&quot;abc&quot;", 3),
            ("b", "opaque", 4),
            ("zip", "&quot;TAG-2&quot;", 9),
        ],
        "",
    )]);
    let scratch = Scratch::new("store-resume");
    let counter = Counter::new(1 << 24, 0, 4096).unwrap();
    let workspace = Workspace::create(
        scratch.path().join("merge"),
        counter.clone(),
        Arc::new(Host),
    )
    .unwrap();
    let out =
        Outcomes::create(scratch.path(), 2, 1, &counter, &Host, &[0; 128], &|| false).unwrap();
    let mut source = Rows {
        rows: vec![
            ("a", Target::Loose(0), Code::NotWritten, b"abc", 3),
            ("b", Target::Loose(1), Code::NotWritten, b"expected", 4),
            ("zip", Target::Archive(0), Code::NotWritten, b"", 9),
        ],
        at: 0,
    };
    let account = Account::new(64 << 20);
    let mut archives = [None];
    run(async {
        let tries = h.tries();
        let request = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
        let cx = store::Context {
            request: &request,
            account: &account,
            workspace: &workspace,
            prefix: "",
            stopped: &|| false,
        };
        let report = resume::run(&cx, &mut source, &out, &mut archives)
            .await
            .unwrap();
        assert_eq!(report.present, 1);
        assert!(!report.resume_list_failed);
    });
    assert_eq!(out.loose(0).unwrap(), Code::Present);
    assert_eq!(out.loose(1).unwrap(), Code::NotWritten);
    assert_eq!(out.archive(0).unwrap().code, Code::NotWritten);
    let listed = archives[0].unwrap();
    assert_eq!(listed.etag.as_bytes(), b"\"TAG-2\"");
    assert!(listed.matches(b"TAG-2", 9));
    assert!(!listed.matches(b"TAG-2", 8));
    assert!(!listed.matches(b"tag-2", 9));
    assert_eq!(account.snapshot().input.used, 0);
    assert_eq!(workspace.claimed(), 0);
}
#[test]
fn failed_or_unordered_late_list_discards_resume_hits_and_verification_mismatches() {
    for last in [
        Reply::error(403, "AccessDenied"),
        page(&[("0", "tag", 1)], ""),
    ] {
        for verifying in [false, true] {
            let h = Harness::new([
                page(
                    &[("a", if verifying { "wrong" } else { "abc" }, 1)],
                    "<NextContinuationToken>next</NextContinuationToken>",
                ),
                last_reply(&last),
            ]);
            let scratch = Scratch::new("store-transaction");
            let counter = Counter::new(1 << 24, 0, 4096).unwrap();
            let workspace = Workspace::create(
                scratch.path().join("merge"),
                counter.clone(),
                Arc::new(Host),
            )
            .unwrap();
            let out = Outcomes::create(scratch.path(), 1, 0, &counter, &Host, &[0; 128], &|| false)
                .unwrap();
            let code = if verifying {
                Code::Written
            } else {
                Code::NotWritten
            };
            out.set(0, code).unwrap();
            let mut source = Rows {
                rows: vec![("a", Target::Loose(0), code, b"abc", 1)],
                at: 0,
            };
            let account = Account::new(64 << 20);
            run(async {
                let tries = h.tries();
                let request = Request::new(
                    &h.store,
                    &tries,
                    &h.slot,
                    tokio::time::Instant::now() + Duration::from_secs(5),
                );
                let cx = store::Context {
                    request: &request,
                    account: &account,
                    workspace: &workspace,
                    prefix: "",
                    stopped: &|| false,
                };
                if verifying {
                    let report = verify::run(&cx, &mut source, &out, 1, 0).await.unwrap();
                    assert_eq!(report.verified, Some(false));
                    assert!(report.verify_list_failed);
                } else {
                    assert!(
                        resume::run(&cx, &mut source, &out, &mut [])
                            .await
                            .unwrap()
                            .resume_list_failed
                    );
                }
            });
            assert_eq!(out.loose(0).unwrap(), code);
            assert_eq!(account.snapshot().input.used, 0);
            assert_eq!(workspace.claimed(), 0);
        }
    }
}
fn last_reply(reply: &Reply) -> Reply {
    match reply {
        Reply::Answer { chunks, end } => Reply::chunks(chunks.clone(), *end),
        _ => unreachable!(),
    }
}
#[test]
fn verification_checks_all_successes_and_uses_returned_tags_for_outcome_seven() {
    let h = Harness::new([page(
        &[
            ("a", "abc", 1),
            ("b", "changed", 9),
            ("d", "changed", 2),
            ("zip", "TAG-2", 9),
        ],
        "",
    )]);
    let scratch = Scratch::new("store-verify");
    let counter = Counter::new(1 << 24, 0, 4096).unwrap();
    let workspace = Workspace::create(
        scratch.path().join("merge"),
        counter.clone(),
        Arc::new(Host),
    )
    .unwrap();
    let out =
        Outcomes::create(scratch.path(), 5, 1, &counter, &Host, &[0; 128], &|| false).unwrap();
    let mut source = Rows {
        rows: vec![
            ("a", Target::Loose(0), Code::Present, b"abc", 1),
            ("b", Target::Loose(1), Code::WrittenBySize, b"changed", 9),
            ("c", Target::Loose(2), Code::Written, b"expected", 1),
            ("d", Target::Loose(3), Code::Written, b"expected", 2),
            ("zip", Target::Archive(0), Code::Written, b"TAG-2", 9),
            ("zzz", Target::Loose(4), Code::Written, b"expected", 1),
        ],
        at: 0,
    };
    for &(.., target, code, _, _) in &source.rows {
        if let Target::Loose(i) = target {
            out.set(i, code).unwrap();
        }
    }
    out.set_archive(
        0,
        &Archive::success(Code::Written, 9, [0; 16], Some("TAG-2")).unwrap(),
    )
    .unwrap();
    let account = Account::new(64 << 20);
    run(async {
        let tries = h.tries();
        let request = Request::new(
            &h.store,
            &tries,
            &h.slot,
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
        let cx = store::Context {
            request: &request,
            account: &account,
            workspace: &workspace,
            prefix: "",
            stopped: &|| false,
        };
        let report = verify::run(&cx, &mut source, &out, 6, 0).await.unwrap();
        assert_eq!(report.verified, Some(false));
        assert_eq!(report.verified_by_size, 1);
        assert_eq!(report.mismatches, 3);
    });
    assert_eq!(out.loose(0).unwrap(), Code::Present);
    assert_eq!(out.loose(1).unwrap(), Code::WrittenBySize);
    assert_eq!(out.loose(2).unwrap(), Code::PublishMismatch);
    assert_eq!(out.loose(3).unwrap(), Code::PublishMismatch);
    assert_eq!(out.loose(4).unwrap(), Code::PublishMismatch);
    assert_eq!(out.archive(0).unwrap().code, Code::Written);
    assert_eq!(account.snapshot().input.used, 0);
    assert_eq!(workspace.claimed(), 0);
    assert!(!Etag::new(b"a").unwrap().matches(b"A"));
}

#[test]
fn verification_null_drain_gate_and_size_mismatch_apply_to_archives_too() {
    for (published, in_flight) in [(0, 0), (1, 1), (1, 0)] {
        let h = Harness::new([page(&[("zip", "0123456789abcdef0123456789abcdef", 8)], "")]);
        let scratch = Scratch::new("verify-size-archive");
        let counter = Counter::new(1 << 24, 0, 4096).unwrap();
        let workspace = Workspace::create(
            scratch.path().join("merge"),
            counter.clone(),
            Arc::new(Host),
        )
        .unwrap();
        let out =
            Outcomes::create(scratch.path(), 0, 1, &counter, &Host, &[0; 128], &|| false).unwrap();
        out.set_archive(
            0,
            &Archive::success(Code::WrittenBySize, 9, [0; 16], Some("different")).unwrap(),
        )
        .unwrap();
        let mut rows = Rows {
            rows: vec![(
                "zip",
                Target::Archive(0),
                Code::WrittenBySize,
                b"different",
                9,
            )],
            at: 0,
        };
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let request = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let cx = store::Context {
                request: &request,
                account: &account,
                workspace: &workspace,
                prefix: "",
                stopped: &|| false,
            };
            let report = verify::run(&cx, &mut rows, &out, published, in_flight).await;
            if in_flight != 0 {
                assert_eq!(report.err(), Some(Error::internal()));
            } else {
                let report = report.unwrap();
                assert_eq!(
                    report.verified,
                    if published == 0 { None } else { Some(false) }
                );
                assert_eq!(report.verified_by_size, 0);
                assert_eq!(report.mismatches, published);
            }
        });
        assert_eq!(
            h.script.seen.count(),
            usize::from(published != 0 && in_flight == 0)
        );
        assert_eq!(
            out.archive(0).unwrap().code,
            if published != 0 && in_flight == 0 {
                Code::PublishMismatch
            } else {
                Code::WrittenBySize
            }
        );
        assert_eq!(account.snapshot().input.used, 0);
    }
}

#[test]
fn dropping_each_merge_at_every_io_yield_never_advertises_partial_results() {
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    async fn cut<F: Future>(at: usize, future: F) -> Option<F::Output> {
        let mut future = Box::pin(future);
        let mut polls = 0;
        poll_fn(|cx| {
            if polls == at {
                return Poll::Ready(None);
            }
            polls += 1;
            future.as_mut().poll(cx).map(Some)
        })
        .await
    }
    for verifying in [false, true] {
        let mut finished = false;
        for at in 0..128 {
            let h = store_support::yielding(vec![
                page(
                    &[("a", if verifying { "wrong" } else { "abc" }, 1)],
                    "<NextContinuationToken>next</NextContinuationToken>",
                ),
                page(&[("b", "bcd", 1)], ""),
            ]);
            let scratch = Scratch::new("merge-drop");
            let counter = Counter::new(1 << 24, 0, 4096).unwrap();
            let workspace = Workspace::create(
                scratch.path().join("merge"),
                counter.clone(),
                Arc::new(Host),
            )
            .unwrap();
            let out = Outcomes::create(scratch.path(), 1, 0, &counter, &Host, &[0; 128], &|| false)
                .unwrap();
            let code = if verifying {
                Code::Written
            } else {
                Code::NotWritten
            };
            out.set(0, code).unwrap();
            let mut rows = Rows {
                rows: vec![("a", Target::Loose(0), code, b"abc", 1)],
                at: 0,
            };
            let account = Account::new(64 << 20);
            let done = run(async {
                let tries = h.tries();
                let request = Request::new(
                    &h.store,
                    &tries,
                    &h.slot,
                    tokio::time::Instant::now() + Duration::from_secs(5),
                );
                let cx = store::Context {
                    request: &request,
                    account: &account,
                    workspace: &workspace,
                    prefix: "",
                    stopped: &|| false,
                };
                cut(at, async {
                    if verifying {
                        verify::run(&cx, &mut rows, &out, 1, 0).await.map(|_| ())
                    } else {
                        resume::run(&cx, &mut rows, &out, &mut []).await.map(|_| ())
                    }
                })
                .await
            });
            let completed = done.is_some();
            if let Some(result) = done {
                result.unwrap();
            }
            assert_eq!(
                out.loose(0).unwrap(),
                if completed {
                    if verifying {
                        Code::PublishMismatch
                    } else {
                        Code::Present
                    }
                } else {
                    code
                },
                "cut {at}"
            );
            assert_eq!(account.snapshot().input.used, 0);
            drop(h.slot.take_permit());
            assert_eq!(h.pool.snapshot().occupied, 0);
            if completed {
                assert_eq!(workspace.claimed(), 0);
                finished = true;
                break;
            }
        }
        assert!(finished);
    }
}
#[test]
fn a_stop_after_replay_preserves_the_counts_of_acknowledged_outcome_writes() {
    for verifying in [false, true] {
        let h = Harness::new([page(
            &[("a", if verifying { "wrong" } else { "abc" }, 1)],
            "",
        )]);
        let scratch = Scratch::new("merge-replay-stop");
        let counter = Counter::new(1 << 24, 0, 4096).unwrap();
        let workspace = Workspace::create(
            scratch.path().join("merge"),
            counter.clone(),
            Arc::new(Host),
        )
        .unwrap();
        let out =
            Outcomes::create(scratch.path(), 1, 0, &counter, &Host, &[0; 128], &|| false).unwrap();
        let initial = if verifying {
            Code::Written
        } else {
            Code::NotWritten
        };
        out.set(0, initial).unwrap();
        let expected = if verifying {
            Code::PublishMismatch
        } else {
            Code::Present
        };
        let mut rows = Rows {
            rows: vec![("a", Target::Loose(0), initial, b"abc", 1)],
            at: 0,
        };
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let request = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let cx = store::Context {
                request: &request,
                account: &account,
                workspace: &workspace,
                prefix: "",
                stopped: &|| out.loose(0).unwrap() == expected,
            };
            if verifying {
                let report = verify::run(&cx, &mut rows, &out, 1, 0).await.unwrap();
                assert_eq!(report.mismatches, 1);
                assert_eq!(report.verified, Some(false));
                assert!(!report.verify_list_failed);
            } else {
                let report = resume::run(&cx, &mut rows, &out, &mut []).await.unwrap();
                assert_eq!(report.present, 1);
                assert!(report.stopped);
                assert!(!report.resume_list_failed);
            }
        });
        assert_eq!(out.loose(0).unwrap(), expected);
        assert_eq!(account.snapshot().input.used, 0);
        assert!(workspace.claimed() > 0); // Stop retains scratch for owned startup cleanup.
    }
}
#[test]
fn a_stop_during_scratch_read_prevents_the_following_outcome_write() {
    use std::{
        io::{self, Read},
        sync::atomic::{AtomicBool, Ordering},
    };
    use unityrs_batch::spool::FileSystem;
    struct Fs(Arc<AtomicBool>);
    struct Reader(Box<dyn Read + Send>, Arc<AtomicBool>);
    impl Read for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let n = self.0.read(bytes)?;
            self.1.store(true, Ordering::Release);
            Ok(n)
        }
    }
    impl FileSystem for Fs {
        fn available(&self, path: &std::path::Path) -> io::Result<u64> {
            Host.available(path)
        }
        fn open(&self, path: &std::path::Path) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(Reader(Host.open(path)?, self.0.clone())))
        }
    }
    for verifying in [false, true] {
        let h = Harness::new([page(
            &[("a", if verifying { "wrong" } else { "abc" }, 1)],
            "",
        )]);
        let scratch = Scratch::new("merge-read-stop");
        let counter = Counter::new(1 << 24, 0, 4096).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let workspace = Workspace::create(
            scratch.path().join("merge"),
            counter.clone(),
            Arc::new(Fs(stopped.clone())),
        )
        .unwrap();
        let out =
            Outcomes::create(scratch.path(), 1, 0, &counter, &Host, &[0; 128], &|| false).unwrap();
        let initial = if verifying {
            Code::Written
        } else {
            Code::NotWritten
        };
        out.set(0, initial).unwrap();
        let mut rows = Rows {
            rows: vec![("a", Target::Loose(0), initial, b"abc", 1)],
            at: 0,
        };
        let account = Account::new(64 << 20);
        run(async {
            let tries = h.tries();
            let request = Request::new(
                &h.store,
                &tries,
                &h.slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            );
            let cx = store::Context {
                request: &request,
                account: &account,
                workspace: &workspace,
                prefix: "",
                stopped: &|| stopped.load(Ordering::Acquire),
            };
            if verifying {
                let report = verify::run(&cx, &mut rows, &out, 1, 0).await.unwrap();
                assert_eq!(report.verified, Some(false));
                assert_eq!(report.mismatches, 0);
                assert!(!report.verify_list_failed);
            } else {
                let report = resume::run(&cx, &mut rows, &out, &mut []).await.unwrap();
                assert!(report.stopped);
                assert_eq!(report.present, 0);
                assert!(!report.resume_list_failed);
            }
        });
        assert_eq!(out.loose(0).unwrap(), initial);
        assert_eq!(account.snapshot().input.used, 0);
    }
}
