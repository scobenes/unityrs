use std::{cmp::Ordering, sync::Arc};
use unityrs_batch::{
    phase::Account,
    sort::{self, Options, Order, Source},
    spool::{Counter, Host, workspace::Workspace},
};
use unityrs_testkit::scratch::Scratch;

struct Bytes;
impl Order for Bytes {
    fn tag(&self) -> u32 {
        7
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        a.cmp(b)
    }
}
struct Numbers {
    left: u32,
    total: u32,
    reverse: bool,
}
impl Source for Numbers {
    fn next(
        &mut self,
        out: &mut sort::Frame,
        _: &dyn Fn() -> bool,
    ) -> Result<bool, unityrs_batch::phase::Error> {
        if self.left == 0 {
            return Ok(false);
        }
        let n = if self.reverse {
            self.left - 1
        } else {
            self.total - self.left
        };
        self.left -= 1;
        out.set(&n.to_be_bytes())?;
        Ok(true)
    }
}

#[test]
fn disk_sort_is_independent_of_cuts_fan_in_and_source_order() {
    let scratch = Scratch::new("sort-orders");
    for fan_in in [2, 3, 16] {
        for records in [1, 16, 17, 257] {
            for reverse in [false, true] {
                let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
                let mut workspace = Workspace::create(
                    scratch.join(format!("w-{fan_in}-{records}-{reverse}")),
                    counter.clone(),
                    Arc::new(Host),
                )
                .unwrap();
                let account = Account::new(64 << 20);
                let options = Options {
                    run_bytes: 1024,
                    run_records: 1,
                    fan_in,
                };
                let mut source = Numbers {
                    left: records,
                    total: records,
                    reverse,
                };
                let result = sort::sort(
                    &mut source,
                    &Bytes,
                    &mut workspace,
                    &account,
                    options,
                    &|| false,
                    &sort::NoHooks,
                )
                .unwrap();
                assert_eq!(result.runs, u64::from(records));
                let file = result.file.unwrap();
                let mut reader =
                    sort::Reader::open(&workspace, &file, &Bytes, account.meter(), &|| false)
                        .unwrap();
                let mut found = 0;
                while let Some(bytes) = reader.next(&|| false).unwrap() {
                    let n = u32::from_be_bytes(bytes.try_into().unwrap());
                    assert_eq!(n, found);
                    found += 1;
                }
                assert_eq!(found, records);
                drop(reader);
                workspace.retire(file, &|| false).unwrap();
                assert_eq!(counter.used(), 0);
                assert_eq!(account.snapshot().input.used, 0);
                assert!(account.snapshot().input.peak <= 40 << 20);
            }
        }
    }
}

#[test]
fn slash_first_matches_written_order() {
    let values = ["", "a", "a/", "a/!", "a/é", "a!", "a.", "a0", "aé", "b"];
    for (i, a) in values.iter().enumerate() {
        for (j, b) in values.iter().enumerate() {
            assert_eq!(sort::slash_first(a.as_bytes(), b.as_bytes()), i.cmp(&j));
        }
    }
    assert_eq!(sort::slash_first(b"a/", b"a\0"), Ordering::Less);
}

