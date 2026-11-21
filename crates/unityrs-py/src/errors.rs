// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py, UnityPy/files/BundleFile.py, UnityPy/files/WebFile.py, UnityPy/streams/EndianBinaryReader.py, UnityPy/helpers/TypeTreeHelper.py, UnityPy/helpers/ImportHelper.py, UnityPy/helpers/ResourceReader.py. See THIRD_PARTY_LICENSES.
//! Python exception classes for the corresponding core failures.
use crate::direct_keys::name;
use pyo3::PyTypeInfo;
use pyo3::{
    create_exception,
    exceptions::{
        PyAssertionError, PyAttributeError, PyEOFError, PyException, PyFileNotFoundError,
        PyIndexError, PyIsADirectoryError, PyKeyError, PyMemoryError, PyNotImplementedError,
        PyOSError, PyOverflowError, PyPermissionError, PyRecursionError, PyTypeError, PyValueError,
    },
    ffi,
    prelude::*,
    types::PyType,
};
use unityrs_core::{
    Error,
    detect::DetectError,
    once::CellError,
    walk::{ParseError, ReadDetail, ReadErrorKind},
};

create_exception!(unityrs, LimitError, PyMemoryError);
create_exception!(unityrs, TypeTreeError, PyException);

/// `T(text)`, its message made now. PyO3's `new_err` converts its argument
/// as the error is raised, with constructors that panic on a failed
/// allocation (a `PanicException` where Python raises `MemoryError`); here
/// a failed allocation is the `MemoryError` it raised ([`no_memory`]).
pub(crate) fn new<T: PyTypeInfo>(text: impl AsRef<str>) -> PyErr {
    let text = text.as_ref();
    crate::python::relay::attach(|py| typed(&py.get_type::<T>(), text))
}

/// `T(arg)`, `arg` made now by `make`; its failure raised as it is.
pub(crate) fn new_with<T: PyTypeInfo>(
    make: impl for<'py> FnOnce(Python<'py>) -> PyResult<Bound<'py, PyAny>> + Send,
) -> PyErr {
    crate::python::relay::attach(|py| match make(py) {
        Ok(arg) => PyErr::from_type(py.get_type::<T>(), arg.unbind()),
        Err(error) => error,
    })
}

/// `class(text)`, attached, as [`new`] makes it. The argument is the one
/// `str`, never a tuple of it: raising it allocates nothing more of ours.
pub(crate) fn typed(class: &Bound<'_, PyType>, text: &str) -> PyErr {
    match text_of(class.py(), text) {
        Ok(text) => PyErr::from_type(class.clone(), text.unbind()),
        Err(error) => error,
    }
}

/// A new `str` of `text`, a failed allocation raised ([`no_memory`]).
pub(crate) fn text_of<'py>(py: Python<'py>, text: &str) -> PyResult<Bound<'py, PyAny>> {
    let Ok(len) = isize::try_from(text.len()) else {
        return Err(no_memory(py));
    };
    // SAFETY: the bytes are borrowed for the call; a new reference or null
    // with an exception set.
    let made = unsafe { ffi::PyUnicode_FromStringAndSize(text.as_ptr().cast(), len) };
    if made.is_null() {
        return Err(no_memory(py));
    }
    // SAFETY: a new, non-null reference.
    Ok(unsafe { Bound::from_owned_ptr(py, made) })
}

/// The `MemoryError` a failed allocation set, taken; with none pending, a
/// `MemoryError` whose raising allocates nothing of ours: its argument is
/// the interpreter's cached empty tuple. Its instance comes from `CPython`'s
/// `MemoryError` free list, which is finite: once those preallocated
/// instances are all in use, `CPython` allocates a new one, and if that fails
/// too, `CPython` raises its own `MemoryError`, never a panic.
pub(crate) fn no_memory(py: Python<'_>) -> PyErr {
    PyErr::take(py).unwrap_or_else(|| PyMemoryError::new_err(()))
}

/// `module.name`, a class, its names made fallibly (`builtins.KeyError`).
pub(crate) fn class_named<'py>(
    py: Python<'py>,
    module: &str,
    name: &str,
) -> PyResult<Bound<'py, PyType>> {
    let module = text_of(py, module)?.cast_into::<pyo3::types::PyString>()?;
    let name = text_of(py, name)?.cast_into::<pyo3::types::PyString>()?;
    class_of(py, &module, &name)
}

/// A failed integer narrowing as PyO3 maps it (`ValueError` with the
/// error's text), its message made now ([`new`]).
#[allow(clippy::needless_pass_by_value, reason = "used as a map_err function")]
pub(crate) fn narrowing(error: std::num::TryFromIntError) -> PyErr {
    new::<PyValueError>(message(&error))
}

