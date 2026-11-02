//! A refusal of the dependency pool the caller's code returns (a fetcher's,
//! a block codec's, a decryptor's, a resolver's `find_file`'s) is retried
//! as the core's own are, once,
//! whatever the code did before it returned (plan section 3.13; review
//! round 10: a fetcher that read through the Environment, let go of the
//! refusal its read met twice and then refused the pool itself had that
//! refusal passed on untried, its read's mark matching it, and a codec's or
//! a decryptor's own refusal was never counted at all; review round 12: a
//! resolver's was retried by a dereference alone, and a fetcher's in a
//! resolver load listed as no dependency was not). Each piece of code here
//! does just that on the run that refuses (its first, or a reopen's fetch):
//! a `load_dependency` of a fetcher that always refuses, which retries and
//! fails, then a refusal of its own; on its next run it serves. Each call
//! must answer as it does when the code never refuses, its code run once
//! more.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use unityrs_core::budget::{BudgetExceeded, DecodeBuf, Reservation};
use unityrs_core::codec::{BlockCodec, BlockDecryptor, Codec, Codecs, Decoded, Stored};
use unityrs_core::env::{CabResolver, EnvOptions, Environment, Fetch, LoadMode, PPtr, Source};
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::Meter;
use unityrs_core::value::Value;
use unityrs_core::{Bytes, Error, FileContext, RawStr};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

use crate::{named_bundle, serialized_cab, src};

/// The pool's refusal as the caller's code makes it, moving no count of
/// the core's.
fn own_refusal() -> Error {
    Error::Budget(BudgetExceeded {
        requested: 64,
        what: Reservation::Dependency,
    })
}

/// A fetcher that always refuses the pool.
#[derive(Debug)]
struct Refuses;

impl Fetch for Refuses {
    fn fetch(&self) -> Result<Bytes, Error> {
        Err(own_refusal())
    }
}

/// The caller's code under test: its runs, the run that refuses, and the
/// Environment that run reads through (taken out at the end, which breaks
/// the cycle the Environment's options or registry make with it).
#[derive(Debug, Default)]
struct Code {
    runs: AtomicU64,
    /// Which run refuses (from 0; the first by default, none at `u64::MAX`).
    refuse_at: u64,
    env: Mutex<Option<Environment>>,
}

impl Code {
    /// Code whose run `n` refuses, and no other.
    fn refusing_at(n: u64) -> Self {
        Self {
            refuse_at: n,
            ..Self::default()
        }
    }

    /// One run: on the refusing one, a read through the Environment that
    /// its retry meets twice and fails, let go, then the code's own
    /// refusal; `Ok` on every other.
    fn run(&self) -> Result<(), Error> {
        if self.runs.fetch_add(1, Ordering::AcqRel) != self.refuse_at {
            return Ok(());
        }
        let env = self.env.lock().unwrap().clone().unwrap();
        let read = env.load_dependency(Source::Fetch {
            name: RawStr::from("refuses.bundle"),
            fetch: Arc::new(Refuses),
        });
        assert!(matches!(read, Err(Error::Budget(_))), "{read:?}");
        Err(own_refusal())
    }

    fn runs(&self) -> u64 {
        self.runs.load(Ordering::Acquire)
    }

    fn set(&self, env: &Environment) {
        *self.env.lock().unwrap() = Some(env.clone());
    }

    fn clear(&self) {
        self.env.lock().unwrap().take();
    }
}

/// A fetcher of `data` whose fetch is [`Code::run`] first.
#[derive(Debug)]
struct CodeFetch(Arc<Code>, Bytes);

impl Fetch for CodeFetch {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.0.run().map(|()| self.1.clone())
    }
}

/// A stored codec whose decode is [`Code::run`] first.
struct CodeCodec(Arc<Code>);

impl BlockCodec for CodeCodec {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        self.0.run()?;
        Stored.decode(input, uncompressed, out)
    }
}

