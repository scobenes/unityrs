//! Standalone host check when the full offline dependency cache is unavailable.
//! This compiles the real layout and header modules; only charged Buffer's
//! allocation adapter is replaced. It does not validate production accounting.
#![allow(dead_code)]
mod phase {
    use std::ops::{Deref, DerefMut};
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Error;
    impl Error {
        pub fn internal() -> Self {
            Self
        }
    }
    pub struct Meter(usize);
    pub struct Account {
        meter: Meter,
    }
    impl Account {
        pub fn new(limit: u64) -> Self {
            Self {
                meter: Meter(usize::try_from(limit).unwrap()),
            }
        }
        pub fn meter(&self) -> &Meter {
            &self.meter
        }
    }
    pub struct Buffer<T>(Vec<T>);
    impl<T: Copy> Buffer<T> {
        pub fn new(meter: &Meter, n: usize) -> Result<Self, Error> {
            if n > meter.0 {
                return Err(Error::internal());
            }
            let mut values = Vec::new();
            values.try_reserve_exact(n).map_err(|_| Error::internal())?;
            Ok(Self(values))
        }
        pub fn extend(&mut self, v: &[T]) -> Result<(), Error> {
            if self.0.len() + v.len() > self.0.capacity() {
                return Err(Error::internal());
            }
            self.0.extend_from_slice(v);
            Ok(())
        }
        pub fn clear(&mut self) {
            self.0.clear();
        }
    }
    impl<T> Deref for Buffer<T> {
        type Target = [T];
        fn deref(&self) -> &[T] {
            &self.0
        }
    }
    impl<T> DerefMut for Buffer<T> {
        fn deref_mut(&mut self) -> &mut [T] {
            &mut self.0
        }
    }
}
#[path = "../../src/winners/layout.rs"]
pub mod layout_source;
mod winners {
    pub use crate::layout_source as layout;
}
#[path = "../../src/archive/headers.rs"]
mod headers;

#[test]
fn emit_for_python() {
    let Some(directory) = std::env::var_os("UNITYRS_ARCHIVE_ORACLE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    for origin in [
        0,
        layout_source::ZIP32 - 66,
        layout_source::ZIP32 - 65,
        layout_source::ZIP32 - 64,
        layout_source::ZIP32 - 33,
        layout_source::ZIP32 - 32,
        layout_source::ZIP32 - 31,
        layout_source::ZIP32 - 1,
        layout_source::ZIP32,
        layout_source::ZIP32 + 1,
    ] {
        let mut layout = layout_source::Layout::seed(origin, 0, 0).unwrap();
        let mut buffer = phase::Buffer::new(phase::Account::new(1 << 20).meter(), 4096).unwrap();
        let mut local = Vec::new();
        let mut central = Vec::new();
        for (name, payload, crc) in [
            ("a", b"x".as_slice(), 0x8cdc_1683),
            ("b", b"yz", 0x0ff4_4862),
        ] {
            let entry = layout
                .member(
                    u64::try_from(name.len()).unwrap(),
                    u64::try_from(payload.len()).unwrap(),
                )
                .unwrap();
            headers::local(
                &mut buffer,
                name,
                u64::try_from(payload.len()).unwrap(),
                crc,
                entry,
            )
            .unwrap();
            local.extend_from_slice(&buffer);
            local.extend_from_slice(payload);
            headers::central(
                &mut buffer,
                name,
                u64::try_from(payload.len()).unwrap(),
                crc,
                entry,
            )
            .unwrap();
            central.extend_from_slice(&buffer);
        }
        local.extend(central);
        headers::end(&mut buffer, layout.finish().unwrap()).unwrap();
        local.extend_from_slice(&buffer);
        std::fs::write(directory.join(format!("offset-{origin}.zip")), local).unwrap();
    }
    for count in [65_534, 65_535, 65_536] {
        let mut layout = layout_source::Layout::default();
        let mut buffer = phase::Buffer::new(phase::Account::new(1 << 20).meter(), 4096).unwrap();
        let mut local = Vec::new();
        let mut central = Vec::new();
        for i in 0..count {
            let name = format!("m{i:06}");
            let entry = layout
                .member(u64::try_from(name.len()).unwrap(), 0)
                .unwrap();
            headers::local(&mut buffer, &name, 0, 0, entry).unwrap();
            local.extend_from_slice(&buffer);
            headers::central(&mut buffer, &name, 0, 0, entry).unwrap();
            central.extend_from_slice(&buffer);
        }
        local.extend(central);
        headers::end(&mut buffer, layout.finish().unwrap()).unwrap();
        local.extend_from_slice(&buffer);
        std::fs::write(directory.join(format!("count-{count}.zip")), local).unwrap();
    }
}
