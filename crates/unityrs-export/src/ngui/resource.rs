//! Shared texture fetching, with each core call under the cutter's own
//! refusal probe and the Environment call under its eviction observation.

use std::sync::Arc;

use unityrs_core::Bytes;
use unityrs_core::files::SerializedFile;
use unityrs_texture::fetch::{self, Error, Hooks};

use super::classify::{self, Class, Reason};
use super::read::Texture;
use super::seam::Seam;

fn classified(error: Error) -> Class {
    match error {
        Error::Charge(error) => classify::charge(&error),
        Error::Data(error) => classify::error(&error),
        Error::Resource { error, path } => classify::resource(&error, &path),
        Error::Serialized => Class::Skip(Reason::decode_error("malformed")),
        Error::Web => Class::Skip(Reason::decode_error("unsupported")),
    }
}

impl Hooks for Seam<'_> {
    type Error = Class;

    fn probe<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Class> {
        Seam::probe(self, || f().map_err(classified))
    }

    fn env_call<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Class> {
        Seam::env_call(self, || f().map_err(classified))
    }

    fn row_error(&self, error: Error) -> Class {
        classified(error)
    }
}

/// The texture's data, empty without inline bytes or a nonempty stream path.
pub(super) fn data(
    seam: &Seam<'_>,
    from: &Arc<SerializedFile>,
    t: &Texture,
) -> Result<Bytes, Class> {
    fetch::fetch_with(
        seam.env(),
        from,
        &t.image_data,
        t.stream.as_ref(),
        seam.meter,
        seam,
    )
}
