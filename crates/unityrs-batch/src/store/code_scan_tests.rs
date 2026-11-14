//! The scan against quick-xml 0.41, the reader and the deserializer
//! `object_store`'s parse uses (`quick_xml::de::from_reader`, as
//! `src/aws/client.rs` calls it for a complete): on every prefix the reader
//! accepts, both read the same root and the same character data, and the
//! scan meets no refused markup; the scan accepts as an answer exactly the
//! bodies the deserializer takes, an `<Error>` root aside, whatever their
//! fields (into a struct of none) and, on bodies that can fail only by their
//! markup, into `object_store`'s own type; and a code it finds is one the
//! reader reads, and the deserializer too.
use super::*;
use quick_xml::{Reader, events::Event};
use serde::Deserialize;

/// `object_store`'s `CompleteMultipartUploadResult` (`src/client/s3.rs`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
struct Complete {
    #[serde(rename = "ETag")]
    e_tag: String,
}

/// A root with any content: what the deserializer takes, whatever the
/// fields.
#[derive(Debug, Deserialize)]
struct Any {}

/// An S3 error body, read by the same deserializer.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ErrorBody {
    code: Option<String>,
}

/// What quick-xml's reader, configured as the deserializer configures it
/// (empty elements expanded), makes of `body` up to where it stops.
struct Read {
    /// The first element's name.
    root: Option<Vec<u8>>,
    /// Which bytes are character data, up to `good`.
    text: Vec<bool>,
    /// Where the reader stopped: the end, or the start of what it refused.
    good: usize,
    /// Whether it read to the end.
    whole: bool,
}

