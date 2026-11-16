// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py, UnityPy/helpers/ImportHelper.py, UnityPy/files/File.py. See THIRD_PARTY_LICENSES.
//! A binding's immutable Python buffer keeps its reader's origin and uses one
//! input meter. The caller retains the bytes; all core storage is metered.
use super::{Environment, load::Target};
use crate::{
    Bytes, Error, File, RawStr,
    budget::Reservation,
    detect::{FileType, detect_at_position},
    files::parse_file_with_reader,
    limits::Meter,
    stream::ReaderKind,
};

impl Environment {
    /// `ImportHelper`'s parser selection without detection or a top-level file
    /// registration (helpers/ImportHelper.py:127-156). Container constructors
    /// still register their child CABs (files/File.py:76-90).
    pub fn parse_reader(
        &self,
        name: &RawStr,
        data: Bytes,
        reader: ReaderKind,
        ty: FileType,
        parent_file: Option<&File>,
    ) -> Result<File, Error> {
        self.parse_reader_with_state(name, data, reader, ty, parent_file, &mut None)
    }

    /// [`Self::parse_reader`] retaining a partial constructor's endian change.
    pub fn parse_reader_with_state(
        &self,
        name: &RawStr,
        data: Bytes,
        reader: ReaderKind,
        ty: FileType,
        parent_file: Option<&File>,
        endian: &mut Option<crate::stream::Endian>,
    ) -> Result<File, Error> {
        use crate::files::{Parent, ParentKind};
        let ctx = self.ctx();
        super::within_input_bytes(data.len() as u64, &ctx.limits)?;
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let parent = match parent_file {
            Some(File::Bundle(file)) => Some(Parent {
                name: file.name().clone(),
                kind: ParentKind::Bundle {
                    version_engine: file.header().version_engine.clone(),
                    bundle: std::sync::Arc::downgrade(file),
                    dir_index: usize::MAX,
                },
                depth: file.parent().map_or(1, |p| p.depth.saturating_add(1)),
                pinned: data.len() as u64,
            }),
            Some(File::Web(file)) => Some(Parent {
                name: file.name().clone(),
                kind: ParentKind::Web {
                    web: std::sync::Arc::downgrade(file),
                },
                depth: file.parent().map_or(1, |p| p.depth.saturating_add(1)),
                pinned: data.len() as u64,
            }),
            _ => None,
        };
        let file = crate::files::parse_file_with_state(
            name.clone(),
            data,
            ty,
            parent,
            &meter,
            ctx,
            reader,
            0,
            endian,
        )?;
        if matches!(file, File::Bundle(_) | File::Web(_)) {
            self.register_file(name, &file, &mut Target::Caller)?;
        }
        Ok(file)
    }

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
        self.load_reader_at_position(name, data, reader, is_dependency, 0)
    }

    /// [`Self::load_reader`] detecting from the caller's original position
    /// before detection resets the reader for construction.
    pub fn load_reader_at_position(
        &self,
        name: RawStr,
        data: Bytes,
        reader: ReaderKind,
        is_dependency: bool,
        position: i128,
    ) -> Result<Option<File>, Error> {
        let ctx = self.ctx();
        super::within_input_bytes(data.len() as u64, &ctx.limits)?;
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let ty = detect_at_position(&data, &ctx.limits, None, reader, position)?;
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
    use crate::detect::check_file_type_with_reader;
    #[test]
    fn forced_web_bypasses_detection_without_listing_a_root_file() {
        let env = Environment::new(super::super::EnvOptions::unitypy());
        let data = Bytes::from_static(b"UnityWebData\0\x11\0\0\0");
        assert_eq!(
            check_file_type_with_reader(&data, &env.ctx().limits, ReaderKind::Memory).unwrap(),
            FileType::ResourceFile
        );
        let file = env
            .parse_reader(
                &"forced".into(),
                data,
                ReaderKind::Memory,
                FileType::WebFile,
                None,
            )
            .unwrap();
        assert!(matches!(file, File::Web(_)));
        assert_eq!(env.files().count(), 0);
    }

    #[test]
    fn forced_reader_enforces_input_limit_before_parsing() {
        let mut options = super::super::EnvOptions::unitypy();
        let mut context = crate::FileContext::unitypy();
        context.limits.max_input_bytes = 2;
        options.context = std::sync::Arc::new(context);
        let env = Environment::new(options);
        assert!(matches!(
            env.parse_reader(
                &"forced".into(),
                Bytes::from_static(b"raw"),
                ReaderKind::Memory,
                FileType::AssetsFile,
                None,
            ),
            Err(Error::Limit(_))
        ));
        assert_eq!(env.files().count(), 0);
    }

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
