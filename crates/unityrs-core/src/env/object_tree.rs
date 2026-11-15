// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! [`Environment::object_tree`]: the tree an object reads with, and the
//! `MonoBehaviour` script hook.

use std::sync::Arc;

use super::resource_data::top_container;
use super::{DerefError, Environment};
use crate::container::View;
use crate::object::ObjectRef;
use crate::provider::{NoTree, ScriptIdent, TreeRef, TreeRequest, TreeSource};
use crate::typetree::Tree;
use crate::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};

/// Whether a read failure is a `ValueError` in UnityPy's object mode, the
/// one exception `generate_monobehaviour_node`'s caller swallows
/// (`files/ObjectReader.py:301-304`). Reader errors preserve their Python
/// categories: bounds can be `ValueError`, `EOFError` or `struct.error`, and
/// malformed children are `ValueError` in Boost but `IndexError` in Python.
/// Limits, refused charges and compilation failures are never swallowed.
fn is_value_error(e: &ParseError) -> bool {
    match e {
        ParseError::CheckRead { .. }
        | ParseError::Deref(DerefError::Null)
        | ParseError::NoTypeTree {
            why: Some(NoTree::NoEntry),
            ..
        } => true,
        ParseError::Provider(p) => {
            p.python_exception
                .as_ref()
                .is_some_and(crate::provider::PythonException::is_value_error)
                && p.limit.is_none()
                && p.memory.is_none()
                && p.allocator.is_none()
        }
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Eof { exception, .. } | ReadDetail::Python(exception) => {
                matches!(
                    *exception,
                    "ValueError"
                        | "UnicodeError"
                        | "UnicodeDecodeError"
                        | "UnicodeEncodeError"
                        | "UnicodeTranslateError"
                )
            }
            ReadDetail::NegativeLength(_)
            | ReadDetail::ArrayChildren
            | ReadDetail::PairChildren
            | ReadDetail::NoRefTypes
            | ReadDetail::RefTypeNotFound => true,
            ReadDetail::Stream(e) => e.exception() == "ValueError",
            ReadDetail::RefTypeNoTree
            | ReadDetail::Limit(_)
            | ReadDetail::Charge(_)
            | ReadDetail::Tree(_) => false,
        },
        _ => false,
    }
}

/// Failure of the explicit MonoBehaviour generator operation. Only `ValueError`
/// failures are swallowed by automatic fallback; other failures remain visible.
#[derive(Debug, thiserror::Error)]
pub enum MonoBehaviourError {
    /// No generator installed.
    #[error("MonoBehaviour detected, but no typetree_generator set to the environment!")]
    NoGenerator,
    /// The generator returned no node.
    #[error("Failed to generate MonoBehaviour node for {}{}{} of {}!", PythonStr(&.0.namespace),
        if .0.namespace.is_empty() { "" } else { "." }, PythonStr(&.0.class), PythonStr(&.0.assembly))]
    MissingNode(ScriptIdent),
    /// The head, script or provider failed.
    #[error(transparent)]
    Parse(#[from] ParseError),
}
const _: () = crate::send_sync::<MonoBehaviourError>();

/// Python str interpolation leaves valid Unicode, quotes and slashes literal.
/// Rust strings cannot hold lone surrogates; show those as Unicode escapes.
struct PythonStr<'a>(&'a crate::RawStr);
impl std::fmt::Display for PythonStr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for chunk in self.0.as_bytes().utf8_chunks() {
            f.write_str(chunk.valid())?;
            for &byte in chunk.invalid() {
                write!(f, "\\u{:04x}", 0xdc00 + u16::from(byte))?;
            }
        }
        Ok(())
    }
}

impl MonoBehaviourError {
    /// Whether UnityPy's automatic tree lookup swallows this failure.
    #[must_use]
    pub fn is_value_error(&self) -> bool {
        match self {
            Self::NoGenerator | Self::MissingNode(_) => true,
            Self::Parse(e) => is_value_error(e),
        }
    }
}

impl Environment {
    /// The tree an object reads with, as UnityPy resolves it
    /// (`files/ObjectReader.py:285-307`): [`ObjectRef::tree`], plus the
    /// `MonoBehaviour` script hook. When the tree came from the provider
    /// chain (not `nodes`, not the type record's own: UnityPy calls
    /// `generate_monobehaviour_node` only inside `if not node:`,
    /// `files/ObjectReader.py:296-304`), its root type is `MonoBehaviour`,
    /// and a provider accepts script requests
    /// ([`crate::provider::TypeTreeProvider::accepts_scripts`], UnityPy's
    /// `env.typetree_generator`), the head is read with that tree
    /// (`check_read` off), `m_Script` is dereferenced through this
    /// Environment and parsed, and the provider is asked with its
    /// [`crate::provider::ScriptIdent`]. As in UnityPy only a `ValueError`
    /// there falls back to the class tree (a null `m_Script`, a head read that
    /// fails with a value error, the provider's miss); a dereference failure
    /// (`FileNotFoundError`, `KeyError`, `IndexError`) is the object's error.
    /// With no such provider nothing is dereferenced (UnityPy raises its
    /// `ValueError` before any deref), so in P1 this equals [`ObjectRef::tree`].
    /// Each step that reads a dependency's object through its meter (the
    /// class tree's compile, for a type record that stores its own tree; the
    /// head; the script), when the pool refuses it, evicts the others and
    /// retries once, the object's top-level bundle held from the first step
    /// on, as the dereference does (plan section 3.13; review round 8, and
    /// round 9: the compile ran outside the retry, so a pool full of an
    /// evictable dependency failed every call on an object with an embedded
    /// tree). The provider chain's lookup charges no meter of the core's.
    pub fn object_tree(
        &self,
        obj: &ObjectRef,
        nodes: Option<&Arc<Tree>>,
    ) -> Result<Option<TreeRef>, ParseError> {
        let _top = top_container(obj.file());
        let found = self.room(|| obj.tree(nodes))?;
        let Some(class_tree) = found else {
            return Ok(None);
        };
        let chain = &obj.file().context().providers;
        if !matches!(class_tree.source, TreeSource::Provider(_))
            || class_tree.tree.root_type() != b"MonoBehaviour"
            || !chain.accepts_scripts()
        {
            return Ok(Some(class_tree));
        }
        match self.generate_monobehaviour_node(obj, Some(&class_tree.tree)) {
            Ok(tree) => Ok(Some(tree)),
            Err(e) if e.is_value_error() => Ok(Some(class_tree)),
            Err(MonoBehaviourError::Parse(e)) => Err(e),
            Err(_) => Ok(Some(class_tree)),
        }
    }

