//! The canonical JSON layer (`docs/CANONICAL_DUMP.md` section 2), ported
//! from the P0 spike's `json.rs` and checked against
//! `tools/parity/canonical.py` (`tests/fixtures/dump/encode.json`).
//!
//! Every line is what Python's `json.dumps(record, ensure_ascii=False,
//! separators=(",", ":"))` writes (`canonical.py` `dumps`). Records are
//! written by hand, member by member, never through serde: [`Obj`] writes
//! an object's members in the order they are added, and the helpers below
//! write each kind of member, into any [`Sink`]: the dump's records go into
//! a charged [`crate::text::Text`].

use sha2::{Digest, Sha256};
use unityrs_core::version::{MAX_LIMBS, Packed};

use crate::text::Sink;

/// Raw-byte payloads above this many bytes are summarised as length plus
/// sha256 (`canonical.py` `INLINE_MAX`).
pub const INLINE_MAX: usize = 64;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// A JSON string exactly as `json.dumps(ensure_ascii=False)` writes it:
/// `"` and `\` escaped, the five short escapes, every other code point
/// below U+0020 as `\u00xx` in lower case, everything else literal.
pub fn write_str<S: Sink>(out: &mut S, s: &str) {
    out.push('"');
    let mut start = 0;
    for (i, b) in s.bytes().enumerate() {
        let esc: &str = match b {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0x08 => "\\b",
            0x0c => "\\f",
            0..0x20 => "",
            _ => continue,
        };
        // `i` is an ASCII byte's index, so both slices end on a boundary.
        out.push_str(s.get(start..i).unwrap_or_default());
        if esc.is_empty() {
            out.push_str("\\u00");
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        } else {
            out.push_str(esc);
        }
        start = i + 1;
    }
    out.push_str(s.get(start..).unwrap_or_default());
    out.push('"');
}

/// Appends `bytes` as lowercase hex, two digits per byte.
pub fn push_hex<S: Sink>(out: &mut S, bytes: &[u8]) {
    for b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 15)]));
    }
}

/// `bytes` as lowercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    push_hex(&mut s, bytes);
    s
}

/// The sha256 of `bytes`.
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The sha256 of `bytes` as lowercase hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

/// A quoted hex string (`canonical.py` `enc_hex`: hashes, GUIDs, reserved bytes).
pub fn write_hex_str<S: Sink>(out: &mut S, bytes: &[u8]) {
    out.push('"');
    push_hex(out, bytes);
    out.push('"');
}

/// The raw-bytes rule (section 3, `canonical.py` `enc_raw`): inline hex up to
/// [`INLINE_MAX`] bytes, else the length and the sha256. The bytes a sha256
/// reads are counted first ([`Sink::count_work`]), and refused, nothing is
/// hashed or written.
pub fn write_raw<S: Sink>(out: &mut S, kind: &str, bytes: &[u8]) {
    if bytes.len() > INLINE_MAX && !out.count_work(bytes.len()) {
        return;
    }
    out.push_str("{\"t\":\"");
    out.push_str(kind);
    if bytes.len() <= INLINE_MAX {
        out.push_str("\",\"hex\":\"");
        push_hex(out, bytes);
        out.push_str("\"}");
    } else {
        out.push_str("\",\"len\":");
        let _ = write!(out, "{}", bytes.len());
        out.push_str(",\"sha256\":\"");
        push_hex(out, &sha256(bytes));
        out.push_str("\"}");
    }
}

/// The metadata-string rule (section 4.2, `canonical.py` `enc_name`): a JSON
/// string when the bytes are valid UTF-8 (strict, as Python's codec and
/// `std::str::from_utf8` agree), else `{"t":"str","hex":H}` at any length,
/// never summarised.
pub fn write_name<S: Sink>(out: &mut S, raw: &[u8]) {
    if let Ok(s) = std::str::from_utf8(raw) {
        write_str(out, s);
    } else {
        out.push_str("{\"t\":\"str\",\"hex\":\"");
        push_hex(out, raw);
        out.push_str("\"}");
    }
}

/// A typetree string value (section 3, `canonical.py` `enc_str`): a JSON
/// string when valid UTF-8, else the raw-bytes rule with kind `str`.
pub fn write_value_str<S: Sink>(out: &mut S, raw: &[u8]) {
    match std::str::from_utf8(raw) {
        Ok(s) => write_str(out, s),
        Err(_) => write_raw(out, "str", raw),
    }
}

