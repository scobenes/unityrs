// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! [`Environment::object_tree`]: the tree an object reads with, and the
//! `MonoBehaviour` script hook.

use std::sync::Arc;

use super::resource_data::top_bundle;
use super::{DerefError, Environment};
use crate::container::View;
use crate::object::ObjectRef;
use crate::provider::{ScriptIdent, TreeRef, TreeRequest, TreeSource};
use crate::typetree::Tree;
use crate::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};

/// Whether a read failure is a `ValueError` in UnityPy's object mode, the
/// one exception `generate_monobehaviour_node`'s caller swallows
/// (`files/ObjectReader.py:301-304`): UnityPyBoost raises `EOFError` for a
/// read past the end, and a limit, a refused charge, a tree or a provider
/// failure is the core's own, never swallowed.
fn is_value_error(e: &ParseError) -> bool {
    match e {
        ParseError::CheckRead { .. } => true,
        ParseError::Read(r) => {
            r.kind != ReadErrorKind::OutOfBounds
                && !matches!(
                    r.detail,
                    ReadDetail::Limit(_) | ReadDetail::Charge(_) | ReadDetail::Tree(_)
                )
        }
        _ => false,
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
        let _top = top_bundle(obj.file());
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
        // UnityPy files/ObjectReader.py:332-350
        let Some(ident) = self.script_ident(obj, &class_tree.tree)? else {
            return Ok(Some(class_tree));
        };
        let Some(version) = obj.file().version() else {
            return Ok(Some(class_tree));
        };
        let req = TreeRequest {
            class_id: obj.class_id(),
            version: &version.version,
            script: Some(&ident),
        };
        // :346-350: the generator's miss is a `ValueError`: the class tree.
        Ok(Some(chain.lookup(&req)?.unwrap_or(class_tree)))
    }

    /// `parse_monobehaviour_head` and `m_Script.deref_parse_as_object()`:
    /// the script's identity, or `None` where UnityPy raises a `ValueError`
    /// (the caller falls back to the class tree).
    fn script_ident(
        &self,
        obj: &ObjectRef,
        class_tree: &Arc<Tree>,
    ) -> Result<Option<ScriptIdent>, ParseError> {
        // :325-330: the head, read with the class tree, `check_read` off.
        let head = match self.room(|| obj.read_typetree(Some(class_tree), false)) {
            Ok(p) => p,
            Err(e) if is_value_error(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(view) = View::new(&head) else {
            return Ok(None);
        };
        let attr_error = || {
            ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Python("AttributeError"),
            })
        };
        let script = view
            .attr(view.root(), b"m_Script")
            .map_err(|_| attr_error())?;
        let pptr = view.pptr(script).map_err(|_| attr_error())?;
        // `deref`: a null pointer is a `ValueError`.
        let target = match self.deref(obj.file(), pptr) {
            Ok(t) => t,
            Err(DerefError::Null) => return Ok(None),
            Err(e) => return Err(ParseError::Deref(Arc::new(e))),
        };
        // `parse_as_object()`, `check_read` on.
        let script = match self.room(|| target.read_typetree(None, true)) {
            Ok(p) => p,
            Err(e) if is_value_error(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(sview) = View::new(&script) else {
            return Ok(None);
        };
        let field = |name: &[u8]| {
            sview
                .attr(sview.root(), name)
                .ok()
                .and_then(|n| match n.value() {
                    crate::value::Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .ok_or_else(attr_error)
        };
        Ok(Some(ScriptIdent {
            assembly: field(b"m_AssemblyName")?,
            namespace: field(b"m_Namespace")?,
            class: field(b"m_ClassName")?,
        }))
    }
}
