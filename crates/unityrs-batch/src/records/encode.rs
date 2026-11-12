use super::{
    Candidate, Diagnostic, Error, LINE, Member, Reason, Record, Skip, Summary, Text, text::escaped,
};
use crate::digest::Hex;
use crate::reason::{KeyRule, LeftOut};
use std::fmt::{self, Write};

pub(super) trait Sink {
    fn put(&mut self, bytes: &[u8]) -> Result<(), Error>;
}
pub(super) struct Count(pub usize);
impl Sink for Count {
    fn put(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|&n| n <= LINE)
            .ok_or(Error::Internal)?;
        Ok(())
    }
}
struct Encoder<'a> {
    sink: &'a mut dyn Sink,
    cut: bool,
}
// fmt's arguments are numbers and small static codes only. A formatter's
// error is converted back to the original sink failure by DisplaySink.
struct DisplaySink<'a> {
    sink: &'a mut dyn Sink,
    error: Option<Error>,
}
impl fmt::Write for DisplaySink<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.sink.put(s.as_bytes()).map_err(|e| {
            self.error = Some(e);
            fmt::Error
        })
    }
}
impl Encoder<'_> {
    fn raw(&mut self, b: &[u8]) -> Result<(), Error> {
        self.sink.put(b)
    }
    fn display(&mut self, d: impl fmt::Display) -> Result<(), Error> {
        let mut sink = DisplaySink {
            sink: self.sink,
            error: None,
        };
        write!(sink, "{d}").map_err(|_| sink.error.unwrap_or(Error::Internal))
    }
    fn field(&mut self, name: &'static str) -> Result<(), Error> {
        self.raw(b",\"")?;
        self.raw(name.as_bytes())?;
        self.raw(b"\":")
    }
    fn content(&mut self, t: Text<'_>) -> Result<(), Error> {
        self.cut |= escaped(t, |bytes| self.sink.put(bytes))?;
        Ok(())
    }
    fn text(&mut self, t: Text<'_>) -> Result<(), Error> {
        self.raw(b"\"")?;
        self.content(t)?;
        self.raw(b"\"")
    }
    fn str_field(&mut self, name: &'static str, value: &str) -> Result<(), Error> {
        self.field(name)?;
        self.text(value.into())
    }
    fn text_field(&mut self, name: &'static str, value: Text<'_>) -> Result<(), Error> {
        self.field(name)?;
        self.text(value)
    }
    fn optional_text(&mut self, name: &'static str, value: Option<Text<'_>>) -> Result<(), Error> {
        self.field(name)?;
        match value {
            Some(t) => self.text(t),
            None => self.raw(b"null"),
        }
    }
    fn num(&mut self, name: &'static str, value: impl fmt::Display) -> Result<(), Error> {
        self.field(name)?;
        self.display(value)
    }
    fn hex(&mut self, name: &'static str, value: &[u8]) -> Result<(), Error> {
        self.field(name)?;
        self.raw(b"\"")?;
        self.display(Hex(value))?;
        self.raw(b"\"")
    }
    fn reason(&mut self, value: Reason<'_>) -> Result<(), Error> {
        self.raw(b"\"")?;
        match value {
            Reason::Text(t) => self.content(t)?,
            Reason::Argument { code, value } => {
                self.content(code.into())?;
                self.raw(b"(")?;
                self.content(value)?;
                self.raw(b")")?;
            }
            Reason::Dependency { id, reason } => {
                self.raw(b"dependency_failed(")?;
                self.content(id)?;
                self.raw(b", ")?;
                self.content(reason)?;
                self.raw(b")")?;
            }
        }
        self.raw(b"\"")
    }
    fn optional_reason(&mut self, reason: Option<Reason<'_>>) -> Result<(), Error> {
        self.field("reason")?;
        match reason {
            Some(r) => self.reason(r),
            None => self.raw(b"null"),
        }
    }
    fn start(&mut self, kind: &str) -> Result<(), Error> {
        self.raw(b"{\"type\":")?;
        self.text(kind.into())
    }
    fn end(&mut self, inherited: bool) -> Result<(), Error> {
        self.field("cut")?;
        self.display(self.cut || inherited)?;
        self.raw(b"}\n")
    }
    fn candidate(&mut self, c: Candidate<'_>) -> Result<(), Error> {
        self.start("candidate")?;
        self.str_field("key", c.key)?;
        self.optional_text("unit", c.unit.map(Text::from))?;
        self.str_field("input", c.input)?;
        self.text_field("serialized_file", c.serialized_file)?;
        self.str_field("kind", c.kind.name())?;
        self.num("path_id", c.path_id)?;
        self.hex("md5", &c.hashes.md5)?;
        self.num("crc32", c.hashes.crc32)?;
        if let Some(picture) = c.picture {
            self.hex("visible_hash", &picture.visible_hash)?;
            self.str_field("mode", picture.mode)?;
        } else {
            self.optional_text("visible_hash", None)?;
            self.optional_text("mode", None)?;
        }
        self.num("size", c.size)?;
        self.field("etag")?;
        if c.hashes.etag.is_multipart() {
            self.raw(b"\"")?;
            self.display(c.hashes.etag)?;
            self.raw(b"\"")?;
        } else {
            self.raw(b"null")?;
        }
        self.field("file")?;
        self.raw(b"\"o")?;
        self.display(c.plan_index)?;
        self.raw(b"\"")?;
        self.end(c.cut)
    }
    fn member(&mut self, m: Member<'_>) -> Result<(), Error> {
        self.start("member")?;
        self.num("archive", m.archive)?;
        self.member_fields(m)?;
        self.end(m.cut)
    }
    fn member_fields(&mut self, m: Member<'_>) -> Result<(), Error> {
        self.text_field("path", m.path)?;
        self.str_field("key", m.key)?;
        self.str_field("input", m.input)?;
        self.hex("md5", m.md5)?;
        self.field("reason")?;
        if let Some(r) = m.reason {
            self.raw(b"\"")?;
            self.display(r)?;
            self.raw(b"\"")
        } else {
            self.raw(b"null")
        }
    }
    fn skip(&mut self, s: Skip<'_>) -> Result<(), Error> {
        if s.class.len() > 64 || !s.class.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::Internal);
        }
        self.start("skip")?;
        self.str_field("input", s.input)?;
        self.text_field("serialized_file", s.serialized_file)?;
        self.num("path_id", s.path_id)?;
        self.str_field("class", s.class)?;
        self.str_field("output", s.output.name())?;
        self.field("sprite")?;
        if let Some(n) = s.sprite {
            self.display(n)?;
        } else {
            self.raw(b"null")?;
        }
        self.optional_reason(Some(s.reason))?;
        self.optional_text("detail", s.detail)?;
        self.end(s.cut)
    }
    fn diagnostic(&mut self, d: Diagnostic<'_>) -> Result<(), Error> {
        self.start("diagnostic")?;
        self.str_field("kind", d.kind.name())?;
        self.str_field("input", d.input)?;
        self.optional_text("bundle", d.bundle.map(Text::from))?;
        self.text_field("name", d.name)?;
        self.end(d.cut)
    }
    fn summary(&mut self, s: Summary<'_>) -> Result<(), Error> {
        self.start("input_summary")?;
        self.str_field("id", s.input)?;
        self.str_field("status", s.status.name())?;
        self.optional_reason(s.reason)?;
        self.optional_text("detail", s.detail)?;
        let figures = s.figures.unwrap_or_default();
        for (name, value) in [
            ("input_peak", figures.input_peak),
            ("dependency_peak", figures.dependency_peak),
            ("evictions", figures.evictions),
            ("export_work", figures.export_work),
            ("decode_work_peak", figures.decode_work_peak),
            ("ms", figures.ms),
        ] {
            self.field(name)?;
            if s.figures.is_some() {
                self.display(value)?;
            } else {
                self.raw(b"null")?;
            }
        }
        self.num("skips", figures.skips)?;
        self.num("unresolved_scripts", figures.unresolved_scripts)?;
        self.num("spooled_part", s.spooled_part)?;
        self.num("claimed_bytes", s.claimed_bytes)?;
        self.end(s.cut)
    }
}
pub(super) fn record(sink: &mut dyn Sink, r: Record<'_>) -> Result<(), Error> {
    let mut e = Encoder { sink, cut: false };
    match r {
        Record::Candidate(c) => e.candidate(c),
        Record::Member(m) => e.member(m),
        Record::Skip(s) => e.skip(s),
        Record::Diagnostic(d) => e.diagnostic(d),
    }
}
pub(super) fn summary(sink: &mut dyn Sink, s: Summary<'_>) -> Result<bool, Error> {
    let mut e = Encoder { sink, cut: false };
    e.summary(s)?;
    Ok(e.cut || s.cut)
}
/// Measure the largest final manifest member row, not its compact artifact
/// row. Include the archive key, all left-out reasons and the newline.
pub(super) fn member_work(archive: &str, m: Member<'_>) -> Result<u64, Error> {
    let mut largest = 0;
    let reasons = [
        None,
        Some(LeftOut::DuplicateMember),
        Some(LeftOut::PrefixConflict),
    ]
    .into_iter()
    .chain(
        KeyRule::ALL
            .into_iter()
            .map(|r| Some(LeftOut::UnwritablePath(r))),
    );
    for reason in reasons {
        let mut count = Count(0);
        let mut e = Encoder {
            sink: &mut count,
            cut: false,
        };
        e.raw(b"{\"archive\":")?;
        e.text(archive.into())?;
        e.member_fields(Member { reason, ..m })?;
        e.num("included", reason.is_none())?;
        e.end(m.cut)?;
        largest = largest.max(count.0);
    }
    Ok(largest as u64)
}
