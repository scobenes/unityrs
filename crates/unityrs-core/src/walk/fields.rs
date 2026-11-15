//! Ordered class fields for language bindings. Incomplete reads keep their
//! completed prefix so constructors can run before a later reader exception.
use super::{Out, ReadDetail, ReadError};

/// Progress of a binding's ordered object construction.
#[derive(Default)]
pub struct FieldProgress {
    /// Number of fully read class dictionaries, in postorder.
    pub completed: usize,
    /// First reader failure, deferred until the completed prefix is constructed.
    pub error: Option<ReadError>,
    /// String descriptor of an unresolved reference, for the binding's
    /// reader-specific diagnostic. These are shared slices of the input.
    pub reference_names: Option<[crate::RawStr; 3]>,
}

const _: () = crate::send_sync::<FieldProgress>();

impl FieldProgress {
    pub(crate) fn recover(&mut self, error: ReadError) -> Result<Out, ReadError> {
        if matches!(
            error.detail,
            ReadDetail::Limit(_) | ReadDetail::Charge(_) | ReadDetail::Tree(_)
        ) {
            return Err(error);
        }
        self.error.get_or_insert(error);
        // The binding stops before visiting the incomplete suffix. This scalar
        // carries no allocation and is never exposed as an object field.
        Ok(Out::Plain(crate::Value::Int(0)))
    }

    pub(crate) fn finish_class(&mut self) {
        if self.error.is_none() {
            self.completed += 1;
        }
    }
}
