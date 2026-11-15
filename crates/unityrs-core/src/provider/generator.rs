// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeGenerator.py. See THIRD_PARTY_LICENSES.
//! External script schemas, not an IL2CPP/Mono generator. Callers decode their
//! own `TypeTreeGeneratorAPI` or `Il2CppDumper` output, normalize its rows with
//! [`generator_node_list`], compile during registration, and supply the trees.
//! No dump, generated schema, or native generator is bundled with the library.

use super::{ProviderError, TreeRequest, TypeTreeProvider};
use crate::RawStr;
use crate::limits::{ChargeError, Meter, MeteredVec};
use crate::typetree::{NodeList, NodeRow, Tree, TreeError};
use std::fmt;
use std::sync::{Arc, Mutex};

/// An external generator or dump reader, with trees compiled during registration.
/// Failures retain their Python category through [`ProviderError::python_exception`].
/// The callback is always called without a cache lock held.
pub trait ScriptGenerator: Send + Sync + fmt::Debug {
    /// Assembly has the case-sensitive `.dll` suffix; fullname is namespace.class
    /// (or class alone). `None` models a generator miss.
    fn get_nodes(
        &self,
        assembly: &RawStr,
        fullname: &RawStr,
    ) -> Result<Option<Arc<Tree>>, ProviderError>;
}

/// The node conversion of `get_nodes_up` (`helpers/TypeTreeGenerator.py:50-75`):
/// size/version zero, meta flags retained, other optional fields absent.
pub fn generator_node_list(mut rows: MeteredVec<NodeRow>) -> Result<NodeList, TreeError> {
    let meter = rows.charge().meter().clone();
    for row in rows.as_mut_slice() {
        meter.count_decode(1)?;
        row.byte_size = 0;
        row.version = 0;
        row.type_flags = None;
        row.variable_count = None;
        row.index = None;
        row.ref_type_hash = None;
    }
    // Linear linking/table construction below (including parent-stack pops).
    meter.count_decode(
        u64::try_from(rows.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(8),
    )?;
    NodeList::from_rows(rows)
}

/// One externally compiled script schema. No input-sized copy is implicit.
#[derive(Debug)]
pub struct DumpEntry {
    assembly: RawStr,
    fullname: RawStr,
    tree: Arc<Tree>,
}

impl DumpEntry {
    /// API-style separate namespace and class fields.
    pub fn api(
        assembly: RawStr,
        namespace: &RawStr,
        class: RawStr,
        tree: Arc<Tree>,
        meter: &Arc<Meter>,
    ) -> Result<Self, ChargeError> {
        let fullname = if namespace.is_empty() {
            class
        } else {
            copy_concat(&[namespace.as_bytes(), b".", class.as_bytes()], meter)?
        };
        Ok(Self::il2cpp(assembly, fullname, tree))
    }

    /// Il2CppDumper-style fully qualified type name and assembly name. The
    /// caller supplies its decoded schema; native metadata extraction is external.
    #[must_use]
    pub fn il2cpp(assembly: RawStr, fullname: RawStr, tree: Arc<Tree>) -> Self {
        Self {
            assembly,
            fullname,
            tree,
        }
    }
}

/// A synthetic or caller-owned dump catalogue. First matching entry wins.
#[derive(Debug)]
pub struct DumpProvider {
    entries: MeteredVec<DumpEntry>,
}
impl DumpProvider {
    /// Take an already charged catalogue; no copy or allocation.
    #[must_use]
    pub fn new(entries: MeteredVec<DumpEntry>) -> Self {
        Self { entries }
    }
}
impl ScriptGenerator for DumpProvider {
    fn get_nodes(
        &self,
        assembly: &RawStr,
        fullname: &RawStr,
    ) -> Result<Option<Arc<Tree>>, ProviderError> {
        let meter = self.entries.charge().meter();
        for e in self.entries.as_slice() {
            if keys_match(meter, &e.assembly, &e.fullname, assembly, fullname)
                .map_err(|e| refusal(e, Arc::from("script work refused")))?
            {
                return Ok(Some(e.tree.clone()));
            }
        }
        Ok(None)
    }
}

#[derive(Debug)]
struct Cached {
    assembly: RawStr,
    fullname: RawStr,
    tree: Arc<Tree>,
}

/// UnityPy's `get_nodes_up` lookup/cache wrapper, usable in a provider chain.
/// Its entry bound only limits retention; it never changes an answer.
#[derive(Debug)]
pub struct GeneratorProvider<G> {
    generator: G,
    cache: Mutex<MeteredVec<Cached>>,
    meter: Arc<Meter>,
    max_entries: usize,
    refusal_detail: Arc<str>,
}
const _: () = {
    crate::send_sync::<dyn ScriptGenerator>();
    crate::send_sync::<DumpEntry>();
    crate::send_sync::<DumpProvider>();
    crate::send_sync::<GeneratorProvider<DumpProvider>>();
};
impl<G: ScriptGenerator> GeneratorProvider<G> {
    /// Create an empty, fallibly grown cache on `meter`.
    #[must_use]
    pub fn new(
        generator: G,
        meter: &Arc<Meter>,
        max_entries: usize,
        refusal_detail: Arc<str>,
    ) -> Self {
        Self {
            generator,
            cache: Mutex::new(MeteredVec::new(meter)),
            meter: meter.clone(),
            max_entries,
            refusal_detail,
        }
    }

    /// Answers retained under the original assembly/fullname pair.
    #[must_use]
    pub fn cached(&self) -> usize {
        self.cache.lock().map_or(0, |c| c.len())
    }

    /// Case-sensitive suffix normalization and cache keys as UnityPy supplies
    /// them (`helpers/TypeTreeGenerator.py:50-75`). The original assembly name
    /// is the key: `Assembly` and `Assembly.dll` remain distinct entries.
    pub fn get_nodes_up(
        &self,
        assembly: &RawStr,
        fullname: &RawStr,
    ) -> Result<Option<Arc<Tree>>, ProviderError> {
        if let Ok(cache) = self.cache.lock() {
            for c in cache.as_slice() {
                if keys_match(&self.meter, &c.assembly, &c.fullname, assembly, fullname)
                    .map_err(|e| self.memory(e))?
                {
                    return Ok(Some(c.tree.clone()));
                }
            }
        }
        let normalized;
        self.meter
            .count_decode(1 + u64::try_from(assembly.len().min(4)).unwrap_or(u64::MAX))
            .map_err(|e| self.memory(e))?;
        let lookup_assembly = if assembly.as_bytes().ends_with(b".dll") {
            assembly
        } else {
            normalized = copy_concat(&[assembly.as_bytes(), b".dll"], &self.meter)
                .map_err(|e| self.memory(e))?;
            &normalized
        };
        let answer = self.generator.get_nodes(lookup_assembly, fullname)?;
        if let Some(tree) = &answer
            && let Ok(mut cache) = self.cache.lock()
            && cache.len() < self.max_entries
        {
            for c in cache.as_slice() {
                if keys_match(&self.meter, &c.assembly, &c.fullname, assembly, fullname)
                    .map_err(|e| self.memory(e))?
                {
                    return Ok(answer);
                }
            }
            let assembly =
                copy_concat(&[assembly.as_bytes()], &self.meter).map_err(|e| self.memory(e))?;
            let fullname =
                copy_concat(&[fullname.as_bytes()], &self.meter).map_err(|e| self.memory(e))?;
            cache
                .push(Cached {
                    assembly,
                    fullname,
                    tree: tree.clone(),
                })
                .map_err(|e| self.memory(e))?;
        }
        Ok(answer)
    }

    fn memory(&self, error: ChargeError) -> ProviderError {
        refusal(error, self.refusal_detail.clone())
    }
}
impl<G: ScriptGenerator> TypeTreeProvider for GeneratorProvider<G> {
    fn name(&self) -> &'static str {
        "script"
    }
    fn accepts_scripts(&self) -> bool {
        true
    }
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let Some(script) = req.script else {
            return Ok(None);
        };
        let joined;
        let fullname = if script.namespace.is_empty() {
            &script.class
        } else {
            joined = copy_concat(
                &[script.namespace.as_bytes(), b".", script.class.as_bytes()],
                &self.meter,
            )
            .map_err(|e| self.memory(e))?;
            &joined
        };
        self.get_nodes_up(&script.assembly, fullname)
    }
}