/// A decryptor (exclusive-or with `KEY`) whose decryption is [`Code::run`]
/// first.
struct CodeDecryptor(Arc<Code>);

const KEY: u8 = 0x5A;

impl BlockDecryptor for CodeDecryptor {
    fn decrypt(&self, block: &mut [u8], _index: usize) -> Result<(), Error> {
        self.0.run()?;
        for b in block {
            *b ^= KEY;
        }
        Ok(())
    }
}

/// A resolver whose `find_file` is [`Code::run`] first, then `source`.
#[derive(Debug)]
struct CodeResolver(Arc<Code>, Source);

impl CabResolver for CodeResolver {
    fn resolve(&self, _: &[u8]) -> Option<Source> {
        Some(self.1.clone())
    }

    fn find_file(&self, _: &[u8]) -> Result<Option<Source>, Error> {
        self.0.run().map(|()| Some(self.1.clone()))
    }
}

/// `batch()`'s options with `ctx`, in `mode`.
fn env_with(ctx: FileContext, mode: LoadMode) -> Environment {
    let mut o = EnvOptions::batch(Arc::new(ctx));
    o.load = mode;
    Environment::new(o)
}

/// `len` bytes of a repeating pattern.
fn pattern(len: usize) -> Vec<u8> {
    (0..=250_u8).cycle().take(len).collect()
}

/// A bundle of one raw entry of `len` bytes, in one block stored under the
/// custom codec id 5.
fn custom_codec_bundle(len: usize) -> Vec<u8> {
    build(&BundleSpec {
        info_codec: TestCodec::Lz4,
        data_codec: TestCodec::Raw(5),
        entries: vec![EntrySpec {
            name: b"payload".to_vec(),
            data: pattern(len),
            flags: 0,
        }],
        ..BundleSpec::default()
    })
}

/// An encrypted bundle (its header's flag set) of one raw entry of `len`
/// bytes in one LZ4 block flagged `0x100`, each stored byte exclusive-or
/// `KEY` (`tests/files_memory.rs` builds the same).
fn encrypted_bundle(len: usize) -> Vec<u8> {
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 1 << 20,
        entries: vec![EntrySpec {
            name: b"secret".to_vec(),
            data: pattern(len),
            flags: 0,
        }],
        encrypted: true,
        ..BundleSpec::default()
    });
    let nul = |from: usize| from + data[from..].iter().position(|c| *c == 0).unwrap() + 1;
    // After the header and the decryptor's 70 bytes, aligned: the hash and
    // the count, then the one block's row.
    let row = (nul(nul(12)) + 20 + 70).next_multiple_of(16) + 20;
    let stored = usize::try_from(u32::from_be_bytes(
        data[row + 4..row + 8].try_into().unwrap(),
    ))
    .unwrap();
    let flags = u16::from_be_bytes([data[row + 8], data[row + 9]]) | 0x100;
    data[row + 8..row + 10].copy_from_slice(&flags.to_be_bytes());
    let len = data.len();
    for b in &mut data[len - stored..] {
        *b ^= KEY;
    }
    data
}

/// `load_dependency` of `source` on `env`, `code` reading through it.
fn load(env: &Environment, code: &Code, source: Source) -> Result<(), Error> {
    code.set(env);
    let r = env.load_dependency(source).map(drop);
    code.clear();
    r
}

#[test]
fn a_fetchers_own_refusal_after_a_read_it_let_go_is_retried() {
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let code = Arc::new(Code::default());
        let env = env_with(FileContext::unitypy(), mode);
        let data = Bytes::from(named_bundle("CAB-F", 100));
        let source = Source::Fetch {
            name: RawStr::from("f.bundle"),
            fetch: Arc::new(CodeFetch(code.clone(), data)),
        };
        let r = load(&env, &code, source);
        assert!(r.is_ok(), "{mode:?}: {r:?}");
        assert_eq!(code.runs(), 2, "{mode:?}: the fetcher's runs");
    }
}

