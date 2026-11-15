//! One finite budget for extraction buffers, paths, retained reports and JSON.
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{Limits, Meter},
};

pub(super) fn parse(raw: &[Vec<u8>]) -> Result<Limits, Box<dyn std::error::Error>> {
    let mut limits = Limits::default();
    let mut args = raw.iter();
    while let Some(arg) = args.next() {
        let target = match arg.as_slice() {
            b"--" => break,
            b"--max-input-memory" => Some(&mut limits.max_input_memory),
            b"--max-input-bytes" => Some(&mut limits.max_input_bytes),
            b"--max-decode-work" => Some(&mut limits.max_decode_work),
            b"--max-tree-depth" => {
                limits.max_tree_depth =
                    std::str::from_utf8(args.next().ok_or("missing limit")?)?.parse()?;
                None
            }
            b"-o"
            | b"--tpk"
            | b"--vorbis-headers"
            | b"--report"
            | b"--asset-filter"
            | b"--ignore-first-container-dirs" => {
                args.next().ok_or("missing argument")?;
                None
            }
            _ => None,
        };
        if let Some(target) = target {
            *target = std::str::from_utf8(args.next().ok_or("missing limit")?)?.parse()?;
        }
    }
    Ok(limits)
}

pub(super) fn extraction(limits: &Limits) -> Arc<Meter> {
    Meter::for_input(limits, Arc::new(Unbounded), Reservation::Input)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_report_and_json_share_the_finite_budget() {
        let mut limits = Limits::default();
        limits.max_input_memory = 4096;
        let meter = extraction(&limits);
        let mut report = unityrs_export::extractor::Report::new(&meter);
        report.written.reserve(1).unwrap();
        let buffer = meter
            .charge(limits.max_input_memory - meter.used() - 16)
            .unwrap();
        let result = super::super::report::json(&report, &meter);
        assert!(matches!(&result, Err(e) if e.resource_refusal()));
        assert!(meter.used() <= limits.max_input_memory);
        drop((result, buffer, report));
        assert_eq!(meter.used(), 0);
    }
}
