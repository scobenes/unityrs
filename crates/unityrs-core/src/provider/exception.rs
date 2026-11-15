//! External generator exceptions, including caller-defined Python subclasses.
use std::sync::Arc;

/// A generator exception preserved for a dynamic-language boundary.
/// Classification is supplied separately from its name: custom subclasses of
/// `ValueError` follow the same fallback as `ValueError` itself.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PythonException {
    // Share the payload so provider and reader errors stay small, and clones
    // preserve both the exception and its ancestry without allocating.
    data: Arc<ExceptionData>,
}

#[derive(Debug, PartialEq, Eq)]
struct ExceptionData {
    type_name: Arc<str>,
    message: Arc<str>,
    is_value_error: bool,
}

const _: () = crate::send_sync::<PythonException>();

impl PythonException {
    /// Preserve a Python failure and its independently determined ancestry.
    #[must_use]
    pub fn new(type_name: Arc<str>, message: Arc<str>, is_value_error: bool) -> Self {
        Self {
            data: Arc::new(ExceptionData {
                type_name,
                message,
                is_value_error,
            }),
        }
    }

    /// Python's exception type name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.data.type_name
    }

    /// Python's `str(exception)`, without the provider's diagnostic prefix.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.data.message
    }

    /// Whether `isinstance(exception, ValueError)` holds in Python.
    #[must_use]
    pub fn is_value_error(&self) -> bool {
        self.data.is_value_error
    }
}