#[test]
fn a_codecs_own_refusal_after_a_read_it_let_go_is_retried() {
    let code = Arc::new(Code::default());
    let mut codecs = Codecs::unitypy();
    codecs
        .set(Codec::Other(5), Arc::new(CodeCodec(code.clone())))
        .unwrap();
    let mut ctx = FileContext::unitypy();
    ctx.codecs = Arc::new(codecs);
    // Eager: the load decodes the block.
    let env = env_with(ctx, LoadMode::Eager);
    let r = load(
        &env,
        &code,
        crate::src("c.bundle", &custom_codec_bundle(300)),
    );
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(code.runs(), 2, "the codec's runs");
}

#[test]
fn a_decryptors_own_refusal_after_a_read_it_let_go_is_retried() {
    let code = Arc::new(Code::default());
    let mut ctx = FileContext::unitypy();
    ctx.decryptor = Some(Arc::new(CodeDecryptor(code.clone())));
    // Eager: the load decrypts and decodes the block.
    let env = env_with(ctx, LoadMode::Eager);
    let r = load(&env, &code, crate::src("d.bundle", &encrypted_bundle(300)));
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(code.runs(), 2, "the decryptor's runs");
}

/// A dereference runs the resolver's `find_file` inside its retry (review
/// round 11 added the call, which returns the caller's error): a refusal of
/// its own, after a read it let go, is counted as it enters the core and
/// retried, so the pointer resolves on the resolver's second run.
#[test]
fn a_resolvers_own_refusal_after_a_read_it_let_go_is_retried() {
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let code = Arc::new(Code::default());
        let target = serialized(&[], &[Obj::named(5, "x5", &[])]);
        let resolver = CodeResolver(code.clone(), src("cab-x", &target));
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        o.resolver = Some(Arc::new(resolver));
        let env = Environment::new(o);
        let from = serialized(&["archive:/CAB-X/CAB-X"], &[Obj::named(1, "a1", &[(1, 5)])]);
        env.load(src("CAB-A", &from)).unwrap();
        let a = serialized_cab(&env, "cab-a");
        code.set(&env);
        let got = env.deref(
            &a,
            PPtr {
                file_id: 1,
                path_id: 5,
            },
        );
        code.clear();
        assert_eq!(got.map(|o| o.path_id()).ok(), Some(5), "{mode:?}");
        assert_eq!(code.runs(), 2, "{mode:?}: the resolver's runs");
    }
}

/// Whose code refuses in [`every_load_through_the_resolver_retries_the_callers_own_refusal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refuser {
    /// The resolver's `find_file`.
    Resolver,
    /// The fetcher of the source the resolver returns.
    Fetcher,
}

/// A public call that loads the file `cab-x` through the resolver, or, for
/// [`Call::Reopen`], opens it again from its fetcher after an eviction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    /// `find_file("cab-x", is_dependency)`.
    FindFile(bool),
    /// A dereference of a pointer to `cab-x`'s object.
    Deref,
    /// The container pass, whose preload dereferences such a pointer, twice.
    Container,
    /// A resource read whose fallback loads the bundle an external names.
    ResourceExternal,
    /// A resource read whose fallback loads a candidate, a raw file.
    ResourceCandidate,
    /// `get_cab("cab-x")` of a dependency loaded through a fetcher, then
    /// evicted, whose reopen fetches it again.
    Reopen,
}

const CALLS_THROUGH_THE_RESOLVER: [Call; 7] = [
    Call::FindFile(true),
    Call::FindFile(false),
    Call::Deref,
    Call::Container,
    Call::ResourceExternal,
    Call::ResourceCandidate,
    Call::Reopen,
];