    /// Explicit `generate_monobehaviour_node` (`files/ObjectReader.py:332-350`).
    /// Unlike automatic fallback, a missing generator, null script or provider
    /// miss is an error. Embedded trees do not suppress this explicit operation.
    pub fn generate_monobehaviour_node(
        &self,
        obj: &ObjectRef,
        nodes: Option<&Arc<Tree>>,
    ) -> Result<TreeRef, MonoBehaviourError> {
        let _top = top_container(obj.file());
        let chain = &obj.file().context().providers;
        if !chain.accepts_scripts() {
            return Err(MonoBehaviourError::NoGenerator);
        }
        let head = self.room(|| obj.parse_monobehaviour_head(nodes))?;
        let ident = self.script_ident(obj, &head)?;
        // An explicit head and an embedded script need no file version in
        // UnityPy. Script lookups use the identity; zero is only the chain's
        // cache discriminator for files below format 7, never a TPK request.
        let unversioned = crate::UnityVersion::from_packed(0);
        let version = obj.file().version().map_or(&unversioned, |v| &v.version);
        let req = TreeRequest {
            class_id: obj.class_id(),
            version,
            script: Some(&ident),
        };
        chain
            .lookup(&req)
            .map_err(ParseError::from)?
            .ok_or(MonoBehaviourError::MissingNode(ident))
    }

    /// `parse_monobehaviour_head` and `m_Script.deref_parse_as_object()`:
    /// the script's identity, or `None` where UnityPy raises a `ValueError`
    /// (the caller falls back to the class tree).
    fn script_ident(
        &self,
        obj: &ObjectRef,
        head: &crate::Parsed,
    ) -> Result<ScriptIdent, ParseError> {
        let attr_error = || {
            ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Python("AttributeError"),
            })
        };
        let view = View::new(head).ok_or_else(attr_error)?;
        let script = view
            .attr(view.root(), b"m_Script")
            .map_err(|_| attr_error())?;
        let pptr = view.pptr(script).map_err(|_| attr_error())?;
        // `deref`: a null pointer is a `ValueError`.
        let target = match self.deref(obj.file(), pptr) {
            Ok(t) => t,
            Err(e) => return Err(ParseError::Deref(e)),
        };
        // `parse_as_object()`, with check_read enforced locally below.
        let script = self.room(|| target.read_typetree_object(None, false))?;
        // UnityPy helpers/TypeTreeHelper.py:155-156 raises ValueError on a
        // mismatch, swallowed by files/ObjectReader.py:301-304. Avoid
        // allocating an Arc<Parsed> diagnostic just to discard it here.
        if script.bytes_read != i64::from(target.info().byte_size) {
            return Err(ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Python("ValueError"),
            }));
        }
        let sview = View::new(&script).ok_or_else(attr_error)?;
        let field = |name: &[u8]| {
            sview
                .attr(sview.root(), name)
                .map(|n| n.value())
                .map_err(|_| attr_error())
        };
        // The f-string converts numeric identity fields with Python str().
        let text = |value: &crate::Value| {
            use crate::Value;
            let formatted = match value {
                Value::Str(s) => return Ok(s.clone()),
                Value::Int(i) => i.to_string(),
                Value::UInt(i) => i.to_string(),
                Value::Bool(b) => if *b { "True" } else { "False" }.to_owned(),
                Value::F32(b) => crate::pyfloat::py_float_repr(f64::from(f32::from_bits(*b))),
                Value::F64(b) => crate::pyfloat::py_float_repr(f64::from_bits(*b)),
                _ => return Err(attr_error()),
            };
            crate::RawStr::charged(formatted.as_bytes(), target.file().meter())
                .map_err(|e| ParseError::Read(crate::walk::charge_error(0, e)))
        };
        let namespace = text(field(b"m_Namespace")?)?;
        Ok(ScriptIdent {
            assembly: match field(b"m_AssemblyName")? {
                crate::Value::Str(s) => s.clone(),
                _ => return Err(attr_error()),
            },
            namespace,
            class: text(field(b"m_ClassName")?)?,
        })
    }
}
