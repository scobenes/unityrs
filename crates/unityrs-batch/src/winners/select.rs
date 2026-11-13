//! Constant-storage E31 accumulator shared by roots and plain keys. This is
//! independent of the in-memory specification model.
use super::{
    Context,
    codec::{Codec, Row, Rule, State, decode},
};
use crate::{phase::Error, sort::Frame};

pub(super) struct Choice {
    first: Frame,
    chosen: Frame,
    rule: Rule,
    different: bool,
    present: bool,
}
impl Choice {
    pub fn new(cx: &Context<'_>) -> Result<Self, Error> {
        Ok(Self {
            first: Frame::new(cx.account.meter())?,
            chosen: Frame::new(cx.account.meter())?,
            rule: Rule::FirstInput,
            different: false,
            present: false,
        })
    }
    pub fn clear(&mut self) {
        self.present = false;
        self.rule = Rule::FirstInput;
        self.different = false;
    }
    pub fn add(&mut self, bytes: &[u8], key: &str, cx: &Context<'_>) -> Result<(), Error> {
        let row = decode(bytes)?;
        let p = cx.prepared.preference(cx.spec, key);
        let rule = if p.and_then(|p| p.input) == Some(row.input) {
            Rule::PreferInput
        } else if p
            .and_then(|p| p.md5)
            .is_some_and(|md5| hex_equal(md5, &row.md5))
        {
            Rule::PreferMd5
        } else {
            Rule::FirstInput
        };
        if self.present {
            self.different |= decode(self.first.bytes())?.md5 != row.md5;
        } else {
            self.first.set(bytes)?;
        }
        if !self.present || (rule as u8) > (self.rule as u8) {
            self.chosen.set(bytes)?;
            self.rule = rule;
        }
        self.present = true;
        Ok(())
    }
    pub fn row(&self) -> Result<Option<Row<'_>>, Error> {
        if !self.present {
            return Ok(None);
        }
        let mut row = decode(self.chosen.bytes())?;
        row.rule = self.rule;
        row.flip = self.rule == Rule::FirstInput && self.different;
        Ok(Some(row))
    }
    pub fn decision(
        &self,
        key: &str,
        cx: &Context<'_>,
        codec: &mut Codec,
        out: &mut Frame,
    ) -> Result<(), Error> {
        let mut row = self.row()?.ok_or_else(Error::internal)?;
        if cx.prepared.withheld(cx, key)? {
            row.state = State::Withheld;
        }
        out.set(codec.encode(row)?)
    }
}
fn hex_equal(text: &str, bytes: &[u8; 16]) -> bool {
    text.len() == 32
        && text
            .bytes()
            .zip(bytes.iter().flat_map(|b| [b >> 4, b & 15]))
            .all(|(a, b)| a == if b < 10 { b + b'0' } else { b - 10 + b'a' })
}
