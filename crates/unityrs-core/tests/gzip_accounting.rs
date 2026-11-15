//! A cached gzip CRC failure keeps main's text and 64-byte heap charge.
use std::{io::Write, sync::Arc};
use unityrs_core::{
    Error,
    budget::{Counting, Reservation},
    detect::check_file_type,
    limits::{Limits, Meter},
    once::{CellError, ResultCell, get_or_publish_result},
};

#[test]
fn crc_failure_still_fits_the_original_cache_boundary() {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"UnityWebData1.0\0").unwrap();
    encoder.write_all(&(0..=255).collect::<Vec<u8>>()).unwrap();
    let mut data = encoder.finish().unwrap();
    let at = data.len() - 8;
    data[at] ^= 1;
    let error = Error::from(check_file_type(&data, &Limits::default()).unwrap_err());
    assert_eq!(
        error.to_string(),
        "detection: gzip: BadGzipFile: CRC check failed"
    );
    assert_eq!(error.heap_bytes(), 64);
    let budget = Arc::new(Counting::new(64));
    let meter = Meter::new(64, budget.clone(), Reservation::Input);
    let cell: ResultCell<(), Error> = ResultCell::new();
    assert_eq!(
        get_or_publish_result(&cell, &meter, || Err(error.clone())),
        Err(error.clone())
    );
    assert!(
        cell.get().is_some(),
        "the original 64-byte allowance caches the error"
    );
    assert_eq!(budget.used(), 64);
    assert_eq!(
        get_or_publish_result(&cell, &meter, || panic!("cached error was recomputed")),
        Err(error)
    );
    drop(cell);
    assert_eq!(budget.used(), 0);
}