fn reader(body: &[u8]) -> Read {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().expand_empty_elements = true;
    let mut read = Read {
        root: None,
        text: vec![false; body.len()],
        good: 0,
        whole: false,
    };
    let mut buf = Vec::new();
    loop {
        let from = usize::try_from(reader.buffer_position()).unwrap();
        let Ok(event) = reader.read_event_into(&mut buf) else {
            read.good = from;
            return read;
        };
        let to = usize::try_from(reader.buffer_position()).unwrap();
        match event {
            Event::Eof => break,
            Event::Start(start) => {
                if read.root.is_none() {
                    read.root = Some(start.name().as_ref().to_vec());
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => read.text[from..to].fill(true),
            _ => {}
        }
        buf.clear();
    }
    read.good = body.len();
    read.whole = true;
    read
}

/// The scan's reading of `body`, fed in chunks of `size`.
fn scan(body: &[u8], size: usize) -> (CodeScan, Vec<bool>) {
    let mut scan = CodeScan::default();
    let mut text = Vec::with_capacity(body.len());
    for chunk in body.chunks(size) {
        scan.feed_text(chunk, |_, is_text| text.push(is_text));
    }
    (scan, text)
}

/// What the scan accepts as a complete's answer (`super::super::complete`).
fn accepts(scan: &CodeScan) -> bool {
    scan.root() == Root::Other && scan.closed() && !scan.malformed()
}

/// The UTF-8 BOM, which quick-xml's reader drops before reading.
const BOM: &str = "\u{feff}";

#[derive(Default)]
struct Counts {
    compared: usize,
    unchecked: usize,
    /// Taken by the deserializer, whatever the fields.
    taken: usize,
    read_whole: usize,
    accepted: usize,
    rejected: usize,
    doctype: usize,
}

/// Compares the two readings of `body`; on `markup_only` bodies, which can
/// fail only by their markup, requires the scan to accept exactly what the
/// deserializer does.
fn compare(body: &str, markup_only: bool, counts: &mut Counts) {
    let bytes = body.as_bytes();
    let shown = body.escape_debug();
    // The reader drops a leading BOM and counts positions after it.
    let skip = if body.starts_with(BOM) { BOM.len() } else { 0 };
    let read = reader(bytes);
    let complete = quick_xml::de::from_reader::<_, Complete>(bytes).is_ok();
    let any = quick_xml::de::from_reader::<_, Any>(bytes).is_ok();
    let error = quick_xml::de::from_reader::<_, ErrorBody>(bytes);
    for size in [1, 3, bytes.len().max(1)] {
        let (scan, text) = scan(bytes, size);
        // Up to where the reader stops, the same markup and the same text.
        let good = read.good.min(bytes.len() - skip);
        assert_eq!(text[skip..skip + good], read.text[..good], "text: {shown}");
        let (prefix, _) = self::scan(&bytes[..skip + good], 1);
        assert!(!prefix.refused_any(), "refused: {shown}");
        let want = match read.root.as_deref() {
            None => Root::None,
            Some(b"Error") => Root::Error,
            Some(_) => Root::Other,
        };
        if read.whole {
            assert!(!scan.refused_any(), "refused: {shown}");
            assert_eq!(scan.root(), want, "root: {shown}");
        }
        // The scan takes what the deserializer takes, an `<Error>` root
        // aside, which the engine fails by design.
        // Past the bounds the scan keeps it may take what the parse
        // refuses, never the other way.
        if scan.unchecked() {
            assert!(
                !any || want != Root::Other || accepts(&scan),
                "accept: {shown}"
            );
        } else {
            assert_eq!(
                accepts(&scan),
                any && want == Root::Other,
                "accept: {shown}"
            );
        }
        if any {
            assert_eq!(scan.root(), want, "root: {shown}");
        }
        assert!(!complete || any, "{shown}");
        if markup_only && !scan.unchecked() {
            assert_eq!(
                accepts(&scan),
                complete && want == Root::Other,
                "markup: {shown}"
            );
        }
        // A code the scan finds lies in what the reader takes, and is the
        // one the deserializer reads.
        let code = scan.code();
        if !code.is_empty() {
            assert!(prefix.code() == code, "code: {shown}");
            if let Ok(error) = &error {
                assert!(
                    code.is(error.code.as_deref().unwrap_or("")),
                    "code: {shown}"
                );
            }
        }
    }
    counts.compared += 1;
    counts.unchecked += usize::from(self::scan(bytes, 1).0.unchecked());
    counts.read_whole += usize::from(read.whole);
    counts.accepted += usize::from(complete);
    counts.taken += usize::from(any);
    counts.rejected += usize::from(!complete);
    counts.doctype += usize::from(body.to_ascii_lowercase().contains("<!doctype"));
}

/// Pieces that cut markup every way the two readings could part on.
const PIECES: &[&str] = &[
    "<Error>",
    "</Error>",
    "<Code>",
    "</Code>",
    "NoSuchUpload",
    "SlowDown",
    "<CompleteMultipartUploadResult>",
    "</CompleteMultipartUploadResult>",
    "<ETag>",
    "</ETag>",
    "<!--",
    "-->",
    "--",
    "-",
    "<![CDATA[",
    "]]>",
    "]",
    "]]",
    "<?",
    "?>",
    "?",
    ">",
    "<",
    "/",
    "/>",
    "\"",
    "'",
    " ",
    "\n",
    "<!DOCTYPE r [",
    "]>",
    "<!DOCTYPE r>",
    "<!ENTITY a '",
    "<!ELEMENT e ",
    "<!ATTLIST e a CDATA \"",
    "<!NOTATION n ",
    "[",
    "&amp;",
    "&#83;",
    "x",
    "<a b='",
    "<a/>",
    "</a>",
    "<a>",
    "<!doctype x ",
    "<Error/>",
    "<!-",
    "<!D",
    "<?xml version=\"1.0\"?>",
    "<Er/ror>",
    "<Error \"",
    "<Error/ >",
    "< Error>",
    "</Code >",
    "</ Code>",
    "<Code x='>'>",
    "<!DOCTYPE Error [<!-- ]><Error> -->]>",
    "<?pi ]>?>",
    "<!ENTITY q \"]>\">",
    "<!DOCTYPE>",
    "<?>",
    "<!x>",
    "<a\"b\">",
    "\t",
    BOM,
    "<Ërror>",
    "</Ërror>",
    "é",
    "日本",
    "<名>",
    "</名>",
    "&#x53;",
    "&#x49;nternalError",
    "&#73;",
    "&lt;",
    "<!x",
    "<!-x-->",
    "<![x[]]>",
    "\u{a0}",
    "<Code>é</Code>",
    "<Error>\u{feff}",
    "&",
    "&bogus;",
    "&#0;",
    "&#xD800;",
    "&#32;",
    "&#X20;",
    "&a b;",
    "<b c='1' c='2'>",
    "</b>",
    "<a b='1'/>",
];

/// Prologs: what may come before the root.
const PROLOGS: &[&str] = &[
    "",
    BOM,
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
    "<!-- c -->",
    "<?pi x?>",
    " \n\t",
    "<!DOCTYPE CompleteMultipartUploadResult [<!-- ]><Error> -->]>",
    "<!DOCTYPE r [<?pi ]> ?><!ENTITY e \"]>\"><!ATTLIST r a CDATA '>'>]>",
    "<!doctype r SYSTEM \"]>\">",
    "<!DOCTYPE r>",
    "<!x>",
    "<?>",
    "<!-x-->",
    "<![x[]]>",
    "<!DOCTYPE>",
    "<!DOCTYPX r>",
    "<![CDATA[]]>",
    " <![CDATA[ ]]>",
    "<![CDATA[]]]>",
    "x",
    "&#32;",
    "&amp;",
    "\u{a0}",
    "\r\n",
    "<!-- -->\u{feff}",
];

/// The root's attributes, which the deserializer reads before its content;
/// the last two make the root empty.
const ATTRS: &[&str] = &[
    "",
    "",
    " ",
    " a='1'",
    " a = \"x\"",
    " a='1' a='2'",
    " a='1'b='2'",
    " a",
    " a=",
    " a=1",
    " a='>",
    " =''",
    " = ''",
    " xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"",
    " é='日本'",
    " a='1' A='1'",
    " abcdefghijklmnopq='1' abcdefghijklmnopq='2'",
    " a\t=\n'1'",
    " a='1'/b='2'",
    " a='1' b='2' c='3' d='4' e='5' a='6'",
    " a='&bogus;'",
    " a='/' /",
    " /",
];

/// Tails: what may follow the root, which the deserializer never reads.
const TAILS: &[&str] = &[
    "",
    "\n",
    "<!x>",
    "<!x",
    "<?>",
    "<a>",
    "</b>",
    "<!-- ",
    "garbage &bogus; <",
    "<Error/>",
];

/// A small deterministic generator (xorshift64).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % 1_000_003).unwrap()
    }
    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.next() % from.len()]
    }
    fn soup(&mut self, out: &mut String, n: usize) {
        for _ in 0..n {
            out.push_str(self.pick(PIECES));
        }
    }
}

