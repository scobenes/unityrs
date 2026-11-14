//! The scan against quick-xml 0.41's reader, which `object_store`'s parse
//! uses: on every body that reader accepts, both read the same root and the
//! same character data, and the scan finds nothing malformed.
use super::*;
use quick_xml::{Reader, events::Event};

/// What quick-xml's reader, configured as `object_store`'s parse configures
/// it (`Deserializer::from_reader` expands empty elements), makes of
/// `body`: none when it refuses it; else the root's name and which bytes
/// are character data.
fn quick_xml(body: &[u8]) -> Option<(Option<Vec<u8>>, Vec<bool>)> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().expand_empty_elements = true;
    let mut text = vec![false; body.len()];
    let mut root = None;
    let mut buf = Vec::new();
    loop {
        let from = usize::try_from(reader.buffer_position()).ok()?;
        let event = reader.read_event_into(&mut buf).ok()?;
        let to = usize::try_from(reader.buffer_position()).ok()?;
        match event {
            Event::Eof => break,
            Event::Start(start) => {
                if root.is_none() {
                    root = Some(start.name().as_ref().to_vec());
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => text.get_mut(from..to)?.fill(true),
            _ => {}
        }
        buf.clear();
    }
    Some((root, text))
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

/// Asserts the scan reads `body` as quick-xml does; whether quick-xml
/// accepted it.
fn agree(body: &[u8]) -> bool {
    let Some((root, text)) = quick_xml(body) else {
        return false;
    };
    for size in [1, 3, body.len().max(1)] {
        let (scan, ours) = scan(body, size);
        let shown = String::from_utf8_lossy(body);
        assert!(!scan.malformed(), "malformed: {shown}");
        let want = match root.as_deref() {
            None => Root::None,
            Some(b"Error") => Root::Error,
            Some(_) => Root::Other,
        };
        assert_eq!(scan.root(), want, "root: {shown}");
        assert_eq!(ours, text, "text: {shown}");
    }
    true
}

/// Pieces that cut markup every way the two readers could part on.
const PIECES: [&str; 64] = [
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
    fn soup(&mut self, out: &mut String, n: usize) {
        for _ in 0..n {
            out.push_str(PIECES[self.next() % PIECES.len()]);
        }
    }
}

#[test]
fn the_scan_reads_markup_as_quick_xml_does() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let prologs = [
        "",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE CompleteMultipartUploadResult [<!-- ]><Error> -->]>",
        "<!DOCTYPE r [<?pi ]> ?><!ENTITY e \"]>\"><!ATTLIST r a CDATA '>'>]>",
        "<!doctype r SYSTEM \"]>\">",
    ];
    let (mut accepted, mut with_doctype) = (0, 0);
    for i in 0..60_000 {
        let mut body = String::new();
        if i % 2 == 0 {
            let n = 1 + rng.next() % 8;
            rng.soup(&mut body, n);
        } else {
            body.push_str(prologs[rng.next() % prologs.len()]);
            let root = ["Error", "CompleteMultipartUploadResult", "a"][rng.next() % 3];
            body.push('<');
            body.push_str(root);
            body.push('>');
            let n = rng.next() % 6;
            rng.soup(&mut body, n);
            body.push_str("</");
            body.push_str(root);
            body.push('>');
        }
        if agree(body.as_bytes()) {
            accepted += 1;
            with_doctype += usize::from(body.to_ascii_lowercase().contains("<!doctype"));
        }
    }
    // Enough of each kind for the comparison to mean something.
    assert!(accepted > 5_000, "{accepted}");
    assert!(with_doctype > 500, "{with_doctype}");
}

#[test]
fn a_doctypes_subset_hides_its_markup() {
    // Review round 3: a comment, an instruction or a quoted literal in the
    // internal subset changes nothing outside it.
    for body in [
        &b"<!DOCTYPE CompleteMultipartUploadResult [<!-- ]><Error> -->]><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>"[..],
        b"<!DOCTYPE r [<?pi ]><Error> ?>]><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        b"<!DOCTYPE r [<!ENTITY q \"]><Error>\">]><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        b"<!DOCTYPE CompleteMultipartUploadResult><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
        b"<!DOCTYPE r SYSTEM \"]><Error>\"><CompleteMultipartUploadResult><ETag>e</ETag></CompleteMultipartUploadResult>",
    ] {
        assert!(agree(body), "{}", String::from_utf8_lossy(body));
        for size in [1, 2, 5, 7, body.len()] {
            let (scan, _) = scan(body, size);
            assert_eq!(scan.root(), Root::Other);
        }
    }
    // An error root after a subset is still one, with its code.
    let body = b"<!DOCTYPE Error [<!ENTITY x \"]>\"><!-- <CompleteMultipartUploadResult> -->]><Error><Code>InternalError</Code></Error>";
    assert!(agree(body));
    for size in [1, 2, 5, body.len()] {
        let (scan, _) = scan(body, size);
        assert_eq!(scan.root(), Root::Error);
        assert!(scan.code().is("InternalError"));
    }
    // Markup quick-xml refuses is malformed.
    for body in [
        &b"<!DOCTYPE><a/>"[..],
        b"<!x><a/>",
        b"<!-x--><a/>",
        b"<![x[]]><a/>",
        b"<?><a/>",
        b"<!DOCTYPX r><a/>",
    ] {
        assert!(
            quick_xml(body).is_none(),
            "{}",
            String::from_utf8_lossy(body)
        );
        assert!(
            scan(body, 1).0.malformed(),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
}
