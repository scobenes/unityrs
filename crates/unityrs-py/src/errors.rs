// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py, UnityPy/files/BundleFile.py, UnityPy/files/WebFile.py, UnityPy/streams/EndianBinaryReader.py, UnityPy/helpers/TypeTreeHelper.py, UnityPy/helpers/ImportHelper.py, UnityPy/helpers/ResourceReader.py. See THIRD_PARTY_LICENSES.
//! Python exception classes for the corresponding core failures.
use pyo3::{
    create_exception,
    exceptions::{
        PyAssertionError, PyAttributeError, PyEOFError, PyException, PyFileNotFoundError,
        PyIndexError, PyIsADirectoryError, PyKeyError, PyMemoryError, PyNotImplementedError,
        PyOSError, PyOverflowError, PyPermissionError, PyRecursionError, PyTypeError, PyValueError,
    },
    prelude::*,
};
use unityrs_core::{
    Error,
    detect::DetectError,
    once::CellError,
    walk::{ParseError, ReadDetail, ReadErrorKind},
};

create_exception!(unityrs, LimitError, PyMemoryError);
create_exception!(unityrs, TypeTreeError, PyException);

pub(crate) fn charge(error: unityrs_core::limits::ChargeError) -> PyErr {
    core(error.into())
}

/// Reader exceptions at the direct helper boundary. The core retains a named
/// short-read error; Python exposes the reader's original exception text.
pub(crate) fn file_parse(
    mut error: Error,
    data: &[u8],
    stream: bool,
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
) -> PyErr {
    while let Error::Entry { source, .. } = error {
        error = (*source).clone();
    }
    if let Error::WebSignature(signature) = &error {
        return signature_error(
            signature.as_bytes(),
            meter,
            false,
            "Invalid WebFile signature: {!r}. Expected 'UnityWebData' or 'TuanjieWebData'.",
        );
    }
    if let Error::BundleSignature(signature) = &error {
        return signature_error(
            signature.as_bytes(),
            meter,
            true,
            "Unknown Bundle signature: {}",
        );
    }
    if let Error::Malformed {
        what: unityrs_core::stream::STREAM_READER,
        detail,
    } = &error
    {
        use unityrs_core::stream::StreamRefusal;
        for refusal in [
            StreamRefusal::NegativePosition,
            StreamRefusal::PositionTooLarge,
            StreamRefusal::NegativeRead,
            StreamRefusal::ReadTooLarge,
            StreamRefusal::ReadSizeOverflow,
        ] {
            if detail.as_ref() == refusal.message() {
                return exception(refusal.exception(), message(detail));
            }
        }
    }
    let template = match &error {
        Error::Malformed {
            what: "WebFile",
            detail,
        } if detail.as_ref() == "invalid signature" => Some((
            false,
            "Invalid WebFile signature: {!r}. Expected 'UnityWebData' or 'TuanjieWebData'.",
        )),
        _ => None,
    };
    if let Some((bundle, template)) = template {
        return file_signature(data, stream, meter, bundle, template);
    }
    if let Error::OutOfBounds {
        what,
        pos,
        need,
        reader,
        length,
        ..
    } = error
    {
        return file_parse_bounds(
            what,
            pos,
            need,
            length,
            reader != unityrs_core::stream::ReaderKind::Memory,
            meter,
        );
    }
    core(error)
}

fn file_parse_bounds(
    what: &'static str,
    pos: i128,
    need: u64,
    length: usize,
    stream: bool,
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
) -> PyErr {
    if what == "string to null" {
        return if stream {
            PyValueError::new_err("Unterminated string: [b'']")
        } else {
            PyException::new_err("String not terminated")
        };
    }
    // Fixed-width numeric fields bound this message well below 512 bytes.
    let _staging = match meter.charge(512) {
        Ok(charge) => charge,
        Err(error) => return charge(error),
    };
    let text = if stream || need == 1 {
        message(&format_args!("unpack requires a buffer of {need} bytes"))
    } else if pos < 0 && pos.saturating_add(i128::from(need)) > 0 {
        message(&format_args!(
            "not enough data to unpack {need} bytes at offset {pos}"
        ))
    } else if pos < 0 && pos < -(length as i128) {
        message(&format_args!(
            "offset {pos} out of range for {length}-byte buffer"
        ))
    } else {
        let pos = if pos < 0 { pos + length as i128 } else { pos };
        let end = pos.saturating_add(i128::from(need));
        message(&format_args!(
            "unpack_from requires a buffer of at least {end} bytes for unpacking {need} bytes at offset {pos} (actual buffer size is {length})"
        ))
    };
    Python::attach(|py| {
        match py
            .import("struct")
            .and_then(|module| module.getattr("error"))
        {
            Ok(class) => match class.cast_into::<pyo3::types::PyType>() {
                Ok(class) => PyErr::from_type(class, (text,)),
                Err(error) => error.into(),
            },
            Err(error) => error,
        }
    })
}

