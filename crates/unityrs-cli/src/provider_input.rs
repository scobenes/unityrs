//! One bounded snapshot of a TPK from an already opened descriptor.
use std::{fs::File, io::Read, sync::Arc};
use unityrs_core::{
    Bytes, Error, LimitError, LimitKind,
    budget::DecodeBuf,
    limits::{Limits, Meter},
};
use unityrs_tpk::{TpkError, TpkProvider};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn check(length: u64, limits: &Limits) -> Result<()> {
    if length > limits.max_input_bytes && limits.max_input_bytes <= TpkProvider::MAX_FILE_BYTES {
        return Err(Error::Limit(LimitError::refused(
            LimitKind::InputBytes,
            length,
            limits.max_input_bytes,
        ))
        .into());
    }
    if length > TpkProvider::MAX_FILE_BYTES {
        return Err(TpkError::TooLarge {
            what: "file",
            value: length,
            max: TpkProvider::MAX_FILE_BYTES,
        }
        .into());
    }
    Ok(())
}

pub fn read(file: File, limits: &Limits, meter: &Arc<Meter>) -> Result<Bytes> {
    let length = file.metadata().map_err(TpkError::from)?.len();
    bounded(file, length, limits, meter)
}

fn bounded(
    mut input: impl Read,
    length: u64,
    limits: &Limits,
    meter: &Arc<Meter>,
) -> Result<Bytes> {
    // Refuse the descriptor's stated size before reserving or reading bytes.
    check(length, limits)?;
    let cap = limits.max_input_bytes.min(TpkProvider::MAX_FILE_BYTES);
    let mut out = DecodeBuf::new(meter, length, cap, LimitKind::InputBytes).map_err(Error::from)?;
    let mut chunk = [0; 8192];
    loop {
        // Probe at most one byte past the bound, without allocating that byte.
        let room = usize::try_from(cap.saturating_sub(out.len() as u64).saturating_add(1))
            .unwrap_or(usize::MAX)
            .min(chunk.len());
        let n = match input.read(chunk.get_mut(..room).unwrap_or_default()) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(TpkError::from(e).into()),
        };
        check((out.len() as u64).saturating_add(n as u64), limits)?;
        out.extend_from_slice(chunk.get(..n).unwrap_or_default())
            .map_err(Error::from)?;
    }
    Ok(out.into_bytes().map_err(Error::from)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unityrs_core::budget::{Reservation, Unbounded};
    #[test]
    fn stated_oversize_is_refused_before_reading_or_reserving() {
        struct Unread;
        impl Read for Unread {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("oversized provider must not be read");
            }
        }
        for cap in [17, TpkProvider::MAX_FILE_BYTES] {
            let mut limits = Limits::default();
            limits.max_input_bytes = cap;
            let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
            assert!(bounded(Unread, cap + 1, &limits, &meter).is_err());
            assert_eq!(meter.granted(), 0);
        }
        let limits = Limits::default();
        let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        let error = bounded(Unread, TpkProvider::MAX_FILE_BYTES + 1, &limits, &meter).unwrap_err();
        assert!(error.downcast_ref::<TpkError>().is_some());
        assert_eq!(meter.granted(), 0);
    }
    #[test]
    fn bounded_reader_refuses_growth_past_each_cap() {
        for cap in [17, TpkProvider::MAX_FILE_BYTES] {
            let mut limits = Limits::default();
            limits.max_input_bytes = cap;
            let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
            let error = bounded(std::io::repeat(0).take(cap + 1), 0, &limits, &meter).unwrap_err();
            assert!(error.to_string().contains(&cap.to_string()), "{error}");
            assert_eq!(meter.used(), 0);
        }
        let limits = Limits::default();
        let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        let error = bounded(
            std::io::repeat(0).take(TpkProvider::MAX_FILE_BYTES + 1),
            0,
            &limits,
            &meter,
        )
        .unwrap_err();
        assert!(error.downcast_ref::<TpkError>().is_some());
        assert_eq!(meter.used(), 0);
    }
}
