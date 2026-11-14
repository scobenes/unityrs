//! OBJ command: select a unique Mesh or renderer by path id in one input.
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path, process::ExitCode, sync::Arc};
use unityrs_core::{
    RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
    provider::ProviderChain,
};
use unityrs_mesh::{
    MeshError,
    obj::{self, MeteredWrite, ObjSink},
    read_mesh_for_obj,
};
mod output;
use output::Outputs;
const USAGE: &str = "usage: unityrs obj INPUT --path-id ID -o OUTPUT [--tpk FILE]\n\
Mesh: OUTPUT is an OBJ file. Renderer: OUTPUT is a directory.\n\
Outputs overwrite existing files and symlinks. A failed rename leaves earlier outputs in place.\n\
An interrupted run can leave .unityrs-obj-*.tmp files, which are safe to delete.";
struct Files<'a> {
    root: &'a Path,
    meter: Arc<Meter>,
    outputs: &'a mut Outputs,
}
impl ObjSink for Files<'_> {
    fn file(
        &mut self,
        stem: &[u8],
        extension: &'static str,
        write: &mut dyn FnMut(&mut dyn MeteredWrite) -> Result<(), MeshError>,
    ) -> Result<(), MeshError> {
        if stem.contains(&b'/')
            || stem.contains(&b'\\')
            || stem.contains(&0)
            || stem == b".."
            || stem == b"."
        {
            return Err(MeshError::Field("unsafe output name"));
        }
        let path = RawStr::charged_concat(
            &[
                self.root.as_os_str().as_bytes(),
                b"/",
                stem,
                extension.as_bytes(),
            ],
            &self.meter,
        )?;
        self.outputs
            .write(Path::new(OsStr::from_bytes(path.as_bytes())), write)
    }
}
fn run(args: &[Vec<u8>]) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = None;
    let mut output = None;
    let mut path_id = None;
    let mut tpk = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_slice() {
            b"--path-id" => {
                path_id =
                    Some(std::str::from_utf8(iter.next().ok_or("missing ID")?)?.parse::<i64>()?);
            }
            b"-o" => output = Some(iter.next().ok_or("missing OUTPUT")?),
            b"--tpk" => tpk = Some(iter.next().ok_or("missing TPK")?),
            _ if !arg.starts_with(b"-") && input.is_none() => input = Some(arg),
            _ => return Err("unexpected argument".into()),
        }
    }
    let input = Path::new(OsStr::from_bytes(input.ok_or("missing INPUT")?));
    let output = Path::new(OsStr::from_bytes(output.ok_or("missing OUTPUT")?));
    let path_id = path_id.ok_or("missing --path-id")?;
    let mut options = EnvOptions::unitypy();
    if let Some(tpk) = tpk {
        let mut context = (*options.context).clone();
        context.providers = Arc::new(ProviderChain::new(vec![Arc::new(
            unityrs_tpk::TpkProvider::open(Path::new(OsStr::from_bytes(tpk)))?,
        )]));
        options.context = Arc::new(context);
    }
    let env = Environment::new(options);
    env.load(Source::Path(input.to_owned()))?;
    let mut found = None;
    for object in env.objects() {
        let object = object?;
        if object.path_id() == path_id && matches!(object.class_id(), 43 | 23 | 137) {
            if found.is_some() {
                return Err("ambiguous path id".into());
            }
            found = Some(object);
        }
    }
    let object = found.ok_or("no Mesh or renderer with that path id")?;
    let meter = object.file().meter();
    let mut outputs = Outputs::new(meter);
    if object.class_id() == 43 {
        let m = read_mesh_for_obj(&object, &env, meter)?;
        // Decide False before creating the file.
        let Some(positions) = m.processed.vertices() else {
            return Ok(());
        };
        if m.processed.vertex_count() == 0 || positions.is_empty() {
            return Ok(());
        }
        outputs.write(output, &mut |out| {
            obj::export_mesh_obj(&m, None, out).map(|_| ())
        })?;
    } else {
        std::fs::create_dir_all(output)?;
        obj::export_mesh_renderer(
            &object,
            &env,
            meter,
            &mut Files {
                root: output,
                meter: meter.clone(),
                outputs: &mut outputs,
            },
        )?;
    }
    outputs.finish()?;
    Ok(())
}
pub(super) fn main(args: &[Vec<u8>]) -> ExitCode {
    if matches!(args,[a] if a==b"-h" || a==b"--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let reason = e.to_string();
            let reason = if reason.is_empty() {
                "assertion failed"
            } else {
                &reason
            };
            eprintln!("{USAGE}\nunityrs obj: {reason}");
            ExitCode::from(1)
        }
    }
}
