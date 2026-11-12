//! Validation's checks of a template (`docs/BATCH_SPEC.md` 2.1): it
//! parses for its kind, and what its literals fix passes 2.3's rules in
//! every expansion. Each `/`-separated segment made only of literals passes
//! the segment rules (`empty_segment`, `dot_segment`, `segment_too_long`,
//! `reserved_name`); a template that starts with a literal passes
//! `leading_slash` and `reserved_prefix` on it, the key `_unityrs` itself
//! refused when that literal is the whole template; one that ends with a
//! literal passes `trailing_slash`. Everything else is each expansion's.
//!
//! A member path (4.7) takes every rule but `reserved_prefix` and
//! `reserved_name`, so its literals are not checked against them either.
//!
//! The check reads the text once through [`super::tokens`] and keeps a
//! few bytes of the segment it is in, so a template of any length that
//! parses costs no allocation. A refusal for an unknown or a disallowed
//! placeholder allocates its name once ([`TemplateError::Unknown`],
//! [`TemplateError::NotAllowed`], from [`super::tokens`]), on the error
//! path only.

use super::{
    KeyRule, RESERVED_NAME, RESERVED_PREFIX, Rules, SEGMENT_MAX, TemplateError, TemplateKind,
    Token, dot_segment, tokens,
};

/// Why validation refuses a template: rule `template` (section 10).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TemplateRefusal {
    /// It does not parse for its kind.
    #[error(transparent)]
    Parse(#[from] TemplateError),
    /// A literal part breaks this rule of 2.3 in every expansion: the
    /// first of those it breaks, in 2.3's order.
    #[error("a literal part breaks {}", .0.name())]
    Literal(KeyRule),
}

/// Checks a key template of `kind` (an output's key, 2.1): it parses, and
/// its literals pass 2.3's rules as 2.1 says.
pub fn check_template(text: &str, kind: TemplateKind) -> Result<(), TemplateRefusal> {
    literal(text, kind, Rules::Key)
}

/// Checks an archive member's path template of `kind` (1.7, 4.7):
/// [`check_template`] without `reserved_prefix` and `reserved_name`.
pub fn check_member_template(text: &str, kind: TemplateKind) -> Result<(), TemplateRefusal> {
    literal(text, kind, Rules::Member)
}

/// The most bytes of a segment's start the checks read: `.unityrs-tmp-`.
const HEAD: usize = RESERVED_NAME.len();

/// The segment being read.
struct Segment {
    /// Whether every piece of it so far is a literal.
    literal: bool,
    /// Its bytes so far, while it is literal.
    len: usize,
    /// Its first [`HEAD`] bytes.
    head: [u8; HEAD],
}

impl Segment {
    const fn new() -> Self {
        Self {
            literal: true,
            len: 0,
            head: [0; HEAD],
        }
    }

    fn push(&mut self, b: u8) {
        if let Some(slot) = self.head.get_mut(self.len) {
            *slot = b;
        }
        self.len += 1;
    }

    fn head(&self) -> &[u8] {
        self.head.get(..self.len.min(HEAD)).unwrap_or_default()
    }

    /// The rule a whole literal segment breaks, if any, `first` or `last`
    /// in the template: an empty one is a leading or trailing `/` there
    /// (the key's own rule), and `//` inside.
    fn rule(&self, first: bool, last: bool, which: Rules) -> Option<KeyRule> {
        if !self.literal {
            return None;
        }
        if self.len == 0 {
            return Some(if first {
                KeyRule::LeadingSlash
            } else if last {
                KeyRule::TrailingSlash
            } else {
                KeyRule::EmptySegment
            });
        }
        if std::str::from_utf8(self.head()).is_ok_and(|s| self.len <= 2 && dot_segment(s)) {
            return Some(KeyRule::DotSegment);
        }
        if self.len > SEGMENT_MAX {
            return Some(KeyRule::SegmentTooLong);
        }
        if which == Rules::Key && self.head() == RESERVED_NAME.as_bytes() {
            return Some(KeyRule::ReservedName);
        }
        None
    }
}

fn literal(text: &str, kind: TemplateKind, which: Rules) -> Result<(), TemplateRefusal> {
    if text.is_empty() {
        return Err(TemplateError::Empty.into());
    }
    let mut worst: Option<KeyRule> = None;
    let mut broke = |rule: Option<KeyRule>| {
        if let Some(r) = rule {
            worst = Some(worst.map_or(r, |w| w.min(r)));
        }
    };
    let mut segment = Segment::new();
    let mut first = true;
    // The literal the template starts with, while no variable has come.
    let mut lead = Segment::new();
    let mut leading = true;
    for token in tokens(text, kind) {
        match token? {
            Token::Var(_) => {
                segment.literal = false;
                leading = false;
            }
            Token::Lit(s) => {
                for &b in s.as_bytes() {
                    if leading {
                        lead.push(b);
                    }
                    if b == b'/' {
                        broke(segment.rule(first, false, which));
                        segment = Segment::new();
                        first = false;
                    } else if segment.literal {
                        segment.push(b);
                    }
                }
            }
        }
    }
    broke(segment.rule(first, true, which));
    let prefix = RESERVED_PREFIX.as_bytes();
    if which == Rules::Key && lead.head().starts_with(prefix) {
        // `_unityrs/...`, or `_unityrs` as the whole template.
        let whole = leading && lead.len == prefix.len();
        if whole || lead.head().get(prefix.len()) == Some(&b'/') {
            broke(Some(KeyRule::ReservedPrefix));
        }
    }
    match worst {
        Some(rule) => Err(TemplateRefusal::Literal(rule)),
        None => Ok(()),
    }
}
