//! Fast Unity asset extraction in Rust, compatible with `UnityPy` 1.25.2.
//!
//! This crate gathers the unityrs library crates behind one name. The
//! reader, [`unityrs_core`], is re-exported at the root: the
//! [`Environment`](env::Environment), files, objects and typetrees. Each other
//! crate is a module behind a feature of the same name:
//!
//! | module | crate | what it does | default |
//! |---|---|---|---|
//! | [`tpk`] | `unityrs-tpk` | typetrees for stripped files from a TPK | yes |
//! | [`texture`] | `unityrs-texture` | `Texture2D` decoders (`encode`: the encoders) | yes |
//! | [`imageio`] | `unityrs-imageio` | PNG, WebP and the other image writers | yes |
//! | [`meshdecode`] | `unityrs-meshdecode` | vertex data, index formats and triangles | yes |
//! | [`mesh`] | `unityrs-mesh` | mesh geometry and OBJ/MTL | yes |
//! | [`scene`] | `unityrs-scene` | `GameObject` and `Transform` graphs | yes |
//! | [`audio`] | `unityrs-audio` | `AudioClip` and FSB5 to WAV (`vorbis`: Vorbis) | yes |
//! | [`anim`] | `unityrs-anim` | animation discovery, binding and sampling | yes |
//! | [`gltf`] | `unityrs-gltf` | the glTF scene model and GLB writer | yes |
//! | [`export`] | `unityrs-export` | sprites, the NGUI cutter and the extractor tree | yes |
//! | `batch` | `unityrs-batch` | the batch engine behind `unityrs batch` | no |
//!
//! Every input is metered: allocations are charged to a budget before they
//! are made, and limits refuse hostile input with a typed error.
//!
//! ```no_run
//! use std::path::Path;
//! use unityrs::env::{EnvOptions, Environment};
//!
//! fn main() -> Result<(), unityrs::Error> {
//!     let env = Environment::new(EnvOptions::unitypy());
//!     env.load_path(Path::new("level0.bundle"))?;
//!     for object in env.objects() {
//!         let object = object?;
//!         println!("path id {} class id {}", object.path_id(), object.class_id());
//!     }
//!     Ok(())
//! }
//! ```
#![forbid(unsafe_code)]

pub use unityrs_core::*;

#[cfg(feature = "anim")]
pub use unityrs_anim as anim;
#[cfg(feature = "audio")]
pub use unityrs_audio as audio;
#[cfg(feature = "batch")]
pub use unityrs_batch as batch;
#[cfg(feature = "export")]
pub use unityrs_export as export;
#[cfg(feature = "gltf")]
pub use unityrs_gltf as gltf;
#[cfg(feature = "imageio")]
pub use unityrs_imageio as imageio;
#[cfg(feature = "mesh")]
pub use unityrs_mesh as mesh;
#[cfg(feature = "meshdecode")]
pub use unityrs_meshdecode as meshdecode;
#[cfg(feature = "scene")]
pub use unityrs_scene as scene;
#[cfg(feature = "texture")]
pub use unityrs_texture as texture;
#[cfg(feature = "tpk")]
pub use unityrs_tpk as tpk;