fn file_signature(
    data: &[u8],
    stream: bool,
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
    bundle: bool,
    template: &str,
) -> PyErr {
    if let Err(error) = meter.count_decode(data.len().min(32767) as u64) {
        return charge(error);
    }
    let mut reader = unityrs_core::stream::Reader::with_kind(
        data,
        unityrs_core::stream::Endian::Big,
        if stream {
            unityrs_core::stream::ReaderKind::Stream
        } else {
            unityrs_core::stream::ReaderKind::Memory
        },
    );
    let signature = match reader.read_string_to_null() {
        Ok(signature) => signature,
        Err(error) => return core(error),
    };
    signature_error(signature, meter, bundle, template)
}

fn signature_error(
    signature: &[u8],
    meter: &std::sync::Arc<unityrs_core::limits::Meter>,
    bundle: bool,
    template: &str,
) -> PyErr {
    let _staging = match meter.charge(
        (signature.len() as u64)
            .saturating_mul(16)
            .saturating_add(1024),
    ) {
        Ok(charge) => charge,
        Err(error) => return charge(error),
    };
    Python::attach(|py| {
        let rendered = (|| {
            let signature = crate::python::string(signature)?;
            let text =
                pyo3::types::PyString::new(py, template).call_method1("format", (signature,))?;
            let class = if bundle {
                py.get_type::<PyNotImplementedError>()
            } else {
                py.get_type::<PyValueError>()
            };
            Ok::<_, PyErr>(PyErr::from_type(class, (text.unbind(),)))
        })();
        rendered.unwrap_or_else(|error| error)
    })
}

pub(crate) fn resource(error: Error, path: &unityrs_core::RawStr) -> PyErr {
    if matches!(error, Error::NotFound(_)) {
        Python::attach(|py| {
            let name = py
                .import("ntpath")?
                .getattr("basename")?
                .call1((crate::python::string(path.as_bytes())?,))?;
            let message = pyo3::types::PyString::new(py, "Resource file {} not found")
                .call_method1("format", (name,))?;
            Ok::<_, PyErr>(PyFileNotFoundError::new_err(message.unbind()))
        })
        .unwrap_or_else(|e| e)
    } else {
        core(error)
    }
}

