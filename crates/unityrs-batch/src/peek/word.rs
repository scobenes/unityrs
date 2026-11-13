//! Stable explicit encoding, independent of Rust enum discriminants.
//! Low byte: 0 pending, 1 all-clear, 2 disk, 3 failed. Success uses the
//! high 32 bits for N; failure uses bytes 1/2 for kind/argument. All other
//! bits must be zero. Limits are reconstructed from M and core defaults.
use crate::{
    attempt::DetailCode,
    reason::{Limit, LimitKind},
};
use std::sync::atomic::{AtomicU64, Ordering};
use unityrs_core::limits::Limits;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureCode {
    NotABundle,
    Container(DetailCode),
    Limit(LimitKind),
}
const DETAILS: [DetailCode; 10] = [
    DetailCode::Detect,
    DetailCode::OutOfBounds,
    DetailCode::Malformed,
    DetailCode::Unsupported,
    DetailCode::Decompress,
    DetailCode::Version,
    DetailCode::TypeIndex,
    DetailCode::Tree,
    DetailCode::AssetBundle,
    DetailCode::Read,
];
const LIMITS: [LimitKind; 11] = [
    LimitKind::InputMemory,
    LimitKind::InputBytes,
    LimitKind::DecodeWork,
    LimitKind::BlockOutput,
    LimitKind::Inflate,
    LimitKind::ContainerDepth,
    LimitKind::TreeDepth,
    LimitKind::ValueDepth,
    LimitKind::ValuesPerObject,
    LimitKind::ZeroSizeElements,
    LimitKind::Count,
];

impl FailureCode {
    /// A checked limit value; None for a non-limit code or an invalid kind.
    #[must_use]
    pub fn limit(self, m: u64) -> Option<Limit> {
        let Self::Limit(kind) = self else {
            return None;
        };
        let d = Limits::default();
        let value = match kind {
            LimitKind::InputMemory | LimitKind::InputBytes => m,
            LimitKind::DecodeWork => m.saturating_mul(4),
            LimitKind::BlockOutput => d.max_block_output,
            LimitKind::Inflate => d.max_inflate,
            LimitKind::ContainerDepth => u64::from(d.max_container_depth),
            LimitKind::TreeDepth => u64::from(d.max_tree_depth),
            LimitKind::ValueDepth => u64::from(d.max_value_depth),
            LimitKind::ValuesPerObject => d.max_values_per_object,
            LimitKind::ZeroSizeElements => d.max_zero_size_elements,
            LimitKind::Count => u64::from(u32::MAX),
            _ => return None,
        };
        Some(Limit { kind, value })
    }
    fn fields(self) -> Result<(u64, u64), InvalidWord> {
        Ok(match self {
            Self::NotABundle => (1, 0),
            Self::Container(detail) => (
                2,
                DETAILS
                    .iter()
                    .position(|d| *d == detail)
                    .ok_or(InvalidWord)? as u64,
            ),
            Self::Limit(kind) => (
                3,
                LIMITS.iter().position(|k| *k == kind).ok_or(InvalidWord)? as u64,
            ),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    #[default]
    Pending,
    Clear(u32),
    Disk(u32),
    Failed(FailureCode),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidWord;
impl Outcome {
    pub fn encode(self) -> Result<u64, InvalidWord> {
        Ok(match self {
            Self::Pending => 0,
            Self::Clear(n) => (u64::from(n) << 32) | 1,
            Self::Disk(n) => (u64::from(n) << 32) | 2,
            Self::Failed(code) => {
                let (kind, arg) = code.fields()?;
                (arg << 16) | (kind << 8) | 3
            }
        })
    }
    pub fn decode(raw: u64) -> Result<Self, InvalidWord> {
        match raw & 255 {
            0 if raw == 0 => Ok(Self::Pending),
            tag @ (1 | 2) if raw & 0xffff_ff00 == 0 => {
                let n = u32::try_from(raw >> 32).map_err(|_| InvalidWord)?;
                Ok(if tag == 1 {
                    Self::Clear(n)
                } else {
                    Self::Disk(n)
                })
            }
            3 if raw >> 24 == 0 => {
                let arg = usize::try_from((raw >> 16) & 255).map_err(|_| InvalidWord)?;
                let code = match (raw >> 8) & 255 {
                    1 if arg == 0 => FailureCode::NotABundle,
                    2 => FailureCode::Container(*DETAILS.get(arg).ok_or(InvalidWord)?),
                    3 => FailureCode::Limit(*LIMITS.get(arg).ok_or(InvalidWord)?),
                    _ => return Err(InvalidWord),
                };
                Ok(Self::Failed(code))
            }
            _ => Err(InvalidWord),
        }
    }
}

#[derive(Debug, Default)]
pub struct Word(AtomicU64);
impl Word {
    pub fn load(&self) -> Result<Outcome, InvalidWord> {
        Outcome::decode(self.0.load(Ordering::Acquire))
    }
    /// One writer, once. Duplicate ends/publications are internal errors.
    pub fn publish(&self, outcome: Outcome) -> Result<(), InvalidWord> {
        if outcome == Outcome::Pending {
            return Err(InvalidWord);
        }
        self.0
            .compare_exchange(0, outcome.encode()?, Ordering::Release, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| InvalidWord)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_word_form_roundtrips_and_reserved_bits_are_rejected() {
        let mut forms = vec![Outcome::Pending, Outcome::Failed(FailureCode::NotABundle)];
        for n in [0, 1, 7, 8, 9, i32::MAX as u32, u32::MAX] {
            forms.extend([Outcome::Clear(n), Outcome::Disk(n)]);
        }
        forms.extend(DETAILS.map(|d| Outcome::Failed(FailureCode::Container(d))));
        forms.extend(LIMITS.map(|k| Outcome::Failed(FailureCode::Limit(k))));
        for form in forms {
            assert_eq!(Outcome::decode(form.encode().unwrap()), Ok(form));
        }
        for raw in [
            4,
            256,
            257,
            258,
            3 | (4 << 8),
            3 | (1 << 8) | (1 << 16),
            1 << 63,
            3 | (1 << 24),
        ] {
            assert_eq!(Outcome::decode(raw), Err(InvalidWord), "{raw:x}");
        }
        assert_eq!(Outcome::Clear(9).encode(), Ok((9 << 32) | 1));
        assert_eq!(Outcome::Disk(9).encode(), Ok((9 << 32) | 2));
        let word = Word::default();
        assert!(word.publish(Outcome::Pending).is_err());
        word.publish(Outcome::Disk(9)).unwrap();
        assert!(word.publish(Outcome::Clear(1)).is_err());
        assert_eq!(word.load(), Ok(Outcome::Disk(9)));
    }
    #[test]
    fn limit_values_come_only_from_m_and_defaults() {
        for kind in LIMITS {
            let code = FailureCode::Limit(kind);
            let limit = code.limit(17).unwrap();
            assert_eq!(limit.kind, kind);
            assert!(limit.value > 0);
        }
        assert_eq!(
            FailureCode::Limit(LimitKind::DecodeWork)
                .limit(17)
                .unwrap()
                .value,
            68
        );
        assert!(FailureCode::Limit(LimitKind::ExportWork).fields().is_err());
        assert!(FailureCode::Container(DetailCode::Edquot).fields().is_err());
    }
}
