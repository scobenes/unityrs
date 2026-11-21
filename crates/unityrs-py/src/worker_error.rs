//! Native failures cross the join boundary before Python translation.
use pyo3::{PyErr, PyResult};

pub(crate) enum Failure {
    Core(unityrs_core::Error),
    Save(unityrs_core::files::SaveError),
    Decode(unityrs_texture::DecodeError, i32),
    Write(unityrs_texture::write::WriteError),
    Sprite(unityrs_texture::sprite::SpriteError),
    Audio(unityrs_audio::Error),
    Shader(
        unityrs_export::shader::Error,
        std::sync::Arc<unityrs_core::limits::Meter>,
    ),
    Mesh(unityrs_mesh::MeshError),
    MeshDecode(unityrs_meshdecode::MeshError),
    Parse(unityrs_core::walk::ParseError, bool),
    Static(&'static str, &'static str),
    Message(&'static str, String),
    Integer(std::num::TryFromIntError),
}

impl From<std::num::TryFromIntError> for Failure {
    fn from(error: std::num::TryFromIntError) -> Self {
        Self::Integer(error)
    }
}

impl From<Failure> for PyErr {
    fn from(error: Failure) -> Self {
        error.python()
    }
}

impl Failure {
    pub(crate) fn core(error: unityrs_core::Error) -> Self {
        fn recursive_message(error: &unityrs_core::Error) -> Option<String> {
            use unityrs_core::{Error, env::DerefError, walk::ParseError};
            match error {
                Error::Entry { source, .. } => recursive_message(source),
                Error::AssetBundle(error) => match &**error {
                    ParseError::CheckRead { .. } => Some(crate::errors::message(error)),
                    ParseError::Deref(DerefError::Load(source))
                        if recursive_message(source).is_some() =>
                    {
                        Some(crate::errors::message(error))
                    }
                    _ => None,
                },
                Error::Deref(error) if matches!(&**error, DerefError::Load(source) if recursive_message(source).is_some()) => {
                    Some(crate::errors::message(error))
                }
                _ => None,
            }
        }
        recursive_message(&error).map_or_else(
            || Self::Core(error),
            |message| Self::Message("ValueError", message),
        )
    }

    pub(crate) fn save(error: unityrs_core::files::SaveError) -> Self {
        match error {
            unityrs_core::files::SaveError::Read(error) => Self::core(error),
            error => Self::Save(error),
        }
    }
    pub(crate) fn parse(error: unityrs_core::walk::ParseError, boost: bool) -> Self {
        // CheckRead retains the complete parsed value. Render its native text
        // here so recursive value destruction stays on the advertised stack.
        if matches!(error, unityrs_core::walk::ParseError::CheckRead { .. }) {
            Self::Message("ValueError", crate::errors::message(&error))
        } else {
            Self::Parse(error, boost)
        }
    }

    pub(crate) fn file_parse(
        self,
        data: &[u8],
        stream: bool,
        meter: &std::sync::Arc<unityrs_core::limits::Meter>,
    ) -> PyErr {
        match self {
            Self::Core(error) => crate::errors::file_parse(error, data, stream, meter),
            error => error.into(),
        }
    }

    pub(crate) fn resource(self, path: &unityrs_core::RawStr) -> PyErr {
        match self {
            Self::Core(error) => crate::errors::resource(error, path),
            error => error.into(),
        }
    }
    pub(crate) fn charge(error: unityrs_core::limits::ChargeError) -> Self {
        Self::Core(error.into())
    }

    pub(crate) fn python(self) -> PyErr {
        match self {
            Self::Core(error) => crate::errors::core(error),
            Self::Save(error) => crate::save::error(error),
            Self::Decode(error, format) => crate::images::decode_error(error, format),
            Self::Write(error) => crate::images::write_error(error),
            Self::Sprite(error) => crate::images::sprite_error(error),
            Self::Audio(error) => crate::exports::audio_error(error),
            Self::Shader(error, meter) => crate::exports::shader_error(error, &meter),
            Self::Mesh(error) => crate::exports::mesh::mesh_error(error),
            Self::MeshDecode(error) => crate::exports::mesh::decode_error(error),
            Self::Parse(error, boost) => crate::errors::parse_reader(&error, boost),
            Self::Static(name, text) => crate::exports::named_error(name, text),
            Self::Message(name, text) => crate::exports::named_error(name, &text),
            Self::Integer(error) => crate::errors::new::<pyo3::exceptions::PyOverflowError>(
                &crate::errors::message(&error),
            ),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, Failure>;

pub(crate) fn finish<T>(result: Result<T>) -> PyResult<T> {
    result.map_err(Into::into)
}