#[test]
fn the_scan_reads_markup_as_quick_xml_does() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut counts = Counts::default();
    let mut markup = Counts::default();
    let mut errors = 0;
    for i in 0..60_000 {
        let mut body = String::new();
        match i % 4 {
            // Anything.
            0 => {
                let n = 1 + rng.next() % 8;
                rng.soup(&mut body, n);
            }
            // A root of any kind, anything inside.
            1 => {
                body.push_str(rng.pick(PROLOGS));
                let root = rng.pick(&["Error", "CompleteMultipartUploadResult", "a", "Ërror"]);
                body.push('<');
                body.push_str(root);
                body.push_str(rng.pick(ATTRS));
                if rng.next().is_multiple_of(8) {
                    body.push_str("/>");
                } else {
                    body.push('>');
                    let n = rng.next() % 6;
                    rng.soup(&mut body, n);
                    body.push_str("</");
                    body.push_str(root);
                    body.push('>');
                }
            }
            // An answer, or an error, that can fail only by its markup.
            2 => {
                for _ in 0..rng.next() % 3 {
                    body.push_str(rng.pick(&PROLOGS[1..]));
                }
                let root = rng.pick(&["CompleteMultipartUploadResult", "Error"]);
                let etag = rng.pick(&["\"e\"", "&quot;SlowDown&quot;", "é&#x53;", "日本"]);
                let attrs = rng.pick(&ATTRS[..ATTRS.len() - 2]);
                body.push_str(
                    &["<", root, attrs, "><ETag>", etag, "</ETag></", root, ">"].concat(),
                );
                for _ in 0..rng.next() % 3 {
                    body.push_str(rng.pick(TAILS));
                }
                compare(&body, true, &mut markup);
                continue;
            }
            // An error body with anything inside.
            _ => {
                body.push_str(rng.pick(PROLOGS));
                body.push_str("<Error>");
                let n = rng.next() % 5;
                rng.soup(&mut body, n);
                body.push_str("<Code>NoSuchUpload</Code>");
                let n = rng.next() % 3;
                rng.soup(&mut body, n);
                body.push_str("</Error>");
                errors += 1;
            }
        }
        compare(&body, false, &mut counts);
    }
    let all = [&counts, &markup];
    let sum = |f: fn(&Counts) -> usize| all.iter().map(|c| f(c)).sum::<usize>();
    let (compared, unchecked, taken, whole, accepted, rejected, doctype) = (
        sum(|c| c.compared),
        sum(|c| c.unchecked),
        sum(|c| c.taken),
        sum(|c| c.read_whole),
        sum(|c| c.accepted),
        sum(|c| c.rejected),
        sum(|c| c.doctype),
    );
    eprintln!(
        "compared {compared} ({unchecked} past the scan's bounds), read whole {whole}, taken {taken}, deserialized {accepted}, \
         refused {rejected}, \
         with a DOCTYPE {doctype}; markup-only {} ({} deserialized); error bodies {errors}",
        markup.compared, markup.accepted
    );
    // Enough of each kind for the comparison to mean something.
    assert_eq!(compared, 60_000);
    assert!(whole > 10_000, "{whole}");
    assert!(taken > 8_000, "{taken}");
    assert!(accepted > 4_000, "{accepted}");
    assert!(rejected > 40_000, "{rejected}");
    assert!(doctype > 5_000, "{doctype}");
    assert!(markup.accepted > 2_000 && markup.rejected > 2_000);
}