/// A packed engine version, exactly (section 4.2: it can pass 64 bits, and a
/// 4,300-digit group packs to more than 4,300 digits). Past 128 bits its
/// decimal digits cost a pass over the limbs for each 19 of them, quadratic
/// work that every file record of a version repeats, so each pass is
/// counted first ([`Sink::count_work`], [`LIMB_WORK`] a limb it divides):
/// when the sink refuses one, nothing is written.
///
/// It allocates nothing but what `out` grows by (a [`crate::text::Text`]
/// charges that first): the limbs are borrowed ([`Packed::big_limbs`]), and
/// a version a parse packed holds at most [`MAX_LIMBS`] of them, so the
/// working copy the passes divide and their base-10^19 digits are on the
/// stack, and the digits are written straight into `out`. Round 7 left a
/// copy of the limbs, the working copy, the digits and their string on the
/// heap, uncharged: a dump of a bundle whose engine string is a
/// 4,300-digit major, and of its SerializedFile, held 13,769 bytes above
/// its charge at its peak, against 64 at `2022.3.0f1` (`tests/staging.rs`
/// `a_big_packed_version_is_copied_charged_and_written_without_scratch`,
/// run on that code). A value of more limbs, which only a caller's own
/// [`Packed::from_limbs`] makes, is converted on the heap.
pub fn write_packed<S: Sink>(out: &mut S, packed: &Packed) {
    match packed.big_limbs() {
        None => {
            let _ = write!(out, "{}", packed.as_u128().unwrap_or_default());
        }
        Some(limbs) => write_limbs(out, limbs, true),
    }
}

/// The work [`write_packed`] counts for each limb a pass divides: a limb's
/// bytes, as the core counts 8 bytes a step of a bundle read.
pub const LIMB_WORK: usize = 8;

/// The base-10^19 digits of a value of `limbs` limbs, at most: each digit
/// divides 10^19 > 2^63 out of it, so `64 * limbs / 63 + 1` of them hold
/// any value below 2^(64 * limbs).
const fn chunks_for(limbs: usize) -> usize {
    limbs.saturating_mul(64) / 63 + 1
}

/// [`chunks_for`] the most limbs a parse packs: the stack scratch's size.
const MAX_CHUNKS: usize = chunks_for(MAX_LIMBS);

/// The decimal digits of a little-endian `u64` limb number, by repeated
/// division by 10^19 (quadratic in the limbs, which a 4,300-digit group
/// keeps to a few hundred).
#[must_use]
pub fn limbs_to_decimal(limbs: &[u64]) -> String {
    let mut s = String::new();
    write_limbs(&mut s, limbs, false);
    s
}

/// The decimal digits of `limbs` into `out`: in scratch on the stack when
/// they are at most [`MAX_LIMBS`], each pass's limbs counted by `out` first
/// when `counted` ([`LIMB_WORK`] a limb). Nothing is written when a count is
/// refused. Its own frame, so the stack it takes is only a big version's
/// record's.
#[inline(never)]
fn write_limbs<S: Sink>(out: &mut S, limbs: &[u64], counted: bool) {
    let limbs = limbs
        .get(..limbs.iter().rposition(|l| *l != 0).map_or(0, |i| i + 1))
        .unwrap_or_default();
    let mut stack_work = [0_u64; MAX_LIMBS];
    let mut stack_chunks = [0_u64; MAX_CHUNKS];
    let (mut heap_work, mut heap_chunks);
    let (work, chunks): (&mut [u64], &mut [u64]) =
        if let Some(work) = stack_work.get_mut(..limbs.len()) {
            (work, &mut stack_chunks)
        } else {
            // A caller's own value: more limbs than any parse packs.
            heap_work = vec![0; limbs.len()];
            heap_chunks = vec![0; chunks_for(limbs.len())];
            (&mut heap_work, &mut heap_chunks)
        };
    work.copy_from_slice(limbs);
    let k = to_chunks(work, chunks, &mut |n| !counted || out.count_work(n));
    if let Some(digits) = k.and_then(|k| chunks.get(..k)) {
        write_chunks(out, digits);
    }
}

/// Divides `n` (little-endian, no leading zero limb) by 10^19 until it is
/// zero, each remainder into `chunks`, least significant first, and each
/// pass's limbs counted by `count` before the pass divides them: the
/// number of digits, or `None`, with nothing more done, when `count`
/// refuses one (or `chunks` is too short, which [`chunks_for`] rules out).
fn to_chunks(
    n: &mut [u64],
    chunks: &mut [u64],
    count: &mut dyn FnMut(usize) -> bool,
) -> Option<usize> {
    const CHUNK: u64 = 10_000_000_000_000_000_000;
    let mut len = n.len();
    let mut k = 0;
    while len > 0 {
        if !count(len.saturating_mul(LIMB_WORK)) {
            return None;
        }
        let mut rem: u128 = 0;
        for limb in n.iter_mut().take(len).rev() {
            let cur = (rem << 64) | u128::from(*limb);
            // cur < CHUNK << 64, so the quotient fits a u64.
            *limb = u64::try_from(cur / u128::from(CHUNK)).unwrap_or(u64::MAX);
            rem = cur % u128::from(CHUNK);
        }
        *chunks.get_mut(k)? = u64::try_from(rem).unwrap_or(0);
        k += 1;
        while len > 0 && n.get(len - 1) == Some(&0) {
            len -= 1;
        }
    }
    Some(k)
}

