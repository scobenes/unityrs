//! New lists for the direct read ([`crate::direct`]), made at their final
//! length and filled before any other code can see them.
//!
//! `PyList_SetItem` is a call per item, with a type check, a bounds check
//! and the release of the slot's old item, none of which a list just made
//! by `PyList_New` needs: its slots are empty and it is held only here.
//! UnityPyBoost fills its lists with `PyList_SET_ITEM`, a store into the
//! list's item array, which the limited API this abi3 module is built
//! against does not offer. So where the running interpreter is checked to
//! lay a list out as every GIL build of `CPython` 3.10 to 3.14 does
//! ([`slots`]), a new list's items are stored into that array directly, as
//! the macro does; anywhere else (a free-threaded build, whose object header
//! differs, or a `CPython` from 3.15 on, whose layout this module has not
//! been checked against), each goes through `PyList_SetItem`.
use crate::direct::own;
use pyo3::{ffi, prelude::*};
use std::sync::OnceLock;

/// `PyListObject`'s `ob_item` (`PyObject **`), in pointer-sized words from
/// the object's start: after `PyObject_VAR_HEAD` (`ob_refcnt`, `ob_type`,
/// `ob_size`) on a GIL build of `CPython` 3.10 to 3.14
/// (Include/cpython/listobject.h).
const OB_ITEM: usize = 3;
/// `ob_size`, the word before it.
const OB_SIZE: usize = 2;
/// `ob_type`, the word before that. A free-threaded build's header puts
/// `ob_tid`, its flags and its local and shared counts first, so the type
/// is not here, nor the size and items in the two words after it.
const OB_TYPE: usize = 1;

/// Proof that this interpreter's lists are laid out as [`OB_ITEM`] says:
/// only [`slots`] makes one, and only where every check holds. Its field is
/// private, so no other code can make one.
#[derive(Clone, Copy)]
pub(crate) struct Slots(());

/// [`Slots`] where this interpreter is checked, once (at import,
/// `crate::native`), to be all of: `CPython` (`sys.implementation.name`);
/// a version from 3.10 to 3.14 (a later one is not taken on
/// trust); a GIL build with the GIL on (`Py_GIL_DISABLED` unset or false,
/// and from 3.13 `sys._is_gil_enabled()` true); and one where a probe
/// list's type, size and item array are found where a GIL build has them.
/// A free-threaded build (this abi3 module declares `gil_used`, but such a
/// build keeps its own object header even with the GIL on) fails the build
/// check, and would fail the probe too: its type is not at [`OB_TYPE`].
/// Any doubt, or a failure, answers `None`.
pub(crate) fn slots(py: Python<'_>) -> Option<Slots> {
    static OK: OnceLock<bool> = OnceLock::new();
    OK.get_or_init(|| interpreter(py).unwrap_or(false) && probe(py).unwrap_or(false))
        .then_some(Slots(()))
}

/// The interpreter checks of [`slots`], before the probe.
fn interpreter(py: Python<'_>) -> Option<bool> {
    // Every name interned (`name!`), and every call fallible, so a failed
    // allocation answers `None`, never a panic.
    use crate::direct_keys::{import_attr, name};
    let sys = name!(py, c"sys").ok()?;
    let name: String = import_attr(py, sys, name!(py, c"implementation").ok()?)
        .ok()?
        .getattr(name!(py, c"name").ok()?)
        .ok()?
        .extract()
        .ok()?;
    let version = py.version_info();
    let listed = matches!((version.major, version.minor), (3, 10..=14));
    if name != "cpython" || !listed {
        return Some(false);
    }
    let get = import_attr(
        py,
        name!(py, c"sysconfig").ok()?,
        name!(py, c"get_config_var").ok()?,
    )
    .ok()?;
    let disabled = crate::direct::call1(py, &get, name!(py, c"Py_GIL_DISABLED").ok()?)?
        .is_truthy()
        .ok()?;
    if disabled {
        return Some(false);
    }
    // 3.13 added `sys._is_gil_enabled()`: from there it must answer
    // true, and a failed lookup or call fails the check.
    if version.minor < 13 {
        return Some(true);
    }
    import_attr(py, sys, name!(py, c"_is_gil_enabled").ok()?)
        .ok()?
        .call0()
        .ok()?
        .is_truthy()
        .ok()
}