#[test]
fn a_doctypes_subset_hides_its_markup() {
    // Review round 3: a comment, an instruction or a quoted literal in the
    // internal subset changes nothing outside it.
    let mut counts = Counts::default();
    for body in [
        "<!DOCTYPE CompleteMultipartUploadResult [<!-- ]><Error> -->]><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        "<!DOCTYPE r [<?pi ]><Error> ?>]><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        "<!DOCTYPE r [<!ENTITY q \"]><Error>\">]><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        "<!DOCTYPE CompleteMultipartUploadResult><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        "<!DOCTYPE r SYSTEM \"]><Error>\"><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
    ] {
        compare(body, true, &mut counts);
        for size in [1, 2, 5, 7, body.len()] {
            assert!(accepts(&scan(body.as_bytes(), size).0), "{body}");
        }
    }
    assert_eq!(counts.accepted, 5);
    // An error root after a subset is still one, with its code.
    let body = "<!DOCTYPE Error [<!ENTITY x \"]>\"><!-- <CompleteMultipartUploadResult> -->]><Error><Code>InternalError</Code></Error>";
    compare(body, true, &mut counts);
    for size in [1, 2, 5, body.len()] {
        let (scan, _) = scan(body.as_bytes(), size);
        assert_eq!(scan.root(), Root::Error);
        assert!(scan.code().is("InternalError"));
    }
}

