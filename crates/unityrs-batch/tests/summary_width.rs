//! Read real artifacts on both sides of the final-flush boundary.
mod artifact_support;
use artifact_support::{meter, summary};
use std::sync::Arc;
use unityrs_batch::{
    records::*,
    spool::{Counter, Directory, Host},
    work::Work,
};
use unityrs_testkit::scratch::Scratch;

fn artifact(name: &[u8], ms: u64, block: u64, cap: u64) -> Result<(Vec<u8>, u64), Error> {
    let scratch = Scratch::new("summary-width");
    let (_, meter) = meter();
    let counter = Counter::new(1 << 20, 0, block).unwrap();
    let directory =
        Directory::create(scratch.join("attempt"), counter.clone(), Arc::new(Host)).unwrap();
    let mut writer = Writer::new(&directory, &meter).unwrap();
    let mut work = Work::new(cap);
    writer.append(
        Record::Diagnostic(Diagnostic {
            kind: DiagnosticKind::NestedContainer,
            input: "input",
            bundle: None,
            name: Text::Raw(name),
            cut: false,
        }),
        &mut work,
    )?;
    let mut end = summary(Status::Complete);
    end.figures.as_mut().unwrap().ms = ms;
    let end = writer.finish(end, &mut work)?;
    let bytes = std::fs::read(directory.path().join("records.ndjson")).unwrap();
    let row: serde_json::Value =
        serde_json::from_slice(bytes.split_inclusive(|b| *b == b'\n').next_back().unwrap())
            .unwrap();
    assert_eq!(row["claimed_bytes"], counter.used());
    assert_eq!(row["ms"], ms);
    assert_eq!(row["spooled_part"], end.spooled_part);
    assert_eq!(row["export_work"], work.export_work());
    Ok((bytes, work.export_work()))
}

#[test]
fn time_digits_and_filesystem_claim_digits_cannot_move_the_4k_boundary() {
    let base = artifact(b"", 9, 4096, u64::MAX).unwrap().0.len();
    let extra = 4096 - base;
    let mut name = vec![0; extra / 6];
    name.extend(std::iter::repeat_n(b'x', extra % 6));
    for ms in [0, 9, 10, 999, u64::MAX] {
        for block in [1, 4096, 65536, 1_000_000] {
            let (bytes, work) = artifact(&name, ms, block, 4096).unwrap();
            assert_eq!(bytes.len(), 4096, "ms={ms}, block={block}");
            assert_eq!(work, 4096);
        }
    }
    assert!(matches!(
        artifact(&name, 9, 4096, 4095),
        Err(Error::Work(_))
    ));
    name.push(b'x');
    let (bytes, work) = artifact(&name, 10, 65536, 8192).unwrap();
    assert_eq!((bytes.len(), work), (4097, 8192));
}