/// Whether a probe list's type, size and item array are where a GIL
/// build of `CPython` 3.10 to 3.14 has them.
fn probe(py: Python<'_>) -> Option<bool> {
    let (a, b) = (
        crate::direct_keys::name!(py, c"unityrs.direct_list.a").ok()?,
        crate::direct_keys::name!(py, c"unityrs.direct_list.b").ok()?,
    );
    // SAFETY: a new list of two empty slots, or null.
    let list = own(py, unsafe { ffi::PyList_New(2) })?;
    for (at, item) in [a, b].into_iter().enumerate() {
        // SAFETY: `at` is in range; the call steals the new reference.
        unsafe { ffi::PyList_SetItem(list.as_ptr(), at.cast_signed(), item.clone().into_ptr()) };
    }
    let base = list.as_ptr().cast::<usize>();
    // SAFETY: every list object is at least five words long (its header,
    // `ob_size`, `ob_item` and `allocated` on a GIL build; a longer header
    // on a free-threaded one), so the words read are within the object,
    // which `list` keeps alive. Only where they hold the list's type and
    // the size it was made with is the next taken for `ob_item`, and only
    // then is it read through, for what `PyList_SetItem` stored.
    let found = unsafe {
        *base.add(OB_TYPE) == list.get_type().as_ptr() as usize && *base.add(OB_SIZE) == 2 && {
            let items = *base.add(OB_ITEM) as *const *mut ffi::PyObject;
            !items.is_null() && *items == a.as_ptr() && *items.add(1) == b.as_ptr()
        }
    };
    Some(found)
}

/// A list of `len` empty slots, held only by the caller, and where
/// [`slots`] holds its item array.
pub(crate) struct NewList<'py> {
    list: Bound<'py, PyAny>,
    items: *mut *mut ffi::PyObject,
    len: isize,
}

impl<'py> NewList<'py> {
    /// `PyList_New(len)`, its slots stored directly where `slots` is
    /// [`slots`]'s proof.
    #[inline]
    pub(crate) fn new(py: Python<'py>, len: usize, slots: Option<Slots>) -> Option<Self> {
        let len = isize::try_from(len).ok()?;
        // SAFETY: a new list of `len` empty slots or null.
        let list = own(py, unsafe { ffi::PyList_New(len) })?;
        let items = if slots.is_some() && len > 0 {
            // SAFETY: a `Slots` exists only where [`slots`] found this
            // interpreter's lists laid out so: the list's item array, of
            // `len` slots, is this word.
            unsafe { *list.as_ptr().cast::<*mut *mut ffi::PyObject>().add(OB_ITEM) }
        } else {
            std::ptr::null_mut()
        };
        Some(Self { list, items, len })
    }

    /// Stores `item` (a new reference, which the list takes) in slot `at`,
    /// empty and below `len`.
    ///
    /// # Safety
    /// `at` is below the length the list was made with, and its slot is
    /// still empty.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "once per list item, in the run's element loop"
    )]
    pub(crate) unsafe fn put(&mut self, at: isize, item: *mut ffi::PyObject) {
        debug_assert!((0..self.len).contains(&at));
        if self.items.is_null() {
            // SAFETY: the caller's; the call steals the item.
            unsafe { ffi::PyList_SetItem(self.list.as_ptr(), at, item) };
        } else {
            // SAFETY: the caller's: an empty slot of this list's item array,
            // which only this function fills (`PyList_SET_ITEM`).
            unsafe { *self.items.offset(at) = item };
        }
    }

    pub(crate) fn len(&self) -> isize {
        self.len
    }

    pub(crate) fn as_ptr(&self) -> *mut ffi::PyObject {
        self.list.as_ptr()
    }

    pub(crate) fn into_list(self) -> Bound<'py, PyAny> {
        self.list
    }
}

/// A list of `items`, each moved in. The items are all made before the
/// list is, and nothing allocates while its slots are filled, so no list
/// with an empty slot is ever reachable.
pub(crate) fn list_of<'py>(
    py: Python<'py>,
    items: impl ExactSizeIterator<Item = Bound<'py, PyAny>>,
) -> Option<Bound<'py, PyAny>> {
    let mut list = NewList::new(py, items.len(), slots(py))?;
    for (at, item) in (0..list.len()).zip(items) {
        // SAFETY: `at` runs over the list's slots once each.
        unsafe { list.put(at, item.into_ptr()) };
    }
    Some(list.into_list())
}
