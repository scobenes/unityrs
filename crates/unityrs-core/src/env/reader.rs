// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py, UnityPy/helpers/ImportHelper.py, UnityPy/files/File.py. See THIRD_PARTY_LICENSES.
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
    /// Loads a bounded source and observes the exact immutable bytes that
    /// detection and parsing consume, before either starts. Split companions
    /// are joined by the source reader first; compressed containers are still
    /// compressed here. The observer shares this load's input meter and may
    /// refuse without publishing any files or CAB registrations.
    #[allow(clippy::needless_pass_by_value)]
    pub fn load_observed(
        &self,
        source: super::Source,
        observe: impl FnOnce(&Bytes, &std::sync::Arc<Meter>) -> Result<(), Error>,
    ) -> Result<File, Error> {
        let name = super::source_name(&source);
        #[cfg(feature = "test-counters")]
        let load_id = crate::counters::LoadId::draw(&name);
        let ctx = self.ctx();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let (data, reader) = super::registry::read_source(&source, &meter, ctx)?;
        observe(&data, &meter)?;
        let ty = check_file_type_with_reader(&data, &ctx.limits, reader)?;
        let parsed = parse_file_with_reader(name.clone(), data, ty, None, &meter, ctx, reader);
        #[cfg(feature = "test-counters")]
        crate::testing::adopt(&parsed, || crate::testing::Key::load(load_id));
        let file = parsed?;
        let load = self.register_file(&name, &file, &mut Target::Caller)?;
        self.list(name, file.clone(), false, load, &meter)?;
        self.inner
            .loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(file)
    }

    /// Parses caller-owned bytes without adding an environment file or
    /// registering a standalone SerializedFile/raw reader. Container child
    /// CABs retain their constructor registrations (helpers/ImportHelper.py:127-156,
    /// files/File.py:76-90), including duplicate directory rows in parse order.
    pub fn parse_reader(
        &self,
        name: &RawStr,
        data: Bytes,
        reader: ReaderKind,
    ) -> Result<File, Error> {
        let ctx = self.ctx();
        super::within_input_bytes(data.len() as u64, &ctx.limits)?;
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let ty = check_file_type_with_reader(&data, &ctx.limits, reader)?;
        let file = parse_file_with_reader(name.clone(), data, ty, None, &meter, ctx, reader)?;
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
    #[test]
    fn observed_load_sees_exact_joined_split_source_and_its_input_meter() {
        let tmp = unityrs_testkit::scratch::Scratch::new("observed-split");
        let data = unityrs_testkit::ngui::file(
            &[],
            &[unityrs_testkit::ngui::Object::new(
                1,
                49,
                unityrs_testkit::ngui::st(
                    "TextAsset",
                    vec![("m_Name", unityrs_testkit::ngui::s("snapshot"))],
                ),
            )],
            true,
        )
        .unwrap();
        let split = data.len() / 2;
        std::fs::write(tmp.join("source.assets.split0"), &data[..split]).unwrap();
        std::fs::write(tmp.join("source.assets.split1"), &data[split..]).unwrap();
        for name in ["source.assets", "source.assets.split0"] {
            let env = Environment::new(super::super::EnvOptions::unitypy());
            let mut observed = Vec::new();
            let mut input_meter = None;
            let file = env
                .load_observed(
                    super::super::Source::Path(tmp.join(name)),
                    |bytes, meter| {
                        assert_eq!(env.files().count(), 0);
                        observed.extend_from_slice(bytes);
                        input_meter = Some(meter.clone());
                        meter.count_decode(bytes.len() as u64)?;
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(observed, data);
            assert!(matches!(file, File::Serialized(_)));
            assert!(std::sync::Arc::ptr_eq(file.meter(), &input_meter.unwrap()));
            assert_eq!(env.files().count(), 1);
        }
    }

    #[test]
    fn observed_load_refusal_happens_before_detection_or_registration() {
        let mut opts = super::super::EnvOptions::unitypy();
        let mut context = (*opts.context).clone();
        context.limits.max_decode_work = 0;
        opts.context = std::sync::Arc::new(context);
        let env = Environment::new(opts);
        let result = env.load_observed(
            super::super::Source::Bytes {
                name: "malformed".into(),
                data: Bytes::from_static(b"AAAAAAAAAAAAAAAAAAAA"),
            },
            |_, meter| {
                meter.count_decode(1)?;
                Ok(())
            },
        );
        assert!(matches!(result, Err(Error::Limit(e)) if e.kind() == crate::LimitKind::DecodeWork));
        assert_eq!(env.files().count(), 0);
        assert_eq!(env.registry().keys().count(), 0);
    }

    #[test]
    fn observed_load_sees_compressed_web_source_before_inflation() {
        let source = unityrs_testkit::web::plain("UnityWebData1.0", &[("entry.resS", &[42; 256])]);
        let compressed = unityrs_testkit::web::gzip(&source);
        let env = Environment::new(super::super::EnvOptions::unitypy());
        let mut observed = Vec::new();
        let file = env
            .load_observed(
                super::super::Source::Bytes {
                    name: "compressed.web".into(),
                    data: Bytes::copy_from_slice(&compressed),
                },
                |bytes, _| {
                    observed.extend_from_slice(bytes);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(observed, compressed);
        let File::Web(web) = file else {
            panic!("expected WebFile")
        };
        assert_eq!(web.data().as_ref(), source);
        assert_eq!(env.files().count(), 1);
    }
}