/// A module's class by interned names (`struct.error`).
fn class_of<'py>(
    py: Python<'py>,
    module: &Bound<'py, pyo3::types::PyString>,
    name: &Bound<'py, pyo3::types::PyString>,
) -> PyResult<Bound<'py, PyType>> {
    Ok(crate::direct_keys::import_attr(py, module, name)?.cast_into::<PyType>()?)
}

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
            new::<PyValueError>("Unterminated string: [b'']")
        } else {
            new::<PyException>("String not terminated")
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
    crate::python::relay::attach(|py| match struct_error(py) {
        Ok(class) => typed(&class, &text),
        Err(error) => error,
    })
}

/// `struct.error`.
fn struct_error(py: Python<'_>) -> PyResult<Bound<'_, PyType>> {
    class_of(py, name!(py, c"struct")?, name!(py, c"error")?)
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
    crate::python::relay::attach(|py| {
        let rendered = (|| {
            let signature = crate::python::held::string(py, signature)?;
            let text = format(py, template, &signature)?;
            let class = if bundle {
                py.get_type::<PyNotImplementedError>()
            } else {
                py.get_type::<PyValueError>()
            };
            Ok::<_, PyErr>(PyErr::from_type(class, text.unbind()))
        })();
        rendered.unwrap_or_else(|error| error)
    })
}

pub(crate) fn resource(error: Error, path: &unityrs_core::RawStr) -> PyErr {
    if matches!(error, Error::NotFound(_)) {
        crate::python::relay::attach(|py| {
            let basename = crate::direct_keys::import_attr(
                py,
                name!(py, c"ntpath")?,
                name!(py, c"basename")?,
            )?;
            let name = crate::python::held::call1(
                &basename,
                &crate::python::held::string(py, path.as_bytes())?,
            )?;
            let message = format(py, "Resource file {} not found", &name)?;
            Ok::<_, PyErr>(PyFileNotFoundError::new_err(message.unbind()))
        })
        .unwrap_or_else(|e| e)
    } else {
        core(error)
    }
}

/// `template.format(arg)`, every object made fallibly.
fn format<'py>(
    py: Python<'py>,
    template: &str,
    arg: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let method = text_of(py, template)?.getattr(name!(py, c"format")?)?;
    crate::python::held::call1(&method, arg)
}

