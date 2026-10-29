//! Canonical value encoding (docs/CANONICAL_DUMP.md section 3) from a [`Val`]
//! and the node that typed it.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;

use rabex::typetree::TypeTreeNode;

use crate::json::{write_raw, write_str};
use crate::value::{Val, element};

/// Encodes values for one SerializedFile; caches each node's dict plan.
/// `(name child, value child)` index pairs for a node whose child names
/// repeat; `None` when every name is unique (the common case).
type Plan = Option<Rc<[(usize, usize)]>>;

#[derive(Default)]
pub struct Encoder {
    /// Each node's plan, by node address.
    plans: HashMap<usize, Plan>,
}

impl Encoder {
    pub fn encode(&mut self, out: &mut String, v: &Val, node: &TypeTreeNode) -> Result<(), String> {
        match v {
            Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Val::I(i) => {
                let _ = write!(out, "{i}");
            }
            Val::U(u) => {
                let _ = write!(out, "{u}");
            }
            Val::F32(b) => {
                if node.m_Type != "float" {
                    return Err(format!("f32 under {} {}", node.m_Type, node.m_Name));
                }
                let _ = write!(out, "{{\"t\":\"f32\",\"v\":\"{b:08x}\"}}");
            }
            Val::F64(b) => {
                if node.m_Type != "double" {
                    return Err(format!("f64 under {} {}", node.m_Type, node.m_Name));
                }
                let _ = write!(out, "{{\"t\":\"f64\",\"v\":\"{b:016x}\"}}");
            }
            Val::Str(s) => write_str(out, s),
            Val::BadStr(b) => write_raw(out, "str", b),
            Val::Bytes(b) => write_raw(out, "bytes", b),
            Val::U8s(b) => write_raw(out, "u8s", b),
            Val::F32s(v) => {
                out.push_str("{\"t\":\"f32s\",\"v\":\"");
                for b in v {
                    let _ = write!(out, "{b:08x}");
                }
                out.push_str("\"}");
            }
            Val::F64s(v) => {
                out.push_str("{\"t\":\"f64s\",\"v\":\"");
                for b in v {
                    let _ = write!(out, "{b:016x}");
                }
                out.push_str("\"}");
            }
            Val::List(items) => {
                out.push('[');
                if !items.is_empty() {
                    let elem = element(node).ok_or("list under a non-vector node")?;
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        self.encode(out, item, elem)?;
                    }
                }
                out.push(']');
            }
            Val::Tuple(items) => {
                out.push_str("{\"t\":\"tuple\",\"v\":[");
                for (i, (item, child)) in items.iter().zip(&node.children).enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.encode(out, item, child)?;
                }
                out.push_str("]}");
            }
            Val::Dict(vals) => self.encode_dict(out, vals, node)?,
            // UnityPy reads a childless struct as an empty dict.
            Val::Unit => out.push_str("{\"t\":\"dict\",\"v\":[]}"),
        }
        Ok(())
    }

    /// A dict with `PyDict_SetItem` semantics: a repeated child name keeps its
    /// first position and takes the last child's value.
    fn encode_dict(
        &mut self,
        out: &mut String,
        vals: &[Val],
        node: &TypeTreeNode,
    ) -> Result<(), String> {
        let plan = self
            .plans
            .entry(std::ptr::from_ref(node) as usize)
            .or_insert_with(|| dict_plan(node))
            .clone();
        out.push_str("{\"t\":\"dict\",\"v\":[");
        let children = &node.children;
        let mut entry =
            |out: &mut String, i: usize, name: usize, last: usize| -> Result<(), String> {
                if i > 0 {
                    out.push(',');
                }
                out.push('[');
                write_str(out, &children[name].m_Name);
                out.push(',');
                let v = vals.get(last).ok_or("dict shorter than its node")?;
                self.encode(out, v, &children[last])?;
                out.push(']');
                Ok(())
            };
        match plan {
            None => {
                for i in 0..vals.len().min(children.len()) {
                    entry(out, i, i, i)?;
                }
            }
            Some(plan) => {
                for (i, &(name, last)) in plan.iter().enumerate() {
                    entry(out, i, name, last)?;
                }
            }
        }
        out.push_str("]}");
        Ok(())
    }
}

fn dict_plan(node: &TypeTreeNode) -> Plan {
    let children = &node.children;
    let dup =
        (1..children.len()).any(|i| children[..i].iter().any(|c| c.m_Name == children[i].m_Name));
    if !dup {
        return None;
    }
    let mut plan: Vec<(usize, usize)> = Vec::new();
    for (i, child) in children.iter().enumerate() {
        match plan
            .iter_mut()
            .find(|(n, _)| children[*n].m_Name == child.m_Name)
        {
            Some(slot) => slot.1 = i,
            None => plan.push((i, i)),
        }
    }
    Some(plan.into())
}
