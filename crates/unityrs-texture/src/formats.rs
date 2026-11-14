//! A set of [`Format`]s: the formats a batch spec lets [`crate::select_in`]
//! export (`docs/BATCH_SPEC.md` 1.5 and section 3). Version 1 exports
//! [`Formats::V1`], the five of `docs/plans/extract.md` E15; version 2 names
//! its own, any of the formats this crate decodes.

use std::fmt;

use crate::format::{Format, TEXTURE_FORMATS};

/// A set of formats, by `TextureFormat` value. Every value UnityPy 1.25.2
/// names is below 128 ([`TEXTURE_FORMATS`]).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Formats(u128);

const _: () = crate::send_sync::<Formats>();
const _: () = {
    let mut rest: &[(i32, &str)] = &TEXTURE_FORMATS;
    while let [(value, _), tail @ ..] = rest {
        assert!(*value >= 0 && *value < 128);
        rest = tail;
    }
};

impl Formats {
    /// No format.
    pub const NONE: Self = Self(0);

    /// The five version 1 exports ([`Format::EXPORTED`]).
    pub const V1: Self = Self::of(&Format::EXPORTED);

    /// Every format this crate decodes ([`Format::ALL`]).
    pub const DECODED: Self = Self::of(&Format::ALL);

    /// The set of `formats`.
    const fn of(mut formats: &[Format]) -> Self {
        let mut set = Self::NONE;
        while let [first, rest @ ..] = formats {
            set = set.with(*first);
            formats = rest;
        }
        set
    }

    /// The set with `format` added.
    #[must_use]
    pub const fn with(self, format: Format) -> Self {
        Self(self.0 | bit(format))
    }

    /// Whether `format` is in the set.
    #[must_use]
    pub const fn contains(self, format: Format) -> bool {
        self.0 & bit(format) != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The formats of the set, in `TextureFormat` order.
    pub fn iter(self) -> impl Iterator<Item = Format> {
        Format::ALL.into_iter().filter(move |f| self.contains(*f))
    }

    /// The format UnityPy 1.25.2 names `name` (a `TextureFormat` member
    /// name), when this crate decodes it: `Err(true)` for a name UnityPy
    /// names that this crate does not decode, `Err(false)` for any other.
    pub fn decoded(name: &str) -> Result<Format, bool> {
        let Some(&(value, _)) = TEXTURE_FORMATS.iter().find(|(_, n)| *n == name) else {
            return Err(false);
        };
        Format::from_texture_format(value).ok_or(true)
    }
}

const fn bit(format: Format) -> u128 {
    // Every value is below 128 (the assertion above).
    1 << (format.texture_format().unsigned_abs() & 127)
}

impl fmt::Debug for Formats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set()
            .entries(self.iter().map(Format::name))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{Format, Formats};

    #[test]
    fn version_1_is_the_five_in_texture_format_order() {
        let names: Vec<_> = Formats::V1.iter().map(Format::name).collect();
        assert_eq!(names, ["RGB24", "RGBA32", "DXT1", "DXT5", "BC7"]);
        for f in Format::ALL {
            assert_eq!(Formats::V1.contains(f), f.exported(), "{f:?}");
            assert!(Formats::DECODED.contains(f));
        }
        assert!(Formats::NONE.is_empty());
        assert_eq!(
            format!("{:?}", Formats::NONE.with(Format::Bc4)),
            r#"{"BC4"}"#
        );
    }

    /// A name is decoded exactly when it names one of [`Format::ALL`],
    /// whatever that list holds (ASTC's twelve since T2b).
    #[test]
    fn the_decoded_names_are_format_all() {
        for (value, name) in crate::TEXTURE_FORMATS {
            let listed = Format::ALL.iter().find(|f| f.texture_format() == value);
            match Formats::decoded(name) {
                Ok(f) => assert_eq!(Some(&f), listed, "{name}"),
                Err(known) => {
                    assert!(known, "{name}");
                    assert_eq!(listed, None, "{name}");
                }
            }
        }
        assert_eq!(Formats::DECODED.iter().count(), Format::ALL.len());
        assert_eq!(Formats::decoded("ASTC_RGBA_8x8"), Ok(Format::AstcRgba8x8));
    }

    #[test]
    fn a_name_is_decoded_known_or_unknown() {
        assert_eq!(Formats::decoded("RGB565"), Ok(Format::Rgb565));
        assert_eq!(Formats::decoded("ETC2_RGBA8"), Ok(Format::Etc2Rgba8));
        // UnityPy names it; this crate does not decode it.
        assert_eq!(Formats::decoded("PVRTC_RGB2"), Err(true));
        assert_eq!(Formats::decoded("ARGBFloat"), Err(true));
        // Names are UnityPy's, case and all.
        assert_eq!(Formats::decoded("rgb565"), Err(false));
        assert_eq!(Formats::decoded("Unknown"), Err(false));
        assert_eq!(Formats::decoded(""), Err(false));
    }
}
