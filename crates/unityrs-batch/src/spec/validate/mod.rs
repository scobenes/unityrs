//! Phase 1's per-value rules (`docs/BATCH_SPEC.md` section 10's order):
//! the checks of one integer and of one string, which the parse
//! (`super::parse`) applies to each value as it reads it, and the sink's.
//!
//! A sink's table is its first `kind` member's, wherever that member is,
//! so the parse gathers the sink's members first (scalars, each name of
//! either table at most once) and [`sink`] checks them at its end, in
//! document order: `duplicate_member`, `unknown_field`, then the value's
//! `type`, `range` and `format`, then `missing`. [`cross`] then checks the
//! cross-field rules on the typed spec.

pub mod cross;

use super::fields::{DIRECTORY, Field, MAX_INT, OBJECT_STORE, SINK_KINDS, Str, Ty};
use super::formats::{self, Verdict};
use super::node::{Member, Node};
use super::{BadSpec, Rule, push_token};

/// The rule an integer field's value breaks, if any. A number past 2^53 - 1
/// by magnitude is `range` whatever its token (a long integer reaches here
/// as a float); any other token but an integer one is `type`, `-0` and
/// `1.0` among them (`docs/BATCH_SPEC.md`'s conventions).
pub(crate) fn int_rule(node: &Node, min: u64, max: u64) -> Option<Rule> {
    match *node {
        Node::UInt(v) if (min..=max).contains(&v) => None,
        Node::UInt(_) | Node::NegInt(_) => Some(Rule::Range),
        #[allow(clippy::cast_precision_loss)]
        Node::Float(f) if f.is_finite() && f.fract() == 0.0 && f.abs() > MAX_INT as f64 => {
            Some(Rule::Range)
        }
        _ => Some(Rule::Type),
    }
}

/// The rule a string field's value breaks, if any: its length or its
/// closed values (`range`), then its form (`format`, or `range` for a URL
/// whose serialization passes its cap).
pub(crate) fn str_rule(v: &str, s: &Str) -> Option<Rule> {
    let len = v.len() as u64;
    if len < s.min || s.max.is_some_and(|m| len > m) {
        return Some(Rule::Range);
    }
    if !s.values.is_empty() && !s.values.contains(&v) {
        return Some(Rule::Range);
    }
    match s.format.map_or(Verdict::Ok, |f| formats::check(f, v)) {
        Verdict::Ok => None,
        Verdict::Range => Some(Rule::Range),
        Verdict::Format => Some(Rule::Format),
    }
}

/// The `kind` field the sink is checked by before its kind is known.
static SINK_KIND: Field = Field {
    name: "kind",
    ty: Ty::Str(Str {
        min: 0,
        max: None,
        values: SINK_KINDS,
        format: None,
    }),
    required: true,
    nullable: false,
    default: None,
};

/// Checks a sink's gathered `members`, at `ptr`, by the table its first
/// `kind` member names (C1: the first, whatever its value): when that is
/// no kind's name, each member against either kind's table (no name is in
/// both but `kind`) and `kind` against both kinds' names.
pub(crate) fn sink(ptr: &str, members: &[Member], kind: Option<&Node>) -> Result<(), BadSpec> {
    let mut w = Walker {
        ptr: ptr.to_owned(),
    };
    match kind {
        Some(Node::Str(k)) if k == "directory" => w.object(members, &DIRECTORY),
        Some(Node::Str(k)) if k == "object_store" => w.object(members, &OBJECT_STORE),
        _ => {
            w.members(members, |name| {
                if name == "kind" {
                    return Some(&SINK_KIND);
                }
                DIRECTORY
                    .iter()
                    .chain(&OBJECT_STORE)
                    .find(|f| f.name == name)
            })?;
            w.missing(members, std::slice::from_ref(&SINK_KIND))
        }
    }
}

struct Walker {
    /// The pointer of the value being checked.
    ptr: String,
}

impl Walker {
    fn err(&self, rule: Rule) -> BadSpec {
        BadSpec::new(rule, self.ptr.clone())
    }

    /// Runs `f` with `name` pushed on the pointer.
    fn under<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> T) -> T {
        let len = self.ptr.len();
        push_token(&mut self.ptr, name);
        let r = f(self);
        self.ptr.truncate(len);
        r
    }

    /// An object of fixed members.
    fn object(&mut self, members: &[Member], table: &'static [Field]) -> Result<(), BadSpec> {
        self.members(members, |name| table.iter().find(|f| f.name == name))?;
        self.missing(members, table)
    }

    /// Each member in order: `duplicate_member`, `unknown_field`, then its
    /// value. `lookup` finds a name's field.
    fn members(
        &mut self,
        members: &[Member],
        lookup: impl Fn(&str) -> Option<&'static Field>,
    ) -> Result<(), BadSpec> {
        let mut seen: Vec<&str> = Vec::new();
        for (name, value) in members {
            self.under(name, |w| {
                if seen.contains(&&**name) {
                    return Err(w.err(Rule::DuplicateMember));
                }
                let Some(field) = lookup(name) else {
                    return Err(w.err(Rule::UnknownField));
                };
                seen.push(name);
                w.value(value, field)
            })?;
        }
        Ok(())
    }

    /// The first required member of `table`, in the table's order, that
    /// `members` lacks.
    fn missing(&mut self, members: &[Member], table: &[Field]) -> Result<(), BadSpec> {
        for f in table.iter().filter(|f| f.required) {
            if !members.iter().any(|(k, _)| **k == *f.name) {
                return Err(self.under(f.name, |w| w.err(Rule::Missing)));
            }
        }
        Ok(())
    }

    /// A sink member's value: every sink field is a scalar.
    fn value(&self, node: &Node, field: &Field) -> Result<(), BadSpec> {
        let rule = match (node, &field.ty) {
            (Node::Null, _) if field.nullable => None,
            (Node::Bool(_), Ty::Bool) => None,
            (Node::UInt(_) | Node::NegInt(_) | Node::Float(_), Ty::Int { min, max }) => {
                int_rule(node, *min, *max)
            }
            (Node::Str(v), Ty::Str(s)) => str_rule(v, s),
            _ => Some(Rule::Type),
        };
        rule.map_or(Ok(()), |r| Err(self.err(r)))
    }
}