/// The resolver of [`every_load_through_the_resolver_retries_the_callers_own_refusal`]:
/// the files it serves, by the exact name asked, each a source of that
/// name; with [`Refuser::Resolver`] its `find_file` runs the code first,
/// with [`Refuser::Fetcher`] each source is a fetch that runs the code.
#[derive(Debug)]
struct Serves {
    code: Arc<Code>,
    refuser: Refuser,
    files: Vec<(&'static str, Bytes)>,
}

impl Serves {
    fn source(&self, name: &str, data: &Bytes) -> Source {
        match self.refuser {
            Refuser::Resolver => src(name, data),
            Refuser::Fetcher => Source::Fetch {
                name: RawStr::from(name),
                fetch: Arc::new(CodeFetch(self.code.clone(), data.clone())),
            },
        }
    }
}

impl CabResolver for Serves {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        let (n, data) = self.files.iter().find(|(n, _)| n.as_bytes() == name)?;
        Some(self.source(n, data))
    }

    fn find_file(&self, name: &[u8]) -> Result<Option<Source>, Error> {
        if self.refuser == Refuser::Resolver {
            self.code.run()?;
        }
        Ok(self.resolve(name))
    }
}

/// `cab-x`: object 5, `x5`.
fn cab_x() -> Vec<u8> {
    serialized(&[], &[Obj::named(5, "x5", &[])])
}

/// What `call` returns on an Environment whose resolver's `refuser` runs
/// `code`, with `keep` (`EnvOptions::unitypy()`'s options) or without
/// (`batch()`'s), in `mode`: the call's answer, written so that two runs
/// compare.
fn through_the_resolver(
    call: Call,
    refuser: Refuser,
    keep: bool,
    mode: LoadMode,
    code: &Arc<Code>,
) -> String {
    let mut o = if keep {
        EnvOptions::unitypy()
    } else {
        EnvOptions::batch(Arc::new(FileContext::unitypy()))
    };
    o.load = mode;
    if call == Call::Reopen {
        // A cache that evicts each dependency as it loads.
        o.dep_cache_bytes = 1;
    }
    let x_bundle = Bytes::from(bundle(&[("CAB-X", cab_x()), ("x.resS", res_bytes(16, 3))]));
    o.resolver = Some(Arc::new(Serves {
        code: code.clone(),
        refuser,
        files: vec![
            ("cab-x", Bytes::from(cab_x())),
            ("archive:/CAB-X/CAB-X", x_bundle),
            ("y.resS", Bytes::from(res_bytes(16, 4))),
        ],
    }));
    let env = Environment::new(o);
    let to_x = PPtr {
        file_id: 1,
        path_id: 5,
    };
    let caller = |name: &str, externals: &[&str], obj: Obj| {
        env.load(src(name, &serialized(externals, &[obj]))).unwrap();
        serialized_cab(&env, &name.to_lowercase())
    };
    let ask = || {
        let got = env.find_file(&RawStr::from("cab-x"), true, &Meter::unlimited());
        format!("{:?}", got.map(|f| f.map(|f| f.file_type())))
    };
    let read = |from: &Arc<SerializedFile>| {
        let o = from.object(1).unwrap();
        let p = o.parse_as_dict(true).unwrap();
        let Some(Value::Str(path)) = p.get(&[b"m_StreamData", b"path"]) else {
            panic!("no path")
        };
        format!("{:?}", env.resource(from, path, 2, 4))
    };
    code.set(&env);
    let got = match call {
        Call::FindFile(is_dependency) => {
            let got = env.find_file(&RawStr::from("cab-x"), is_dependency, &Meter::unlimited());
            format!("{:?}", got.map(|f| f.map(|f| f.file_type())))
        }
        Call::Deref => {
            let a = caller(
                "CAB-A",
                &["archive:/CAB-X/CAB-X"],
                Obj::named(1, "a1", &[(1, 5)]),
            );
            format!("{:?}", env.deref(&a, to_x).map(|t| t.path_id()))
        }
        Call::Container => {
            let rows = vec![unityrs_testkit::env::row("assets/x.asset", (0, 1), (1, 5))];
            let ab = unityrs_testkit::env::Bundle {
                preload: vec![(1, 5)],
                rows,
                ..unityrs_testkit::env::Bundle::default()
            };
            caller("CAB-B", &["archive:/CAB-X/CAB-X"], Obj::asset_bundle(1, ab));
            let pass = || match env.container() {
                Ok(items) => format!("{:?}", items.map(|i| i.map(|_| ())).collect::<Vec<_>>()),
                Err(e) => format!("{e}"),
            };
            format!("{} / {}", pass(), pass())
        }
        Call::ResourceExternal => read(&caller(
            "CAB-R",
            &["archive:/CAB-X/CAB-X"],
            Obj::streamed(1, "t1", "archive:/CAB-X/x.resS", 2, 4),
        )),
        Call::ResourceCandidate => read(&caller(
            "CAB-C",
            &[],
            Obj::streamed(1, "t1", "archive:/CAB-C/y.resS", 2, 4),
        )),
        Call::Reopen => {
            let source = Serves {
                code: code.clone(),
                refuser,
                files: Vec::new(),
            }
            .source("cab-x", &Bytes::from(cab_x()));
            drop(env.load_dependency(source).unwrap());
            // The next dependency's load evicts `cab-x`, which nothing holds.
            drop(env.load_dependency(src("o.bundle", &named_bundle("CAB-O", 16))));
            assert_eq!(env.stats().dependencies_evicted, 1, "{call:?}");
            ask()
        }
    };
    code.clear();
    got
}

