//! JSON report uses the existing Python-compatible string writer, retaining
//! invalid UTF-8 as JSON escapes. No diagnostic formats arbitrary input text.
use std::sync::Arc;
use unityrs_core::limits::Meter;
use unityrs_export::{
    extractor::{Error, Report},
    pyjson,
};
use unityrs_mesh::obj::{ChargedBytes, MeteredWrite};
struct StringSink<'a> {
    out: &'a mut ChargedBytes,
    error: Option<unityrs_mesh::MeshError>,
}
impl pyjson::Sink for StringSink<'_> {
    fn put(&mut self, bytes: &[u8]) {
        if self.error.is_none() {
            self.error = self.out.write_bytes(bytes).err();
        }
    }
}
fn string(out: &mut ChargedBytes, bytes: &[u8]) -> Result<(), Error> {
    let mut sink = StringSink { out, error: None };
    pyjson::string(bytes, &mut sink);
    if let Some(e) = sink.error {
        return Err(e.into());
    }
    Ok(())
}
fn code(error: &Error) -> &'static str {
    match error {
        Error::UnsafePath => "unsafe_path",
        Error::Field(_) => "field",
        Error::Core(_) => "core",
        Error::Deref(_) => "dereference",
        Error::Charge(_) => "charge",
        Error::Simple(_) => "simple",
        Error::Audio(_) => "audio",
        Error::Shader(_) => "shader",
        Error::Mesh(_) => "mesh",
        Error::MeshDecode(_) => "mesh_decode",
        Error::Decode(_) => "decode",
        Error::Sprite(_) => "sprite",
        Error::Fetch(_) => "resource",
        Error::Image(_) => "image_writer",
        Error::Io(_) => "io",
    }
}
pub(super) fn checked(report: Report) -> Result<Report, Error> {
    if report
        .failures
        .as_slice()
        .iter()
        .any(|f| f.error.resource_refusal())
    {
        let (failures, _charge) = report.failures.into_parts();
        let failure = failures
            .into_iter()
            .find(|f| f.error.resource_refusal())
            .ok_or(Error::Field("missing resource failure"))?;
        return Err(failure.error);
    }
    Ok(report)
}
pub(super) fn json(report: &Report, meter: &Arc<Meter>) -> Result<ChargedBytes, Error> {
    let mut out = ChargedBytes::new(meter);
    out.write_bytes(b"{\"written\":[")?;
    for (i, file) in report.written.as_slice().iter().enumerate() {
        if i > 0 {
            out.write_bytes(b",")?;
        }
        out.write_bytes(b"{\"path\":")?;
        string(&mut out, file.path.as_bytes())?;
        out.write_bytes(b",\"serialized_file\":")?;
        string(&mut out, file.object.file().name().as_bytes())?;
        out.write_args(format_args!(
            ",\"path_id\":{},\"class_id\":{},\"size\":{}}}",
            file.object.path_id(),
            file.object.class_id(),
            file.size
        ))?;
    }
    out.write_bytes(b"],\"failures\":[")?;
    for (i, failure) in report.failures.as_slice().iter().enumerate() {
        if i > 0 {
            out.write_bytes(b",")?;
        }
        out.write_bytes(b"{\"path\":")?;
        string(&mut out, failure.path.as_bytes())?;
        out.write_bytes(b",\"serialized_file\":")?;
        string(&mut out, failure.object.file().name().as_bytes())?;
        out.write_args(format_args!(
            ",\"path_id\":{},\"class_id\":{},\"reason\":\"{}\"}}",
            failure.object.path_id(),
            failure.object.class_id(),
            code(&failure.error)
        ))?;
    }
    out.write_bytes(b"]}\n")?;
    Ok(out)
}
