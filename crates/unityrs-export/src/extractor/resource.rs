//! Inspect terminal typed errors, including lazy input reads. Tentative
//! vector growth refusals may recover and must not make a run fail.
use super::Error;
use crate::{audio, pyjson, shader, simple};
use unityrs_core::{
    env::DerefError,
    typetree::TreeError,
    walk::{ParseError, ReadDetail},
};

enum Fault<'a> {
    Core(&'a unityrs_core::Error),
    Parse(&'a ParseError),
    Deref(&'a DerefError),
    Tree(&'a TreeError),
}
fn resource(mut fault: Fault<'_>) -> bool {
    loop {
        fault = match fault {
            Fault::Core(e) => match e {
                unityrs_core::Error::Limit(_)
                | unityrs_core::Error::Budget(_)
                | unityrs_core::Error::Allocator(_) => return true,
                unityrs_core::Error::Entry { source, .. } => Fault::Core(source),
                unityrs_core::Error::AssetBundle(p) => Fault::Parse(p),
                unityrs_core::Error::Deref(d) => Fault::Deref(d),
                unityrs_core::Error::Tree(t) => Fault::Tree(t),
                unityrs_core::Error::Detect(d) => {
                    return matches!(
                        d,
                        unityrs_core::detect::DetectError::InflateLimit(_)
                            | unityrs_core::detect::DetectError::Work(_)
                            | unityrs_core::detect::DetectError::Allocator(_)
                    );
                }
                _ => return false,
            },
            Fault::Parse(p) => match p {
                ParseError::Read(r) => match &r.detail {
                    ReadDetail::Limit(_) | ReadDetail::Charge(_) => return true,
                    ReadDetail::Tree(t) => Fault::Tree(t),
                    _ => return false,
                },
                ParseError::Tree(t) => Fault::Tree(t),
                ParseError::Deref(d) => Fault::Deref(d),
                ParseError::Provider(p) => {
                    return p.limit.is_some() || p.memory.is_some() || p.allocator.is_some();
                }
                _ => return false,
            },
            Fault::Deref(d) => match d {
                DerefError::Load(e) => Fault::Core(e),
                _ => return false,
            },
            Fault::Tree(t) => return matches!(t, TreeError::Limit(_) | TreeError::Charge(_)),
        };
    }
}
fn simple(e: &simple::Error) -> bool {
    match e {
        simple::Error::Charge(_) | simple::Error::Json(pyjson::WriteError::Charge(_)) => true,
        simple::Error::Core(e) => resource(Fault::Core(e)),
        simple::Error::Parse(e) => resource(Fault::Parse(e)),
        _ => false,
    }
}
fn mesh_decode(e: &unityrs_meshdecode::MeshError) -> bool {
    match e {
        unityrs_meshdecode::MeshError::Charge(_) => true,
        unityrs_meshdecode::MeshError::Resource(e) => resource(Fault::Core(e)),
        _ => false,
    }
}
pub(super) fn refused(e: &Error) -> bool {
    match e {
        Error::Charge(_) => true,
        Error::Core(e) => resource(Fault::Core(e)),
        Error::Deref(e) => resource(Fault::Deref(e)),
        Error::Simple(e) | Error::Audio(audio::Error::Read(e)) => simple(e),
        Error::Audio(audio::Error::Audio(e)) => matches!(e, unityrs_audio::Error::Charge(_)),
        Error::Shader(e) => match e {
            shader::Error::Charge(_) => true,
            shader::Error::Core(e) => resource(Fault::Core(e)),
            shader::Error::Parse(e) => resource(Fault::Parse(e)),
            _ => false,
        },
        Error::Mesh(e) => match e {
            // Raw mesh extraction never constructs a scene; keep any
            // unexpected scene failure terminal rather than hiding it.
            unityrs_mesh::MeshError::Charge(_) | unityrs_mesh::MeshError::Scene(_) => true,
            unityrs_mesh::MeshError::Core(e) => resource(Fault::Core(e)),
            unityrs_mesh::MeshError::Parse(e) => resource(Fault::Parse(e)),
            unityrs_mesh::MeshError::Deref(e) => resource(Fault::Deref(e)),
            unityrs_mesh::MeshError::Decode(e) => mesh_decode(e),
            unityrs_mesh::MeshError::Texture(e) => {
                matches!(e, unityrs_texture::DecodeError::Charge(_))
            }
            unityrs_mesh::MeshError::View(e) => {
                matches!(e, unityrs_core::objmode::ViewError::Charge(_))
            }
            _ => false,
        },
        Error::MeshDecode(e) => mesh_decode(e),
        Error::Decode(e) => matches!(e, unityrs_texture::DecodeError::Charge(_)),
        Error::Sprite(e) => match e {
            unityrs_texture::sprite::SpriteError::Charge(_) => true,
            unityrs_texture::sprite::SpriteError::Mesh(e) => mesh_decode(e),
            _ => false,
        },
        Error::Fetch(e) => match e {
            unityrs_texture::fetch::Error::Charge(_) => true,
            unityrs_texture::fetch::Error::Data(e)
            | unityrs_texture::fetch::Error::Resource { error: e, .. } => resource(Fault::Core(e)),
            _ => false,
        },
        Error::Image(unityrs_imageio::writers::SaveError::Encode(e)) => matches!(
            e,
            unityrs_imageio::EncodeError::Refused { .. }
                | unityrs_imageio::EncodeError::Alloc { .. }
                | unityrs_imageio::EncodeError::ZlibMemory
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unityrs_core::{LimitError, LimitKind, limits::ChargeError};
    #[test]
    fn retained_refusals_keep_their_type_through_input_and_export_wrappers() {
        let limit = LimitError::refused(LimitKind::DecodeWork, 2, 1);
        let parse = ParseError::Read(unityrs_core::walk::ReadError {
            kind: unityrs_core::walk::ReadErrorKind::Other,
            at: 0,
            detail: ReadDetail::Limit(limit),
        });
        for error in [
            Error::Charge(ChargeError::Limit(limit)),
            Error::Simple(simple::Error::Parse(parse.clone())),
            Error::Shader(shader::Error::Parse(parse.clone())),
            Error::Mesh(unityrs_mesh::MeshError::Parse(parse)),
            Error::MeshDecode(unityrs_meshdecode::MeshError::Charge(limit.into())),
        ] {
            assert!(refused(&error), "{error:?}");
        }
        for error in [
            Error::UnsafePath,
            Error::Field("bad input"),
            Error::Simple(simple::Error::Field("bad input")),
            Error::MeshDecode(unityrs_meshdecode::MeshError::NoAttribute("m_VertexData")),
        ] {
            assert!(!refused(&error), "{error:?}");
        }
    }
}
