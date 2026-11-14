//! The UnityCN key a version 2 spec names by `unitycn_key: {"env": NAME}`
//! (`docs/BATCH_SPEC.md` 1.1), read at preflight as a header's variable is
//! (1.2): once, from the process's environment, never by `--check`.
//!
//! The variable holds the key as 32 hexadecimal digits, either case: any
//! 16 bytes, a NUL or a byte past ASCII among them, in a value every shell
//! and orchestrator can pass. UnityPy's `set_assetbundle_decrypt_key` takes
//! a `str` and encodes it as UTF-8, so a key of 16 printable characters is
//! given as their bytes in hex (`printf %s KEY | xxd -p`).
//!
//! The value is copied out of the environment whole (`std::env::var_os`,
//! std's own locked read; nothing in this crate reads the environment
//! another way) and only then measured; the copy lives for the read alone, and `--check`'s bound counts it at the 16 KiB
//! cap as it counts every `{"env": NAME}` value (`crate::bound::c_hdr`).
//!
//! An unset or empty variable is `header_env_missing`, one over 16 KiB
//! `header_env_too_long`, and any other value that is not 32 hex digits
//! `header_env_invalid`, each with the variable's name as its detail: the
//! reasons 9.3 gives every `{"env": NAME}` reference. The key renders as
//! its length alone ([`UnityCnKey`]'s `Debug`); the engine copies it into
//! each load's options (`unityrs_core::FileContext`), whose `Debug` leaves
//! it out.

use std::ffi::OsString;
use std::fmt;

use crate::reason::Preflight;
use crate::spec::fields::MAX_HEADER_VALUE;

/// The key's bytes. `Debug` and `Display` print its length alone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UnityCnKey([u8; 16]);

impl UnityCnKey {
    /// The bytes, for `FileContext::set_assetbundle_decrypt_key`.
    #[must_use]
    pub(crate) fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Debug for UnityCnKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}

impl fmt::Display for UnityCnKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// The key in variable `name` of the environment `env` reads.
pub fn read(name: &str, env: impl Fn(&str) -> Option<OsString>) -> Result<UnityCnKey, Preflight> {
    let Some(value) = env(name).filter(|v| !v.is_empty()) else {
        return Err(Preflight::HeaderEnvMissing);
    };
    if value.len() as u64 > MAX_HEADER_VALUE {
        return Err(Preflight::HeaderEnvTooLong);
    }
    let (pairs, []) = value.as_encoded_bytes().as_chunks::<2>() else {
        return Err(Preflight::HeaderEnvInvalid);
    };
    if pairs.len() != 16 {
        return Err(Preflight::HeaderEnvInvalid);
    }
    let mut key = [0; 16];
    for (out, &[high, low]) in key.iter_mut().zip(pairs) {
        let (Some(high), Some(low)) = (nibble(high), nibble(low)) else {
            return Err(Preflight::HeaderEnvInvalid);
        };
        *out = high << 4 | low;
    }
    Ok(UnityCnKey(key))
}

fn nibble(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    use super::{Preflight, UnityCnKey, read};

    const HEX: &str = "73796e7468657469632d6b65792d3031";

    fn one(value: Option<OsString>) -> impl Fn(&str) -> Option<OsString> {
        move |name| (name == "K").then(|| value.clone()).flatten()
    }

    #[test]
    fn the_key_is_32_hex_digits_of_either_case() {
        let key = read("K", one(Some(HEX.into()))).unwrap();
        assert_eq!(key.bytes(), b"synthetic-key-01");
        let upper = read("K", one(Some(HEX.to_uppercase().into()))).unwrap();
        assert_eq!(upper, key);
        let zeros = read("K", one(Some("0".repeat(32).into()))).unwrap();
        assert_eq!(zeros.bytes(), &[0; 16]);
    }

    #[test]
    fn a_missing_long_or_malformed_value_is_its_preflight_reason() {
        assert_eq!(read("K", one(None)), Err(Preflight::HeaderEnvMissing));
        assert_eq!(
            read("K", one(Some("".into()))),
            Err(Preflight::HeaderEnvMissing)
        );
        // Another variable's value is not this one's.
        assert_eq!(
            read("L", one(Some(HEX.into()))),
            Err(Preflight::HeaderEnvMissing)
        );
        let long = "0".repeat(16 * 1024 + 1);
        assert_eq!(
            read("K", one(Some(long.into()))),
            Err(Preflight::HeaderEnvTooLong)
        );
        for bad in [
            &HEX[..30],
            &format!("{HEX}00"),
            &format!(" {}", &HEX[1..]),
            &format!("{}\n", &HEX[..31]),
            "synthetic-key-01",
            &format!("{}g", &HEX[..31]),
            &format!("+{}", &HEX[1..]),
        ] {
            assert_eq!(
                read("K", one(Some(bad.into()))),
                Err(Preflight::HeaderEnvInvalid),
                "{bad:?}"
            );
        }
        let mut raw = HEX.as_bytes().to_vec();
        raw[0] = 0xff;
        assert_eq!(
            read("K", one(Some(OsString::from_vec(raw)))),
            Err(Preflight::HeaderEnvInvalid)
        );
    }

    /// The key never renders: `Debug` and `Display` print its length.
    #[test]
    fn a_key_renders_its_length_alone() {
        let key: UnityCnKey = read("K", one(Some(HEX.into()))).unwrap();
        let text = format!("{key:?} {key} {:?}", Ok::<_, ()>(key));
        assert_eq!(text, "<16 bytes> <16 bytes> Ok(<16 bytes>)");
    }
}
