//! Render every parsed entry, even after an earlier entry's rendering error.
use std::sync::Arc;
use unityrs_core::limits::Meter;
use unityrs_export::shader::{Error, Program};

/// The first verdict and the number of entries attempted, for deterministic
/// fuzz replay and a regression check on errors hiding later code families.
pub fn render_all(
    blob: &[u8],
    version: (u32, u32),
    meter: &Arc<Meter>,
) -> (Result<(), Error>, usize) {
    let program = match Program::parse(blob, version, meter) {
        Ok(program) => program,
        Err(error) => return (Err(error), 0),
    };
    let mut failure = None;
    let mut attempted = 0;
    for index in 0..program.subprogram_count() {
        let result = i64::try_from(index)
            .map_err(|_| Error::Index)
            .and_then(|index| program.export(index, meter));
        attempted += 1;
        if let Err(error) = result
            && failure.is_none()
        {
            failure = Some(error);
        }
    }
    (failure.map_or(Ok(()), Err), attempted)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_first_entry_does_not_hide_later_metal_entry() {
        let mut blob = Vec::new();
        for n in [2i32, 28, 0, 0, 60, 0, 0] {
            blob.extend(n.to_le_bytes());
        }
        for (kind, code) in [(1, b"\xff".as_slice()), (23, b"ABCDmain\0source")] {
            for n in [
                202_012_090,
                kind,
                0,
                0,
                0,
                0,
                0,
                i32::try_from(code.len()).unwrap(),
            ] {
                blob.extend(n.to_le_bytes());
            }
            blob.extend(code);
            blob.resize(blob.len().next_multiple_of(4), 0);
        }
        // First record is 36 bytes including padding.
        blob[16..20].copy_from_slice(&64i32.to_le_bytes());
        let meter = Meter::unlimited();
        let (result, attempted) = render_all(&blob, (2021, 3), &meter);
        assert_eq!(attempted, 2);
        assert_eq!(result.unwrap_err().exception(), Some("UnicodeDecodeError"));
        assert_eq!(meter.used(), 0);
    }
}