/// Every public call that loads through the resolver, or opens a
/// dependency again from its fetcher, retries a refusal of the dependency
/// pool that the caller's code returns, after a read it let go, once, as it
/// retries its own (plan section 3.13): the resolver's `find_file`
/// (`Environment::find_file` under either flag, a dereference, the
/// container pass's preload) and the fetcher of the source it returns (each
/// of those, and a resource read's fallback through an external and
/// through a candidate), under `EnvOptions::unitypy()`'s options and
/// `batch()`'s, in both modes, and a fetcher's reopen of an evicted
/// dependency (`get_cab`). Each answers as it does when the code never
/// refuses, the code run once more. Review round 12: `find_file` returned
/// the resolver's refusal unretried, which `deref` retried; and a load
/// `keep_resolver_loads` lists as no dependency (`find_file` with
/// `is_dependency` false, a resource fallback through an external)
/// returned its fetcher's, which a fallback through a candidate retried.
#[test]
fn every_load_through_the_resolver_retries_the_callers_own_refusal() {
    let mut failures = Vec::new();
    for call in CALLS_THROUGH_THE_RESOLVER {
        let resource = matches!(call, Call::ResourceExternal | Call::ResourceCandidate);
        for refuser in [Refuser::Resolver, Refuser::Fetcher] {
            // A resource fallback asks `resolve`, which cannot refuse, and a
            // reopen asks the source alone.
            if refuser == Refuser::Resolver && (resource || call == Call::Reopen) {
                continue;
            }
            for keep in [true, false] {
                // `unitypy()`'s options evict nothing, so nothing reopens.
                if keep && call == Call::Reopen {
                    continue;
                }
                for mode in [LoadMode::Eager, LoadMode::Lazy] {
                    let what = format!("{call:?} {refuser:?} keep {keep} {mode:?}");
                    let never = Arc::new(Code::refusing_at(u64::MAX));
                    let want = through_the_resolver(call, refuser, keep, mode, &never);
                    // A reopen's fetch is the fetcher's second run.
                    let at = u64::from(call == Call::Reopen);
                    let code = Arc::new(Code::refusing_at(at));
                    let got = through_the_resolver(call, refuser, keep, mode, &code);
                    if got != want || code.runs() != never.runs() + 1 {
                        failures.push(format!(
                            "{what}: {got} (runs {}), never refused: {want} (runs {})",
                            code.runs(),
                            never.runs()
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