#[test]
fn canonical_bytes_match_across_twenty_orders_run_sizes_and_fan_ins() {
    struct Permutation {
        cursor: u32,
        seed: u32,
    }
    impl Source for Permutation {
        fn next(
            &mut self,
            out: &mut sort::Frame,
            _: &dyn Fn() -> bool,
        ) -> Result<bool, unityrs_batch::phase::Error> {
            if self.cursor == 257 {
                return Ok(false);
            }
            let value = (self.cursor * (2 * self.seed + 1) + self.seed) % 257;
            self.cursor += 1;
            out.set(&value.to_be_bytes())?;
            Ok(true)
        }
    }
    let scratch = Scratch::new("sort-canonical");
    let mut expected = b"URSSORT\0".to_vec();
    expected.extend_from_slice(&1_u32.to_le_bytes());
    expected.extend_from_slice(&7_u32.to_le_bytes());
    for n in 0_u32..257 {
        expected.extend_from_slice(&8_u32.to_le_bytes());
        expected.extend_from_slice(&n.to_be_bytes());
    }
    for seed in 0..20 {
        for run_bytes in [1024, 32 * 1024 * 1024] {
            for fan_in in [2, 3, 16] {
                let mut workspace = Workspace::create(
                    scratch.join(format!("{seed}-{run_bytes}-{fan_in}")),
                    Counter::new(u64::MAX, 0, 4096).unwrap(),
                    Arc::new(Host),
                )
                .unwrap();
                let report = sort::sort(
                    &mut Permutation { cursor: 0, seed },
                    &Bytes,
                    &mut workspace,
                    &Account::new(64 << 20),
                    Options {
                        run_bytes,
                        fan_in,
                        ..Options::default()
                    },
                    &|| false,
                    &sort::NoHooks,
                )
                .unwrap();
                let file = report.file.unwrap();
                let mut actual = Vec::new();
                std::io::Read::read_to_end(
                    &mut workspace.open(&file, &|| false).unwrap(),
                    &mut actual,
                )
                .unwrap();
                assert_eq!(actual, expected);
                workspace.retire(file, &|| false).unwrap();
            }
        }
    }
}

#[test]
fn equal_keys_and_duplicates_have_canonical_bytes_across_merge_shapes() {
    struct Key;
    impl Order for Key {
        fn tag(&self) -> u32 {
            19
        }
        fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
            a.first().cmp(&b.first())
        }
    }
    struct Records(std::vec::IntoIter<&'static [u8]>);
    impl Source for Records {
        fn next(
            &mut self,
            out: &mut sort::Frame,
            _: &dyn Fn() -> bool,
        ) -> Result<bool, unityrs_batch::phase::Error> {
            let Some(bytes) = self.0.next() else {
                return Ok(false);
            };
            out.set(bytes)?;
            Ok(true)
        }
    }
    let scratch = Scratch::new("sort-equal-keys");
    // Literal full artifact: semantic key is the first byte, payload breaks ties.
    // Both copies of `az` and `bm` must survive every merge.
    let expected = b"URSSORT\0\x01\0\0\0\x13\0\0\0\x06\0\0\0aa\x06\0\0\0am\x06\0\0\0az\x06\0\0\0az\x06\0\0\0ba\x06\0\0\0bm\x06\0\0\0bm\x06\0\0\0bz";
    for fan_in in [2, 3, 16] {
        for cut in [1, 2, 3, 8] {
            for reverse in [false, true] {
                let mut records: Vec<&'static [u8]> =
                    vec![b"bz", b"az", b"bm", b"am", b"az", b"aa", b"bm", b"ba"];
                if reverse {
                    records.reverse();
                }
                let mut workspace = Workspace::create(
                    scratch.join(format!("{fan_in}-{cut}-{reverse}")),
                    Counter::new(u64::MAX, 0, 4096).unwrap(),
                    Arc::new(Host),
                )
                .unwrap();
                let report = sort::sort(
                    &mut Records(records.into_iter()),
                    &Key,
                    &mut workspace,
                    &Account::new(64 << 20),
                    Options {
                        run_records: cut,
                        fan_in,
                        ..Options::default()
                    },
                    &|| false,
                    &sort::NoHooks,
                )
                .unwrap();
                let file = report.file.unwrap();
                let mut actual = Vec::new();
                std::io::Read::read_to_end(
                    &mut workspace.open(&file, &|| false).unwrap(),
                    &mut actual,
                )
                .unwrap();
                assert_eq!(
                    actual, expected,
                    "fan_in={fan_in}, cut={cut}, reverse={reverse}"
                );
                workspace.retire(file, &|| false).unwrap();
            }
        }
    }
}
