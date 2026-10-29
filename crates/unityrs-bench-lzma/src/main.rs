//! P0 spike: decode the LZMA blocks of real bundles with liblzma and
//! lzma-rust2, check both against `UnityPy`, and time them. The method and the
//! results are in `docs/spikes/lzma.md`.
//!
//! ```text
//! unityrs-bench-lzma verify --list FILE --expect ORACLE.tsv
//! unityrs-bench-lzma bench  --list FILE --set NAME [--threads N] [--reps R]
//!                           [--codec NAME,NAME] [--per-block FILE]
//! ```
//!
//! A list holds one bundle path per line. `ORACLE.tsv` is what
//! `unitypy_lzma.py oracle` writes, one row per LZMA block `UnityPy`
//! decompressed: path, block index, then either the length and sha256 of its
//! output, or `error` and the exception it raised on that block.

mod codec;
#[cfg(test)]
mod synth;
mod unityfs;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::hint::black_box;
use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::Scope;
use std::time::{Duration, Instant};

use codec::Codec;
use sha2::{Digest, Sha256};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("verify") => verify(&Args::parse(&args[1..])),
        Some("bench") => bench(&Args::parse(&args[1..])),
        _ => Err("usage: unityrs-bench-lzma verify|bench --list FILE ... (see src/main.rs)".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--name value` pairs; a repeated name keeps the last value.
struct Args(HashMap<String, String>);

impl Args {
    fn parse(args: &[String]) -> Args {
        let pairs = args.chunks(2).filter_map(|pair| match pair {
            [name, value] => Some((name.trim_start_matches("--").to_owned(), value.clone())),
            _ => None,
        });
        Args(pairs.collect())
    }

    fn get(&self, name: &str) -> Result<&str, String> {
        self.0
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("missing --{name}"))
    }

    fn number(&self, name: &str, default: usize) -> Result<usize, String> {
        self.0.get(name).map_or(Ok(default), |v| {
            v.parse().map_err(|e| format!("--{name}: {e}"))
        })
    }
}

/// A bundle read into memory with its LZMA blocks located.
struct Bundle {
    path: String,
    bytes: Vec<u8>,
    blocks: Vec<unityfs::LzmaBlock>,
}

fn load(list: &str) -> Result<Vec<Bundle>, String> {
    let text = std::fs::read_to_string(list).map_err(|e| format!("{list}: {e}"))?;
    let paths = text.lines().map(str::trim).filter(|line| !line.is_empty());
    paths
        .map(|path| {
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            let blocks = unityfs::lzma_blocks(&bytes).map_err(|e| format!("{path}: {e}"))?;
            Ok(Bundle {
                path: path.to_owned(),
                bytes,
                blocks,
            })
        })
        .collect()
}

/// Checks every decoder against the oracle, both ways: every block the finder
/// reports needs exactly one oracle row, and every oracle row needs exactly
/// one reported block. The verdict counts blocks, not failure lines.
fn verify(args: &Args) -> Result<(), String> {
    let bundles = load(args.get("list")?)?;
    let expect_path = args.get("expect")?;
    let expect_text =
        std::fs::read_to_string(expect_path).map_err(|e| format!("{expect_path}: {e}"))?;
    let oracle = parse_oracle(&expect_text).map_err(|e| format!("{expect_path}: {e}"))?;
    let verdict = check(&bundles, &oracle);

    let without = bundles.iter().filter(|b| b.blocks.is_empty()).count();
    let names: Vec<&str> = Codec::ALL.iter().map(|c| c.name()).collect();
    println!(
        "{} bundles ({without} without LZMA blocks), {} LZMA blocks, {} bytes out, \
         {} oracle rows, decoders {names:?}: {} blocks failed",
        bundles.len(),
        verdict.blocks,
        verdict.bytes,
        oracle.len(),
        verdict.failed.len()
    );
    for (&(props, dict), blocks) in &verdict.props {
        println!("properties {props:#04x}, dictionary {dict} bytes: {blocks} blocks");
    }
    for line in &verdict.lines {
        println!("FAIL {line}");
    }
    if verdict.failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} blocks did not match UnityPy",
            verdict.failed.len()
        ))
    }
}

/// What `UnityPy` did with one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect<'a> {
    Output { len: usize, sha256: &'a str },
    Raised(&'a str),
}

type Oracle<'a> = HashMap<(&'a str, usize), Expect<'a>>;

/// Every non-empty line must be a well-formed row, and no block may have two.
fn parse_oracle(text: &str) -> Result<Oracle<'_>, String> {
    let mut rows = Oracle::new();
    let mut bad = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let row = match cols[..] {
            [path, index, "error", exception] if !path.is_empty() && !exception.is_empty() => index
                .parse()
                .ok()
                .map(|i| ((path, i), Expect::Raised(exception))),
            [path, index, len, sha256] if !path.is_empty() && is_sha256(sha256) => {
                match (index.parse(), len.parse()) {
                    (Ok(i), Ok(len)) => Some(((path, i), Expect::Output { len, sha256 })),
                    _ => None,
                }
            }
            _ => None,
        };
        let line_no = n + 1;
        match row {
            None => bad.push(format!("line {line_no} is malformed: {line:?}")),
            Some((key, expect)) => {
                if rows.insert(key, expect).is_some() {
                    bad.push(format!("line {line_no} repeats {}#{}", key.0, key.1));
                }
            }
        }
    }
    if bad.is_empty() {
        Ok(rows)
    } else {
        Err(format!("{} bad oracle rows\n{}", bad.len(), bad.join("\n")))
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The outcome of [`check`].
#[derive(Default)]
struct Verdict {
    lines: Vec<String>,
    /// Each block with at least one failure, once.
    failed: BTreeSet<(String, usize)>,
    /// Blocks the finder reported, and the bytes the blocks-info gives them.
    blocks: usize,
    bytes: usize,
    /// Reported blocks per properties byte and dictionary size.
    props: BTreeMap<(u8, u32), usize>,
}

impl Verdict {
    fn fail(&mut self, path: &str, index: usize, why: &str) {
        self.lines.push(format!("{path}#{index}{why}"));
        self.failed.insert((path.to_owned(), index));
    }
}

fn check(bundles: &[Bundle], oracle: &Oracle) -> Verdict {
    let mut verdict = Verdict::default();
    let mut matched: HashMap<(&str, usize), usize> = HashMap::new();
    for bundle in bundles {
        for block in &bundle.blocks {
            let (path, index) = (bundle.path.as_str(), block.index);
            let input = &bundle.bytes[block.start..block.end];
            verdict.blocks += 1;
            verdict.bytes += block.uncompressed;
            if let (Some(&props), Some(dict)) = (input.first(), input.get(1..5)) {
                let dict = u32::from_le_bytes(dict.try_into().unwrap());
                *verdict.props.entry((props, dict)).or_default() += 1;
            }
            let Some((&key, &expect)) = oracle.get_key_value(&(path, index)) else {
                verdict.fail(path, index, ": no oracle row");
                continue;
            };
            *matched.entry(key).or_default() += 1;
            if let Expect::Output { len, .. } = expect
                && len != block.uncompressed
            {
                let why = format!(
                    ": UnityPy made {len} bytes, blocks-info says {}",
                    block.uncompressed
                );
                verdict.fail(path, index, &why);
            }
            for &codec in Codec::ALL {
                let mut out = vec![0u8; block.uncompressed];
                let decoded = codec.decode(input, &mut out);
                let outcome = match expect {
                    Expect::Output { sha256, .. } => decoded.and_then(|()| {
                        let sha = hex(&Sha256::digest(&out));
                        if sha == sha256 {
                            Ok(())
                        } else {
                            Err(format!("sha256 {sha}, UnityPy {sha256}"))
                        }
                    }),
                    Expect::Raised(exception) => match decoded {
                        Ok(()) => Err(format!("decoded a block UnityPy raised {exception} on")),
                        Err(_) => Ok(()),
                    },
                };
                if let Err(e) = outcome {
                    verdict.fail(path, index, &format!(" {}: {e}", codec.name()));
                }
            }
        }
    }

    let listed: HashSet<&str> = bundles.iter().map(|b| b.path.as_str()).collect();
    let mut keys: Vec<&(&str, usize)> = oracle.keys().collect();
    keys.sort_unstable();
    for &(path, index) in keys {
        let why = match matched.get(&(path, index)).copied().unwrap_or(0) {
            1 => continue,
            0 if listed.contains(path) => {
                ": UnityPy decompressed a block the finder did not report".to_owned()
            }
            0 => ": oracle row for a bundle not in the list".to_owned(),
            n => format!(": oracle row matched by {n} reported blocks"),
        };
        verdict.fail(path, index, &why);
    }
    verdict
}

/// One block to decode, borrowed from its bundle.
struct Job<'a> {
    path: &'a str,
    index: usize,
    input: &'a [u8],
    size: usize,
}

/// Times each decoder over the set. Reps alternate between decoders so that a
/// slow spell on a shared box lands on all of them. Writes one TSV row per
/// decoder and rep to stdout: impl, set, threads, rep, blocks, bytes out,
/// seconds. With `--threads 1` and `--per-block FILE`, also appends one row
/// per block to FILE.
fn bench(args: &Args) -> Result<(), String> {
    let bundles = load(args.get("list")?)?;
    let set = args.get("set")?;
    let threads = args.number("threads", 1)?.max(1);
    let reps = args.number("reps", 5)?;
    let codecs: Vec<Codec> = match args.0.get("codec") {
        Some(names) => {
            let names: Vec<&str> = names.split(',').collect();
            Codec::ALL
                .iter()
                .copied()
                .filter(|c| names.contains(&c.name()))
                .collect()
        }
        None => Codec::ALL.to_vec(),
    };
    if codecs.is_empty() {
        return Err("no decoder in this build matches --codec".into());
    }
    // Appended to, so that several runs can collect into one file.
    let mut per_block = match args.0.get("per-block") {
        Some(path) => Some(
            std::fs::File::options()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| format!("{path}: {e}"))?,
        ),
        None => None,
    };

    // Largest first, so that under threads the biggest block starts at once
    // and cannot be left to run alone at the end.
    let mut jobs: Vec<Job> = bundles
        .iter()
        .flat_map(|bundle| {
            bundle.blocks.iter().map(|block| Job {
                path: &bundle.path,
                index: block.index,
                input: &bundle.bytes[block.start..block.end],
                size: block.uncompressed,
            })
        })
        .collect();
    jobs.sort_by_key(|job| std::cmp::Reverse(job.size));
    let total: usize = jobs.iter().map(|job| job.size).sum();

    let work = |codec: Codec, i: usize| run(codec, &jobs[i]);
    std::thread::scope(|scope| {
        // Started and warmed before any clock, as the Python side's process
        // pool is, and reused by every rep.
        let pool = (threads > 1).then(|| Pool::new(scope, threads, jobs.len(), &work));
        for &codec in &codecs {
            if let Some(job) = jobs.last() {
                run(codec, job)?; // warm up
            }
            if let Some(pool) = &pool {
                pool.warm(codec)?;
            }
        }
        let mut stdout = std::io::stdout().lock();
        for rep in 0..reps {
            for &codec in &codecs {
                let (elapsed, times) = match &pool {
                    None => serial(codec, &jobs)?,
                    Some(pool) => (pool.run(codec)?, Vec::new()),
                };
                let name = codec.name();
                let row = format!(
                    "{name}\t{set}\t{threads}\t{rep}\t{}\t{total}\t{:.6}",
                    jobs.len(),
                    elapsed.as_secs_f64()
                );
                writeln!(stdout, "{row}").map_err(|e| e.to_string())?;
                if let Some(file) = per_block.as_mut() {
                    let mut rows = String::new();
                    for (job, time) in jobs.iter().zip(&times) {
                        let (path, index, size) = (job.path, job.index, job.size);
                        let secs = time.as_secs_f64();
                        let _ = writeln!(
                            rows,
                            "{name}\t{set}\t{rep}\t{path}\t{index}\t{size}\t{secs:.6}"
                        );
                    }
                    file.write_all(rows.as_bytes()).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    })
}

/// Decodes one block into a fresh buffer, as a reader would per bundle.
fn run(codec: Codec, job: &Job) -> Result<(), String> {
    let mut out = vec![0u8; job.size];
    codec
        .decode(job.input, &mut out)
        .map_err(|e| format!("{}#{} {}: {e}", job.path, job.index, codec.name()))?;
    black_box(out);
    Ok(())
}

fn serial(codec: Codec, jobs: &[Job]) -> Result<(Duration, Vec<Duration>), String> {
    let mut times = Vec::with_capacity(jobs.len());
    for job in jobs {
        let start = Instant::now();
        run(codec, job)?;
        times.push(start.elapsed());
    }
    Ok((times.iter().sum(), times))
}

/// Worker threads that live across reps, so that an 8-way time holds no
/// thread start-up. Each run hands every worker the same task, and the
/// workers call `work(task, i)` for every `i` in `0..jobs` once between them,
/// taking each `i` off a shared counter.
struct Pool<T> {
    tasks: Vec<mpsc::Sender<T>>,
    done: mpsc::Receiver<Result<(), String>>,
    next: Arc<AtomicUsize>,
    jobs: usize,
}

impl<T: Copy + Send> Pool<T> {
    fn new<'scope, F>(
        scope: &'scope Scope<'scope, '_>,
        threads: usize,
        jobs: usize,
        work: &'scope F,
    ) -> Pool<T>
    where
        T: 'scope,
        F: Fn(T, usize) -> Result<(), String> + Sync,
    {
        let next = Arc::new(AtomicUsize::new(jobs));
        let (done_tx, done) = mpsc::channel();
        let tasks = (0..threads)
            .map(|_| {
                let (task_tx, task_rx) = mpsc::channel::<T>();
                let (done_tx, next) = (done_tx.clone(), Arc::clone(&next));
                scope.spawn(move || {
                    for task in task_rx {
                        let take_jobs = || loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            if i >= jobs {
                                return Ok(());
                            }
                            work(task, i)?;
                        };
                        let result = catch_unwind(AssertUnwindSafe(take_jobs))
                            .unwrap_or_else(|_| Err("a worker panicked".into()));
                        if done_tx.send(result).is_err() {
                            return;
                        }
                    }
                });
                task_tx
            })
            .collect();
        Pool {
            tasks,
            done,
            next,
            jobs,
        }
    }

    /// Wall time for the workers to take every job once.
    fn run(&self, task: T) -> Result<Duration, String> {
        self.next.store(0, Ordering::Relaxed);
        let start = Instant::now();
        self.round(task)?;
        Ok(start.elapsed())
    }

    /// One round with no job left to take, so that every worker has started
    /// and is waiting before the first run.
    fn warm(&self, task: T) -> Result<(), String> {
        self.next.store(self.jobs, Ordering::Relaxed);
        self.round(task)
    }

    fn round(&self, task: T) -> Result<(), String> {
        for tasks in &self.tasks {
            tasks.send(task).map_err(|_| "a worker stopped")?;
        }
        let mut result = Ok(());
        for _ in &self.tasks {
            let done = self.done.recv().map_err(|_| "a worker stopped")?;
            result = result.and(done);
        }
        result
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Synthetic bundles in the corpus layout, so that the parser, every decoder
/// and the oracle check are tested without game data.
#[cfg(all(test, feature = "liblzma"))]
mod tests {
    use super::{Bundle, Codec, Pool, check, hex, parse_oracle, unityfs};
    use crate::synth::{Header, bundle, lzma_block, sample};
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::time::Duration;

    /// The corpus layout: format 8, flags 0x240, one LZMA block (0x41).
    fn corpus_bundle(path: &str, data: &[u8]) -> Bundle {
        let header = Header {
            format: 8,
            engine: "6000.3.17f1",
            flags: 0x240,
            align: true,
            pad: true,
        };
        let block = lzma_block(data, 0x5D);
        let (bytes, _) = bundle(&header, &[(&block, data.len(), 0x41)]);
        let blocks = unityfs::lzma_blocks(&bytes).unwrap();
        Bundle {
            path: path.to_owned(),
            bytes,
            blocks,
        }
    }

    fn row(path: &str, data: &[u8]) -> String {
        let sha = hex(&Sha256::digest(data));
        format!("{path}\t0\t{}\t{sha}\n", data.len())
    }

    #[test]
    fn every_decoder_returns_the_source() {
        // Longer than the dictionary, so that it wraps.
        let data = sample(3 << 18);
        let found = corpus_bundle("a", &data);
        assert_eq!(found.blocks.len(), 1);
        let block = found.blocks[0];
        assert_eq!(block.uncompressed, data.len());
        for &codec in Codec::ALL {
            let mut out = vec![0u8; data.len()];
            codec
                .decode(&found.bytes[block.start..block.end], &mut out)
                .unwrap();
            assert!(out == data, "{} decoded other bytes", codec.name());
        }
    }

    #[test]
    fn a_matching_oracle_passes() {
        let data = sample(50_000);
        let text = row("a", &data);
        let verdict = check(&[corpus_bundle("a", &data)], &parse_oracle(&text).unwrap());
        assert!(verdict.failed.is_empty(), "{:?}", verdict.lines);
        assert_eq!((verdict.blocks, verdict.bytes), (1, data.len()));
        let dict = crate::synth::DICT;
        assert_eq!(
            verdict.props.into_iter().collect::<Vec<_>>(),
            [((0x5D, dict), 1)]
        );
    }

    /// A finder that drops a block must not pass.
    #[test]
    fn an_oracle_row_with_no_reported_block_fails() {
        let data = sample(50_000);
        let mut dropped = corpus_bundle("a", &data);
        dropped.blocks.clear();
        let text = row("a", &data);
        let verdict = check(&[dropped], &parse_oracle(&text).unwrap());
        assert_eq!(verdict.failed.len(), 1, "{:?}", verdict.lines);
        assert!(
            verdict.lines[0].contains("did not report"),
            "{:?}",
            verdict.lines
        );
    }

    #[test]
    fn extra_and_repeated_rows_fail() {
        let data = sample(50_000);
        let extra = format!("{}{}", row("a", &data), row("b", &data));
        let verdict = check(&[corpus_bundle("a", &data)], &parse_oracle(&extra).unwrap());
        assert_eq!(verdict.failed.len(), 1, "{:?}", verdict.lines);
        assert!(verdict.lines[0].starts_with("b#0"), "{:?}", verdict.lines);
        // The same bundle listed twice reports each block twice.
        let twice = [corpus_bundle("a", &data), corpus_bundle("a", &data)];
        let verdict = check(&twice, &parse_oracle(&row("a", &data)).unwrap());
        assert_eq!(verdict.failed.len(), 1, "{:?}", verdict.lines);
        assert!(
            verdict.lines[0].contains("matched by 2"),
            "{:?}",
            verdict.lines
        );
        let repeated = format!("{0}{0}", row("a", &data));
        let e = parse_oracle(&repeated).unwrap_err();
        assert!(e.contains("line 2 repeats a#0"), "{e}");
    }

    #[test]
    fn malformed_rows_fail() {
        let sha = "0".repeat(64);
        for line in [
            format!("a\t0\t10\t{sha}\textra"),
            "a\t0\t10".to_owned(),
            format!("a\tx\t10\t{sha}"),
            format!("a\t0\tten\t{sha}"),
            format!("a\t0\t10\t{}", &sha[1..]),
            format!("a\t0\t10\t{}", sha.to_uppercase().replace('0', "A")),
            format!("\t0\t10\t{sha}"),
            "a\t0\terror\t".to_owned(),
        ] {
            let text = format!("b\t0\t10\t{sha}\n{line}\n");
            let e = parse_oracle(&text).unwrap_err();
            assert!(e.contains("line 2 is malformed"), "{line:?}: {e}");
        }
    }

    /// One wrong block is one failed block, however many lines it prints.
    #[test]
    fn the_verdict_counts_blocks_not_lines() {
        let data = sample(50_000);
        let wrong = format!("a\t0\t{}\t{}\n", data.len() + 1, "0".repeat(64));
        let verdict = check(&[corpus_bundle("a", &data)], &parse_oracle(&wrong).unwrap());
        assert_eq!(
            verdict.lines.len(),
            1 + Codec::ALL.len(),
            "{:?}",
            verdict.lines
        );
        assert_eq!(verdict.failed.len(), 1);
    }

    #[test]
    fn a_block_unitypy_raised_on_must_fail_every_decoder() {
        let data = sample(50_000);
        let text = "a\t0\terror\tLZMAError\n";
        let verdict = check(&[corpus_bundle("a", &data)], &parse_oracle(text).unwrap());
        assert_eq!(verdict.failed.len(), 1);
        assert_eq!(verdict.lines.len(), Codec::ALL.len(), "{:?}", verdict.lines);
        let mut rejected = corpus_bundle("a", &data);
        let start = rejected.blocks[0].start;
        rejected.bytes[start] = 0x5F; // lc 5 + lp 0: every decoder refuses
        let verdict = check(&[rejected], &parse_oracle(text).unwrap());
        assert!(verdict.failed.is_empty(), "{:?}", verdict.lines);
    }

    /// Every job once per run, on the same threads from run to run. Each job
    /// sleeps, so that a pool that started new threads per run would show
    /// more threads than it has workers.
    #[test]
    fn the_pool_reuses_its_threads() {
        let seen = Mutex::new(Vec::new());
        let work = |rep: usize, i: usize| {
            std::thread::sleep(Duration::from_millis(1));
            seen.lock()
                .unwrap()
                .push((rep, i, std::thread::current().id()));
            Ok(())
        };
        std::thread::scope(|scope| {
            let pool = Pool::new(scope, 4, 60, &work);
            pool.warm(usize::MAX).unwrap();
            for rep in 0..3 {
                pool.run(rep).unwrap();
            }
        });
        let seen = seen.into_inner().unwrap();
        for rep in 0..3 {
            let mut jobs: Vec<usize> = seen.iter().filter(|s| s.0 == rep).map(|s| s.1).collect();
            jobs.sort_unstable();
            assert_eq!(jobs, (0..60).collect::<Vec<_>>(), "rep {rep}");
        }
        assert_eq!(seen.len(), 180, "the warm round took a job");
        let threads: HashSet<_> = seen.iter().map(|s| s.2).collect();
        assert!(
            threads.len() <= 4,
            "{} threads ran 3 runs of 4 workers",
            threads.len()
        );
    }

    #[test]
    fn a_failing_job_fails_the_run_and_the_pool_carries_on() {
        let work = |fail: bool, i: usize| {
            if fail && i == 7 {
                Err("job 7".to_owned())
            } else {
                Ok(())
            }
        };
        std::thread::scope(|scope| {
            let pool = Pool::new(scope, 3, 20, &work);
            assert_eq!(pool.run(true).unwrap_err(), "job 7");
            pool.run(false).unwrap();
        });
    }
}