#[test]
fn refused_markup_latches_and_counts_only_before_the_root_closes() {
    // Review round 4: no code after refused markup, and nothing after the
    // root's end tag fails the answer.
    let mut counts = Counts::default();
    let latched = "<!x<Error><Code>NoSuchUpload</Code></Error>>";
    compare(latched, false, &mut counts);
    for size in [1, 4, latched.len()] {
        let (scan, _) = scan(latched.as_bytes(), size);
        assert!(scan.code().is_empty() && scan.malformed());
    }
    let inside = "<Error><!x><Code>NoSuchUpload</Code></Error>";
    assert!(scan(inside.as_bytes(), 1).0.code().is_empty());
    let trailing =
        "<CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult><!x>";
    compare(trailing, true, &mut counts);
    assert_eq!(counts.accepted, 1);
    for size in [1, 3, trailing.len()] {
        let (scan, _) = scan(trailing.as_bytes(), size);
        assert!(accepts(&scan) && scan.refused_any());
    }
    // Refused before the close, or no close at all: refused.
    for body in [
        "<CompleteMultipartUploadResult><!x><ETag>e</ETag></CompleteMultipartUploadResult>",
        "<!x><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        "<CompleteMultipartUploadResult><ETag>e</ETag>",
        "",
    ] {
        compare(body, true, &mut counts);
        assert!(!accepts(&scan(body.as_bytes(), 1).0), "{body}");
    }
}

#[test]
fn what_the_deserializer_checks_past_the_reader_the_scan_checks() {
    // References it resolves before the root and in the root's own text,
    // not deeper; the root's attributes; character data before the root;
    // and end tags that name another element.
    let mut counts = Counts::default();
    for body in [
        "<a>&bogus;</a>",
        "<a><b>&bogus;</b></a>",
        "<a><b><c>&bogus;</c></b></a>",
        "<a>&#0;</a>",
        "<a><b>&#0;</b></a>",
        "<a b='&bogus;'/>",
        "<a><b c='&bogus;'/></a>",
        "<a>& </a>",
        "<a><b>&</b></a>",
        "<a>&#x20;</a>",
        "&#32;<a/>",
        "&#x20;<a/>",
        "&#x0000020;<a/>",
        "&#X20;<a/>",
        "&#+32;<a/>",
        "&#9;\n<a/>",
        "&amp;<a/>",
        "<a>&#xD800;</a>",
        "<a>&#x110000;</a>",
        "<a>&#x10FFFF;</a>",
        "<a>&#99999999999;</a>",
        "<a>&a b;</a>",
        "<a><b>&#xD800;</b></a>",
        "<a><![CDATA[&bogus;]]></a>",
        "<a><b x='1' x='2'/></a>",
        "<a><b x/></a>",
        "<a>x<b/>&bogus;</a>",
        "<a>&quot;&apos;&lt;&gt;&amp;&#83;&#x53;&#x0053;</a>",
        "<a>&quote;</a>",
        "<a>&;</a>",
        " <![CDATA[ ]]><a/>",
        "<![CDATA[]]]><a/>",
        "<![CDATA[x]]><a/>",
        "\u{feff}<a/>",
        " \u{feff}<a/>",
        "x<a/>",
        "<a =''/>",
        "<a = ''/>",
        "<a b='1' b='2'/>",
        "<a b='1'b='2'/>",
        "<a b/>",
        "<a b=/>",
        "<a b='/>",
        "<a b='1' />",
        "<a b='1' / >",
        "<a/b='1'/>",
        "<a></b>",
        "<a></a ></a>",
        "<a></ a>",
        "<a><b></b ></a>",
        "</a><a/>",
        "<a><b></a>",
        "<Ërror></Ërror>",
        "<名></名>",
    ] {
        compare(body, false, &mut counts);
    }
    assert_eq!(counts.compared, 54);
}