fn refusal(error: ChargeError, detail: Arc<str>) -> ProviderError {
    let mut out = ProviderError::new("script", detail);
    match error {
        ChargeError::Limit(l) => out.limit = Some(l),
        ChargeError::Budget(b) => out.memory = Some(b),
        ChargeError::Allocator(a) => out.allocator = Some(a),
    }
    out
}

/// One step per comparison, plus an upper bound on the bytes it examines.
fn same(meter: &Meter, a: &RawStr, b: &RawStr) -> Result<bool, ChargeError> {
    let bytes = if a.len() == b.len() { a.len() } else { 0 };
    meter.count_decode(1_u64.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX)))?;
    Ok(a == b)
}

fn keys_match(
    meter: &Meter,
    a: &RawStr,
    f: &RawStr,
    assembly: &RawStr,
    fullname: &RawStr,
) -> Result<bool, ChargeError> {
    Ok(same(meter, a, assembly)? && same(meter, f, fullname)?)
}

fn copy_concat(parts: &[&[u8]], meter: &Arc<Meter>) -> Result<RawStr, ChargeError> {
    let bytes = parts.iter().fold(0_u64, |sum, p| {
        sum.saturating_add(u64::try_from(p.len()).unwrap_or(u64::MAX))
    });
    meter.count_decode(bytes)?;
    RawStr::charged_concat(parts, meter)
}
