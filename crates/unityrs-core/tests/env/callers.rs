//! A refusal of the dependency pool the caller's code returns (a fetcher's,
//! a block codec's, a decryptor's) is retried as the core's own are, once,
//! whatever the code did before it returned (plan section 3.13; review
//! round 10: a fetcher that read through the Environment, let go of the
//! refusal its read met twice and then refused the pool itself had that
//! refusal passed on untried, its read's mark matching it, and a codec's or
//! a decryptor's own refusal was never counted at all). Each piece of code
//! here does just that on its first run: a `load_dependency` of a fetcher
//! that always refuses, which retries and fails, then a refusal of its own;
//! on its second run it serves. Each load must succeed, its code run twice.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use unityrs_core::budget::{BudgetExceeded, DecodeBuf, Reservation};
use unityrs_core::codec::{BlockCodec, BlockDecryptor, Codec, Codecs, Decoded, Stored};
use unityrs_core::env::{EnvOptions, Environment, Fetch, LoadMode, Source};
use unityrs_core::{Bytes, Error, FileContext, RawStr};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};

use crate::named_bundle;

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

/// The caller's code under test: its runs, and the Environment its first
/// run reads through (taken out at the end, which breaks the cycle the
/// Environment's options or registry make with it).
#[derive(Debug, Default)]
struct Code {
    runs: AtomicU64,
    env: Mutex<Option<Environment>>,
}

impl Code {
    /// One run: on the first, a read through the Environment that its
    /// retry meets twice and fails, let go, then the code's own refusal;
    /// `Ok` after it.
    fn run(&self) -> Result<(), Error> {
        if self.runs.fetch_add(1, Ordering::AcqRel) > 0 {
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