pub(crate) fn core(error: Error) -> PyErr {
    if error.is_meter_refusal() {
        return match error {
            Error::Limit(_) => LimitError::new_err(message(&error)),
            _ => PyMemoryError::new_err(message(&error)),
        };
    }
    match error {
        Error::Entry { source, .. } => core((*source).clone()),
        Error::AssetBundle(source) => parse((*source).clone()),
        Error::Detect(DetectError::Unterminated) => PyException::new_err("String not terminated"),
        Error::Detect(DetectError::UnterminatedStream) => {
            PyValueError::new_err("Unterminated string: [b'']")
        }
        Error::Detect(DetectError::InflateLimit(_) | DetectError::Work(_)) | Error::Limit(_) => {
            LimitError::new_err(message(&error))
        }
        Error::Detect(DetectError::Allocator(_)) | Error::Allocator(_) | Error::Budget(_) => {
            PyMemoryError::new_err(message(&error))
        }
        Error::Detect(DetectError::GzipCrc {
            expected, actual, ..
        }) => core(
            DetectError::Gzip(
                format!("BadGzipFile: CRC check failed {expected:#x} != {actual:#x}").into(),
            )
            .into(),
        ),
        Error::Detect(DetectError::Gzip(detail)) => gzip(&detail),
        Error::OutOfBounds { .. } => {
            Python::attach(
                |py| match py.import("struct").and_then(|m| m.getattr("error")) {
                    Ok(cls) => PyErr::from_type(
                        cls.cast_into()
                            .unwrap_or_else(|_| py.get_type::<PyEOFError>()),
                        (message(&error),),
                    ),
                    Err(e) => e,
                },
            )
        }
        Error::Io(e) => {
            let message = message(&e.0);
            match e.0.kind() {
                std::io::ErrorKind::NotFound => PyFileNotFoundError::new_err(message),
                std::io::ErrorKind::PermissionDenied => PyPermissionError::new_err(message),
                std::io::ErrorKind::IsADirectory => PyIsADirectoryError::new_err(message),
                _ => PyOSError::new_err(message),
            }
        }
        Error::NotFound(_) => PyFileNotFoundError::new_err(message(&error)),
        Error::TypeIndex { .. } => PyIndexError::new_err(message(&error)),
        Error::Tree(
            unityrs_core::typetree::TreeError::Empty
            | unityrs_core::typetree::TreeError::LevelUnderflow { .. },
        ) => PyIndexError::new_err("list index out of range"),
        Error::Malformed {
            what: "HEAD typetree",
            ref detail,
        } if detail.as_ref() == "shared subtree hash not found" => {
            PyKeyError::new_err(message(&error))
        }
        Error::Malformed {
            what: "HEAD typetree",
            ref detail,
        } if detail.as_ref() == "shared hash index out of range" => {
            PyIndexError::new_err("list index out of range")
        }
        Error::Malformed {
            what: "HEAD typetree",
            ref detail,
        } if detail.as_ref() == "cyclic shared subtree" => {
            PyRecursionError::new_err("maximum recursion depth exceeded")
        }
        Error::Version(unityrs_core::version::VersionError::NoFallback) => Python::attach(|py| {
            match py
                .import("unityrs.exceptions")
                .and_then(|m| m.getattr("UnityVersionFallbackError"))
            {
                Ok(cls) => match cls.cast_into::<pyo3::types::PyType>() {
                    Ok(cls) => PyErr::from_type(cls, (message(&error),)),
                    Err(error) => error.into(),
                },
                Err(error) => error,
            }
        }),
        Error::Version(unityrs_core::version::VersionError::NoMatch(ref raw)) => {
            invalid_version(raw.as_bytes())
        }
        _ => PyValueError::new_err(message(&error)),
    }
}

fn invalid_version(raw: &[u8]) -> PyErr {
    let prefix = b"Invalid version string: ";
    let mut bytes = Vec::new();
    let Some(len) = prefix.len().checked_add(raw.len()) else {
        return PyMemoryError::new_err("version error message is too large");
    };
    if bytes.try_reserve_exact(len).is_err() {
        return PyMemoryError::new_err("version error message allocation refused");
    }
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(raw);
    match crate::python::string(&bytes) {
        Ok(message) => {
            Python::attach(|py| PyErr::from_type(py.get_type::<PyValueError>(), (message,)))
        }
        Err(error) => error,
    }
}

fn gzip(detail: &str) -> PyErr {
    Python::attach(|py| {
        let text = detail.split_once(": ").map_or(detail, |(_, s)| s);
        let text = if detail.starts_with("zlib.error:") {
            message(&format_args!(
                "Error -3 while decompressing data: {}",
                text.strip_prefix("deflate decompression error: ")
                    .unwrap_or(text)
            ))
        } else {
            message(&text)
        };
        let (module, name) = if detail.starts_with("EOFError:") {
            return PyEOFError::new_err(text);
        } else if detail.starts_with("zlib.error:") {
            ("zlib", "error")
        } else {
            ("gzip", "BadGzipFile")
        };
        match py
            .import(module)
            .and_then(|m| m.getattr(name))
            .and_then(|c| c.cast_into::<pyo3::types::PyType>().map_err(Into::into))
        {
            Ok(cls) => PyErr::from_type(cls, (text,)),
            Err(error) => error,
        }
    })
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err consumes the core error"
)]
pub(crate) fn parse(error: ParseError) -> PyErr {
    parse_reader(&error, true)
}