/// Base-10^19 `digits`, least significant first, as one decimal integer
/// ("0" for none).
fn write_chunks<S: Sink>(out: &mut S, digits: &[u64]) {
    let mut it = digits.iter().rev();
    match it.next() {
        Some(top) => {
            let _ = write!(out, "{top}");
        }
        None => out.push('0'),
    }
    for c in it {
        let _ = write!(out, "{c:019}");
    }
}

/// A JSON object being written: members in the order they are added.
pub struct Obj<'a, S: Sink> {
    out: &'a mut S,
    first: bool,
}

const _: () = crate::send_sync::<Obj<'static, String>>();

impl<'a, S: Sink> Obj<'a, S> {
    /// Opens an object on `out`.
    pub fn new(out: &'a mut S) -> Self {
        out.push('{');
        Self { out, first: true }
    }

    /// Writes a member's key and returns the buffer for its value.
    pub fn key(&mut self, key: &str) -> &mut S {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
        write_str(self.out, key);
        self.out.push(':');
        self.out
    }

    /// A string member.
    pub fn str(&mut self, key: &str, v: &str) {
        let out = self.key(key);
        write_str(out, v);
    }

    /// An integer member.
    pub fn int(&mut self, key: &str, v: impl std::fmt::Display) {
        let out = self.key(key);
        let _ = write!(out, "{v}");
    }

    /// An integer member, or `null`.
    pub fn opt_int<T: std::fmt::Display>(&mut self, key: &str, v: Option<T>) {
        match v {
            Some(v) => self.int(key, v),
            None => self.null(key),
        }
    }

    /// A boolean member.
    pub fn bool(&mut self, key: &str, v: bool) {
        self.key(key).push_str(if v { "true" } else { "false" });
    }

    /// A `null` member.
    pub fn null(&mut self, key: &str) {
        self.key(key).push_str("null");
    }

    /// A name member (the metadata-string rule), or `null`.
    pub fn name(&mut self, key: &str, v: Option<&[u8]>) {
        match v {
            Some(v) => {
                let out = self.key(key);
                write_name(out, v);
            }
            None => self.null(key),
        }
    }

    /// A hex member, or `null`.
    pub fn hex(&mut self, key: &str, v: Option<&[u8]>) {
        match v {
            Some(v) => {
                let out = self.key(key);
                write_hex_str(out, v);
            }
            None => self.null(key),
        }
    }

    /// A member whose value is already canonical JSON text.
    pub fn raw(&mut self, key: &str, json: &str) {
        self.key(key).push_str(json);
    }

    /// Closes the object.
    pub fn end(self) {
        self.out.push('}');
    }
}

