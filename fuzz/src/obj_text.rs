//! Raw OBJ/MTL text and corrected renderer export from mutated scenes.
use crate::scene::{Digest, Outcome};
use std::sync::Arc;
use unityrs_core::limits::Meter;
use unityrs_mesh::{
    MeshError,
    obj::{self, MeteredWrite, ObjSink, Writer},
};

struct Sink<'a> {
    digest: &'a mut Digest,
    meter: &'a Arc<Meter>,
}
impl ObjSink for Sink<'_> {
    fn file(
        &mut self,
        stem: &[u8],
        extension: &'static str,
        write: &mut dyn FnMut(&mut dyn MeteredWrite) -> Result<(), MeshError>,
    ) -> Result<(), MeshError> {
        self.digest.tag((stem, extension));
        let mut writer = Writer::new(&mut *self.digest, self.meter);
        write(&mut writer)?;
        writer.finish()?;
        Ok(())
    }
}
/// Panics on nondeterministic output or a retained operation charge.
pub fn check(input: &[u8]) -> bool {
    crate::scene::check(input, |case, limit| {
        crate::scene::run(case, limit, |env, file, meter| {
            let mut digest = Digest::default();
            for object in file.objects() {
                let result = match object.class_id() {
                    43 => unityrs_mesh::read_mesh_for_obj(&object, env, meter).and_then(|mesh| {
                        let mut writer = Writer::new(&mut digest, meter);
                        let written = obj::export_mesh_obj(&mesh, None, &mut writer)?;
                        writer.finish()?;
                        digest.tag(format!("{written:?}"));
                        Ok(())
                    }),
                    23 | 137 => obj::export_mesh_renderer(
                        &object,
                        env,
                        meter,
                        &mut Sink {
                            digest: &mut digest,
                            meter,
                        },
                    )
                    .map(|summary| digest.tag(format!("{summary:?}"))),
                    _ => continue,
                };
                if let Err(e) = result {
                    return Outcome::mesh_error(e);
                }
            }
            Outcome::Done(digest.finish())
        })
    })
}