pub(crate) fn core(error: Error) -> PyErr {
    if error.is_meter_refusal() {
        return match error {
            Error::Limit(_) => new::<LimitError>(message(&error)),
            _ => new::<PyMemoryError>(message(&error)),
        };
    }
    match error {
        Error::Entry { source, .. } => core((*source).clone()),
        Error::AssetBundle(source) => parse((*source).clone()),
        Error::Detect(DetectError::Unterminated) => new::<PyException>("String not terminated"),
        Error::Detect(DetectError::UnterminatedStream) => {
            new::<PyValueError>("Unterminated string: [b'']")
        }
        Error::Detect(DetectError::InflateLimit(_) | DetectError::Work(_)) | Error::Limit(_) => {
            new::<LimitError>(message(&error))
        }
        Error::Detect(DetectError::Allocator(_)) | Error::Allocator(_) | Error::Budget(_) => {
            new::<PyMemoryError>(message(&error))
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
        Error::OutOfBounds { .. } => crate::python::relay::attach(|py| {
            let class = crate::direct_keys::import_attr(
                py,
                match name!(py, c"struct") {
                    Ok(module) => module,
                    Err(error) => return error,
                },
                match name!(py, c"error") {
                    Ok(name) => name,
                    Err(error) => return error,
                },
            );
            match class {
                Ok(cls) => typed(
                    &cls.cast_into()
                        .unwrap_or_else(|_| py.get_type::<PyEOFError>()),
                    &message(&error),
                ),
                Err(e) => e,
            }
        }),
        Error::Io(e) => {
            let message = message(&e.0);
            match e.0.kind() {
                std::io::ErrorKind::NotFound => new::<PyFileNotFoundError>(message),
                std::io::ErrorKind::PermissionDenied => new::<PyPermissionError>(message),
                std::io::ErrorKind::IsADirectory => new::<PyIsADirectoryError>(message),
                _ => new::<PyOSError>(message),
            }
        }
        Error::NotFound(_) => new::<PyFileNotFoundError>(message(&error)),
        Error::TypeIndex { .. } => new::<PyIndexError>(message(&error)),
        Error::Tree(
            unityrs_core::typetree::TreeError::Empty
            | unityrs_core::typetree::TreeError::LevelUnderflow { .. },
        ) => new::<PyIndexError>("list index out of range"),
        Error::Malformed {
            what: "HEAD typetree",
            ref detail,
        } if detail.as_ref() == "shared subtree hash not found" => {
            new::<PyKeyError>(message(&error))
        }
        Error::Malformed {
            what: "HEAD typetree",
            ref detail,
        } if detail.as_ref() == "shared hash index out of range" => {
            new::<PyIndexError>("list index out of range")
        }
        Error::Malformed {
            what: "HEAD typetree",
            ref detail,
        } if detail.as_ref() == "cyclic shared subtree" => {
            new::<PyRecursionError>("maximum recursion depth exceeded")
        }
        Error::Version(unityrs_core::version::VersionError::NoFallback) => {
            crate::python::relay::attach(|py| {
                match class_of(
                    py,
                    name!(py, c"unityrs.exceptions")?,
                    name!(py, c"UnityVersionFallbackError")?,
                ) {
                    Ok(cls) => Ok(typed(&cls, &message(&error))),
                    Err(error) => Err(error),
                }
            })
            .unwrap_or_else(|error| error)
        }
        Error::Version(unityrs_core::version::VersionError::NoMatch(ref raw)) => {
            invalid_version(raw.as_bytes())
        }
        _ => new::<PyValueError>(message(&error)),
    }
}

fn invalid_version(raw: &[u8]) -> PyErr {
    let prefix = b"Invalid version string: ";
    let mut bytes = Vec::new();
    let Some(len) = prefix.len().checked_add(raw.len()) else {
        return new::<PyMemoryError>("version error message is too large");
    };
    if bytes.try_reserve_exact(len).is_err() {
        return new::<PyMemoryError>("version error message allocation refused");
    }
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(raw);
    match crate::python::string(&bytes) {
        Ok(message) => crate::python::relay::attach(|py| {
            PyErr::from_type(py.get_type::<PyValueError>(), message)
        }),
        Err(error) => error,
    }
}

fn gzip(detail: &str) -> PyErr {
    crate::python::relay::attach(|py| {
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
        let class = if detail.starts_with("EOFError:") {
            Ok(py.get_type::<PyEOFError>())
        } else if detail.starts_with("zlib.error:") {
            name!(py, c"zlib").and_then(|m| class_of(py, m, name!(py, c"error")?))
        } else {
            name!(py, c"gzip").and_then(|m| class_of(py, m, name!(py, c"BadGzipFile")?))
        };
        match class {
            Ok(cls) => typed(&cls, &text),
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
        return new::<PyMemoryError>(message(&error));
    }
    match error {
        ParseError::Tree(unityrs_core::typetree::TreeError::Charge(error))
        | ParseError::Read(unityrs_core::walk::ReadError {
            detail: ReadDetail::Charge(error),
            ..
        }) => {
            return charge(error.clone());
        }
        _ => {}
    }
    match &error {
        ParseError::NoTypeTree {
            why: Some(why),
            class_id,
        } => match why.exception() {
            "KeyError" => {
                let class_id = i64::from(*class_id);
                new_with::<PyKeyError>(move |py| crate::python::held::int(py, class_id))
            }
            "ValueError" => new::<PyValueError>("Could not find exact version"),
            // `()` is the interpreter's empty tuple: raising it allocates
            // nothing of ours.
            "AssertionError" => PyAssertionError::new_err(()),
            "NotImplementedError" => new::<PyNotImplementedError>("Unsupported comparison"),
            _ => new::<TypeTreeError>("There are no TypeTree nodes for this object."),
        },
        ParseError::NoTypeTree { .. } => {
            new::<TypeTreeError>("There are no TypeTree nodes for this object.")
        }
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Limit(_) => new::<LimitError>(message(&error)),
            ReadDetail::RefTypeNoTree => new::<PyAttributeError>(message(&r.detail)),
            ReadDetail::NoRefTypes {
                reader: unityrs_core::value::ReadBy::Boost,
                ..
            } => new::<PyValueError>("Failed to get ref type node"),
            ReadDetail::NoRefTypes { absent: true, .. } => {
                new::<PyAttributeError>("'SerializedFile' object has no attribute 'ref_types'")
            }
            ReadDetail::NoRefTypes { .. } => new::<PyValueError>("SerializedFile has no ref_types"),
            ReadDetail::Stream(s) => exception(s.exception(), message(s)),
            ReadDetail::BoostEof {
                method,
                exception: kind,
                ..
            } => exception(kind, message(&format_args!("{method} out of bounds"))),
            ReadDetail::Eof { need, .. } => {
                crate::python::relay::attach(|py| match struct_error(py) {
                    Ok(cls) => typed(
                        &cls,
                        &message(&format_args!("unpack requires a buffer of {need} bytes")),
                    ),
                    Err(error) => error,
                })
            }
            ReadDetail::NegativeLength(_) if boost => {
                new::<PyValueError>("Negative length read from TypeTree")
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
            _ if r.kind == ReadErrorKind::OutOfBounds => new::<PyEOFError>(message(&r.detail)),
            _ => new::<PyValueError>(message(&r.detail)),
        },
        _ => new::<PyValueError>(message(&error)),
    }
}
fn exception(name: &str, message: String) -> PyErr {
    match name {
        "IndexError" => new::<PyIndexError>(message),
        "AttributeError" => new::<PyAttributeError>(message),
        "KeyError" => new::<PyKeyError>(message),
        "TypeError" => new::<PyTypeError>(message),
        "EOFError" => new::<PyEOFError>(message),
        "OverflowError" => new::<PyOverflowError>(message),
        "OSError" => new::<PyOSError>(message),
        _ => new::<PyValueError>(message),
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