/// Writes `items` as a JSON array, each with `f`.
pub fn write_array<S: Sink, T>(out: &mut S, items: &[T], mut f: impl FnMut(&mut S, &T)) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        f(out, item);
    }
    out.push(']');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_like_python() {
        let mut out = String::new();
        write_str(
            &mut out,
            "a\"\\\n\r\t\u{8}\u{c}\u{1}\u{1f}/\u{7f}\u{2028}é\u{1f600}",
        );
        assert_eq!(
            out,
            "\"a\\\"\\\\\\n\\r\\t\\b\\f\\u0001\\u001f/\u{7f}\u{2028}é\u{1f600}\""
        );
    }

    #[test]
    fn raw_rule_switches_at_64_bytes() {
        let mut a = String::new();
        write_raw(&mut a, "bytes", &[0xab; 64]);
        assert!(a.starts_with("{\"t\":\"bytes\",\"hex\":\"abab"), "{a}");
        let mut b = String::new();
        write_raw(&mut b, "bytes", &[0xab; 65]);
        assert!(
            b.starts_with("{\"t\":\"bytes\",\"len\":65,\"sha256\":\""),
            "{b}"
        );
    }

    #[test]
    fn names_are_never_summarised() {
        let mut out = String::new();
        write_name(&mut out, &[0xff; 100]);
        assert_eq!(
            out,
            format!("{{\"t\":\"str\",\"hex\":\"{}\"}}", "ff".repeat(100))
        );
        let mut ok = String::new();
        write_name(&mut ok, "caf\u{e9}".as_bytes());
        assert_eq!(ok, "\"caf\u{e9}\"");
    }

    #[test]
    fn limbs_print_exactly() {
        assert_eq!(limbs_to_decimal(&[]), "0");
        assert_eq!(limbs_to_decimal(&[0, 0]), "0");
        assert_eq!(limbs_to_decimal(&[u64::MAX]), u64::MAX.to_string());
        assert_eq!(limbs_to_decimal(&[0, 1]), (1u128 << 64).to_string());
        assert_eq!(
            limbs_to_decimal(&[u64::MAX, u64::MAX]),
            u128::MAX.to_string()
        );
        // 2^128 = 340282366920938463463374607431768211456
        assert_eq!(
            limbs_to_decimal(&[0, 0, 1]),
            "340282366920938463463374607431768211456"
        );
        // A chunk boundary with inner zeros: 10^19 * 10^19 + 1 = 10^38 + 1.
        let v: u128 = 10u128.pow(38) + 1;
        let limbs = [
            u64::try_from(v & u128::from(u64::MAX)).unwrap(),
            u64::try_from(v >> 64).unwrap(),
        ];
        assert_eq!(limbs_to_decimal(&limbs), v.to_string());
        // The most limbs a parse packs, in the stack scratch, and one more,
        // a caller's own value, on the heap: the core's `Display` (its own
        // conversion) agrees, and a value written through `write_packed`
        // too, uncounted by a `String`.
        for n in [MAX_LIMBS, MAX_LIMBS + 1] {
            let limbs: Vec<u64> = (0..n)
                .map(|i| u64::MAX - u64::try_from(i).unwrap())
                .collect();
            let want = Packed::from_limbs(&limbs).to_string();
            assert!(want.len() > 19 * n, "{n} limbs: {} digits", want.len());
            let mut out = String::new();
            write_packed(&mut out, &Packed::from_limbs(&limbs));
            for got in [limbs_to_decimal(&limbs), out] {
                assert!(
                    got == want,
                    "{n} limbs: {} digits, {} wanted",
                    got.len(),
                    want.len()
                );
            }
        }
    }

    /// A sink that counts the work it is told of, and refuses past `max`.
    struct Counting {
        text: String,
        work: usize,
        max: usize,
    }

    impl std::fmt::Write for Counting {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.text.push_str(s);
            Ok(())
        }
    }

    impl Sink for Counting {
        fn push_str(&mut self, s: &str) {
            self.text.push_str(s);
        }
        fn push(&mut self, c: char) {
            self.text.push(c);
        }
        fn count_work(&mut self, bytes: usize) -> bool {
            if self.work + bytes > self.max {
                return false;
            }
            self.work += bytes;
            true
        }
    }

    /// 2^128 (three limbs) takes three passes: 2^128 / 10^19 is still past
    /// 2^64 (two limbs), then 34 (one): 3 + 2 + 1 limbs divided, 8 bytes
    /// each, counted before each pass; refused at the second pass, nothing
    /// is written. Below 2^128 nothing is counted.
    #[test]
    fn a_packed_version_counts_each_pass_before_it_divides() {
        let big = Packed::from_limbs(&[0, 0, 1]);
        let mut out = Counting {
            text: String::new(),
            work: 0,
            max: usize::MAX,
        };
        write_packed(&mut out, &big);
        assert_eq!(out.text, "340282366920938463463374607431768211456");
        assert_eq!(out.work, 6 * LIMB_WORK);
        let mut out = Counting {
            text: String::new(),
            work: 0,
            max: 4 * LIMB_WORK,
        };
        write_packed(&mut out, &big);
        assert_eq!((out.text.as_str(), out.work), ("", 3 * LIMB_WORK));
        let mut out = Counting {
            text: String::new(),
            work: 0,
            max: 0,
        };
        write_packed(&mut out, &Packed::from_limbs(&[u64::MAX, u64::MAX]));
        assert_eq!(
            (out.text.as_str(), out.work),
            (u128::MAX.to_string().as_str(), 0)
        );
    }

    #[test]
    fn objects_write_members_in_order() {
        let mut out = String::new();
        let mut o = Obj::new(&mut out);
        o.str("rec", "end");
        o.int("files", 3u64);
        o.opt_int::<i32>("x", None);
        o.bool("b", false);
        o.name("n", Some(&[0x61, 0xff]));
        o.hex("h", Some(&[0xab, 0x01]));
        o.end();
        assert_eq!(
            out,
            "{\"rec\":\"end\",\"files\":3,\"x\":null,\"b\":false,\"n\":{\"t\":\"str\",\"hex\":\"61ff\"},\"h\":\"ab01\"}"
        );
    }
}