pub(crate) fn parse_reader(error: &ParseError, boost: bool) -> PyErr {
    // files/ObjectReader.py:244-262, files/ObjectReader.py:285-307;
    // helpers/TypeTreeHelper.py:123-158. Boost's short read is EOFError.
    if error.is_meter_refusal() {
        return PyMemoryError::new_err(message(&error));
    }
    match &error {
        ParseError::NoTypeTree {
            why: Some(why),
            class_id,
        } => match why.exception() {
            "KeyError" => PyKeyError::new_err(*class_id),
            "ValueError" => PyValueError::new_err("Could not find exact version"),
            "AssertionError" => PyAssertionError::new_err(()),
            "NotImplementedError" => PyNotImplementedError::new_err("Unsupported comparison"),
            _ => TypeTreeError::new_err("There are no TypeTree nodes for this object."),
        },
        ParseError::NoTypeTree { .. } => {
            TypeTreeError::new_err("There are no TypeTree nodes for this object.")
        }
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Limit(_) => LimitError::new_err(message(&error)),
            ReadDetail::RefTypeNoTree => PyAttributeError::new_err(message(&r.detail)),
            ReadDetail::NoRefTypes {
                reader: unityrs_core::value::ReadBy::Boost,
                ..
            } => PyValueError::new_err("Failed to get ref type node"),
            ReadDetail::NoRefTypes { absent: true, .. } => {
                PyAttributeError::new_err("'SerializedFile' object has no attribute 'ref_types'")
            }
            ReadDetail::NoRefTypes { .. } => {
                PyValueError::new_err("SerializedFile has no ref_types")
            }
            ReadDetail::Stream(s) => exception(s.exception(), message(s)),
            ReadDetail::BoostEof {
                method,
                exception: kind,
                ..
            } => exception(kind, format!("{method} out of bounds")),
            ReadDetail::Eof { need, .. } => Python::attach(|py| {
                match py
                    .import("struct")
                    .and_then(|m| m.getattr("error"))
                    .and_then(|c| c.cast_into::<pyo3::types::PyType>().map_err(Into::into))
                {
                    Ok(cls) => PyErr::from_type(
                        cls,
                        (format!("unpack requires a buffer of {need} bytes"),),
                    ),
                    Err(error) => error,
                }
            }),
            ReadDetail::NegativeLength(_) if boost => {
                PyValueError::new_err("Negative length read from TypeTree")
            }
            ReadDetail::Python(s) => {
                let (name, text) = s.split_once(": ").unwrap_or((s, s));
                // Core diagnostics render a missing reference key with quotes;
                // KeyError itself adds those quotes around its argument.
                let text = if name == "KeyError" {
                    text.strip_prefix('\'')
                        .and_then(|key| key.strip_suffix('\''))
                        .unwrap_or(text)
                } else {
                    text
                };
                exception(name, message(&text))
            }
            _ if r.kind == ReadErrorKind::OutOfBounds => PyEOFError::new_err(message(&r.detail)),
            _ => PyValueError::new_err(message(&r.detail)),
        },
        _ => PyValueError::new_err(message(&error)),
    }
}
fn exception(name: &str, message: String) -> PyErr {
    match name {
        "IndexError" => PyIndexError::new_err(message),
        "AttributeError" => PyAttributeError::new_err(message),
        "KeyError" => PyKeyError::new_err(message),
        "TypeError" => PyTypeError::new_err(message),
        "EOFError" => PyEOFError::new_err(message),
        "OverflowError" => PyOverflowError::new_err(message),
        "OSError" => PyOSError::new_err(message),
        _ => PyValueError::new_err(message),
    }
}

pub(crate) fn stream(name: &str) -> PyErr {
    exception(name, "invalid stream position or length".into())
}

// Exception rendering must not turn an allocation refusal into an abort.
pub(crate) fn message(value: &impl std::fmt::Display) -> String {
    use std::fmt::Write;
    struct Buffer(String);
    impl Write for Buffer {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0
                .try_reserve(text.len())
                .map_err(|_| std::fmt::Error)?;
            self.0.push_str(text);
            Ok(())
        }
    }
    let mut out = Buffer(String::new());
    let _ = write!(out, "{value}");
    out.0
}
