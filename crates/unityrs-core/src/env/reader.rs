// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! A binding's immutable Python buffer keeps its reader's origin and uses one
//! input meter. The caller retains the bytes; all core storage is metered.
use super::{Environment, load::Target};
use crate::{
    Bytes, Error, File, RawStr,
    budget::Reservation,
    detect::{FileType, check_file_type_with_reader},
    files::parse_file_with_reader,
    limits::Meter,
    stream::ReaderKind,
};

impl Environment {
    /// Loads caller-owned bytes by the memory or stream reader's rules,
    /// registers their CABs and lists the file with UnityPy's dependency flag
    /// (environment.py:100-160). ZIP members are loaded and the result is None.
    /// The caller's buffer is not copied or charged; tables, decoded bytes and
    /// subsequent reads share this load's single input meter.
    pub fn load_reader(
        &self,
        name: RawStr,
        data: Bytes,
        reader: ReaderKind,
        is_dependency: bool,
    ) -> Result<Option<File>, Error> {
        let ctx = self.ctx();
        super::within_input_bytes(data.len() as u64, &ctx.limits)?;
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let ty = check_file_type_with_reader(&data, &ctx.limits, reader)?;
        if ty == FileType::Zip {
            self.load_zip_data(data, &meter)?;
            self.list_zip_none(name, &meter)?;
            return Ok(None);
        }
        let file = parse_file_with_reader(name.clone(), data, ty, None, &meter, ctx, reader)?;
        let load = self.register_file(&name, &file, &mut Target::Caller)?;
        self.list(name, file.clone(), is_dependency, load, &meter)?;
        self.inner
            .loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Some(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_and_stream_origins_keep_their_detection_rules() {
        let env = Environment::new(super::super::EnvOptions::unitypy());
        let data = Bytes::from_static(b"AAAAAAAAAAAAAAAAAAAA");
        assert!(matches!(
            env.load_reader("memory".into(), data.clone(), ReaderKind::Memory, false),
            Err(Error::Detect(crate::detect::DetectError::Unterminated))
        ));
        let file = env
            .load_reader("stream".into(), data, ReaderKind::Stream, false)
            .unwrap()
            .unwrap();
        let File::Raw(raw) = file else {
            panic!("expected raw reader")
        };
        assert_eq!(raw.reader_kind(), ReaderKind::Stream);
        assert_eq!(env.files().count(), 1);
    }
}
