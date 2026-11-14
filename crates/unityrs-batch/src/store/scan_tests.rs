use super::*;

fn scan(page: &[u8]) -> Result<(), Breach> {
    TagScan::new(1_000).feed(page)
}

/// A page of `n` entries, as S3 writes one.
fn page(n: usize, prefixes: usize) -> Vec<u8> {
    let mut out = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>b</Name><Prefix>p/</Prefix><KeyCount>1</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>true</IsTruncated>"#
        .to_vec();
    for i in 0..n {
        out.extend_from_slice(
            format!(
                "<Contents><Key>p/{i}&amp;x</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified>\
                 <ETag>&quot;d41d8cd98f00b204e9800998ecf8427e&quot;</ETag><ChecksumAlgorithm>CRC32</ChecksumAlgorithm>\
                 <Size>1</Size><Owner><ID>o</ID><DisplayName>d</DisplayName></Owner><StorageClass>STANDARD</StorageClass></Contents>"
            )
            .as_bytes(),
        );
    }
    for i in 0..prefixes {
        out.extend_from_slice(
            format!("<CommonPrefixes><Prefix>p/{i}/</Prefix></CommonPrefixes>").as_bytes(),
        );
    }
    out.extend_from_slice(b"<NextContinuationToken>t</NextContinuationToken></ListBucketResult>");
    out
}

#[test]
fn a_full_page_passes_in_any_chunking() {
    let full = page(1_000, 0);
    assert_eq!(scan(&full), Ok(()));
    assert_eq!(scan(&page(0, 1_000)), Ok(()));
    let mut bytewise = TagScan::new(1_000);
    for b in &full {
        assert_eq!(bytewise.feed(std::slice::from_ref(b)), Ok(()));
    }
    let mut odd = TagScan::new(1_000);
    for chunk in full.chunks(7) {
        assert_eq!(odd.feed(chunk), Ok(()));
    }
}

#[test]
fn entries_prefixes_and_start_tags_are_capped() {
    assert_eq!(scan(&page(1_001, 0)), Err(Breach));
    assert_eq!(scan(&page(0, 1_001)), Err(Breach));
    assert_eq!(TagScan::new(1).feed(&page(2, 0)), Err(Breach));
    // 16,064 start tags pass; one more breaks.
    let mut at = b"<r>".to_vec();
    at.extend(std::iter::repeat_n(&b"<e/>"[..], (STARTS_MAX - 1) as usize).flatten());
    assert_eq!(scan(&at), Ok(()));
    at.extend_from_slice(b"<e/>");
    assert_eq!(scan(&at), Err(Breach));
}

#[test]
fn start_tags_after_the_first_entry_outside_entries_are_capped() {
    let mut body = b"<r><Contents><Key>k</Key></Contents>".to_vec();
    body.extend(std::iter::repeat_n(&b"<x/>"[..], AFTER_MAX as usize).flatten());
    assert_eq!(scan(&body), Ok(()));
    body.extend_from_slice(b"<x></x>");
    assert_eq!(scan(&body), Err(Breach));
    // Before the first entry they are not counted, and inside one they are
    // the entry's.
    let mut before = b"<r>".to_vec();
    before.extend(std::iter::repeat_n(&b"<x/>"[..], 200).flatten());
    before.extend_from_slice(b"<Contents>");
    before.extend(std::iter::repeat_n(&b"<y/>"[..], 200).flatten());
    before.extend_from_slice(b"</Contents><CommonPrefixes><P/></CommonPrefixes>");
    assert_eq!(scan(&before), Ok(()));
    // A prefixed name is its local name.
    assert_eq!(
        scan(b"<r><s3:Contents></s3:Contents><s3:Contents/>"),
        Ok(())
    );
}

#[test]
fn nesting_is_capped_and_an_empty_element_adds_no_depth() {
    let open = "<a>".repeat(8);
    assert_eq!(scan(format!("{open}<b/><b/>").as_bytes()), Ok(()));
    assert_eq!(scan(format!("{open}<b>").as_bytes()), Err(Breach));
    assert_eq!(scan(format!("{open}</a><b x='1' />").as_bytes()), Ok(()));
    // An end tag with nothing open is malformed.
    assert_eq!(scan(b"<a></a></a>"), Err(Breach));
}

#[test]
fn markup_and_text_runs_are_capped_at_6_kib() {
    let max = MARKUP_MAX as usize;
    let tag = |len: usize| format!("<r a=\"{}\">", "v".repeat(len - 8));
    assert_eq!(scan(tag(max).as_bytes()), Ok(()));
    assert_eq!(scan(tag(max + 1).as_bytes()), Err(Breach));
    let comment = |len: usize| format!("<r><!--{}-->", "c".repeat(len - 7));
    assert_eq!(scan(comment(max).as_bytes()), Ok(()));
    assert_eq!(scan(comment(max + 1).as_bytes()), Err(Breach));
    let pi = |len: usize| format!("<?{}?>", "p".repeat(len - 4));
    assert_eq!(scan(pi(max).as_bytes()), Ok(()));
    assert_eq!(scan(pi(max + 1).as_bytes()), Err(Breach));
    let decl = |len: usize| format!("<!DOCTYPE r [{}]>", "d".repeat(len - 15));
    assert_eq!(scan(decl(max).as_bytes()), Ok(()));
    assert_eq!(scan(decl(max + 1).as_bytes()), Err(Breach));
    let text = |len: usize| format!("<r>{}</r>", "t".repeat(len));
    assert_eq!(scan(text(max).as_bytes()), Ok(()));
    assert_eq!(scan(text(max + 1).as_bytes()), Err(Breach));
    // A run goes on through comments and instructions, CDATA's text in it.
    let half = "t".repeat(max / 2);
    let merged = format!("<r>{half}<!-- c --><?p?>{half}<![CDATA[x]]></r>");
    assert_eq!(scan(merged.as_bytes()), Err(Breach));
    let ended = format!("<r>{half}<x/>{half}<![CDATA[x]]></r>");
    assert_eq!(scan(ended.as_bytes()), Ok(()));
    let cdata = |len: usize| format!("<r><![CDATA[{}]]></r>", "c".repeat(len - 12));
    assert_eq!(scan(cdata(max).as_bytes()), Ok(()));
    assert_eq!(scan(cdata(max + 1).as_bytes()), Err(Breach));
}

#[test]
fn quotes_comments_and_cdata_hide_markup() {
    // A quoted `>` or `/` does not end or empty a tag.
    assert_eq!(
        scan(br#"<r a=">/" b='>'><Contents x="/"></Contents></r>"#),
        Ok(())
    );
    let mut s = TagScan::new(1_000);
    assert_eq!(s.feed(br#"<r a="/>">"#), Ok(()));
    assert_eq!(s.depth, 1);
    // Tags inside comments and CDATA are text, not tags.
    let hidden = format!(
        "<r><!--{}--><![CDATA[{}]]></r>",
        "<a>".repeat(9),
        "<a>".repeat(9)
    );
    assert_eq!(scan(hidden.as_bytes()), Ok(()));
    // A breach is final.
    let mut s = TagScan::new(1_000);
    assert_eq!(s.feed(b"</x>"), Err(Breach));
    assert_eq!(s.feed(b"<r/>"), Err(Breach));
}

fn code(body: &[u8]) -> Code {
    let mut s = CodeScan::default();
    s.feed(body);
    s.code()
}

#[test]
fn the_code_scan_finds_the_error_code_across_chunks() {
    let body = b"<?xml version=\"1.0\"?><Error><Code>NoSuchUpload</Code><Code>Other</Code></Error>";
    for size in [1, 3, 7, body.len()] {
        let mut s = CodeScan::default();
        for chunk in body.chunks(size) {
            s.feed(chunk);
        }
        assert!(s.code().is("NoSuchUpload"), "{size}");
        assert!(!s.code().is("Other"));
    }
    let mut s = CodeScan::default();
    s.feed(b"<Error><Code>ExpiredToken");
    assert!(!s.code().is("ExpiredToken"), "unclosed");
    s.feed(b"</Code>");
    assert!(s.code().is("ExpiredToken"));
    let long = format!("<Error><Code>{}</Code></Error>", "A".repeat(CODE_MAX + 1));
    assert_eq!(code(long.as_bytes()), Code::default());
    let at = format!("<Error><Code>{}</Code></Error>", "A".repeat(CODE_MAX));
    assert!(code(at.as_bytes()).is(&"A".repeat(CODE_MAX)));
    assert!(code(b"<Error><Message>x</Message></Error>").is_empty());
    // Attributes, a namespace and a spaced end tag are still elements.
    let spaced = b"<Error xmlns=\"a>b\" x='<Code>'><Code >SlowDown</Code ></Error>";
    assert!(code(spaced).is("SlowDown"));
}

#[test]
fn the_code_scan_reads_elements_alone() {
    // Markup that is no element names no code (review round 1).
    for (body, want) in [
        (&b"<Error><!--<Code>NoSuchUpload</Code>--><Code>AccessDenied</Code></Error>"[..], "AccessDenied"),
        (b"<Error><![CDATA[<Code>NoSuchUpload</Code>]]><Code>AccessDenied</Code></Error>", "AccessDenied"),
        (b"<Error><?pi <Code>NoSuchUpload</Code>?><Code>AccessDenied</Code></Error>", "AccessDenied"),
        (b"<!DOCTYPE Error [<!ENTITY a '<Code>NoSuchUpload</Code>'>]><Error><Code>AccessDenied</Code></Error>", "AccessDenied"),
        (b"<Error><!-- - -> --><Code>AccessDenied</Code></Error>", "AccessDenied"),
    ] {
        for size in [1, 2, 5, body.len()] {
            let mut s = CodeScan::default();
            for chunk in body.chunks(size) {
                s.feed(chunk);
            }
            assert!(s.code().is(want), "{} {size}", String::from_utf8_lossy(body));
        }
    }
    for body in [
        // A comment, CDATA or element inside the code: no plain text.
        &b"<Error><Code>No<!---->SuchUpload</Code></Error>"[..],
        b"<Error><Code><![CDATA[NoSuchUpload]]></Code></Error>",
        b"<Error><Code><Code>NoSuchUpload</Code></Code></Error>",
        b"<Error><Code>NoSuch&amp;Upload</Code></Error>",
        // Nested below the root's child, or under another root.
        b"<Error><Detail><Code>NoSuchUpload</Code></Detail></Error>",
        b"<Result><Code>NoSuchUpload</Code></Result>",
        b"<Code>NoSuchUpload</Code>",
        // A close that is cut, or another element's.
        b"<Error><Code>NoSuchUpload</Cod",
        b"<Error><Code>NoSuchUpload</Codex>",
        b"<Error><Code>NoSuchUpload<",
        b"<Error><Code></Code></Error>",
        // An unclosed comment or CDATA swallows the rest.
        b"<Error><!--<Code>NoSuchUpload</Code>",
        b"<Error><![CDATA[<Code>NoSuchUpload</Code>",
    ] {
        assert!(code(body).is_empty(), "{}", String::from_utf8_lossy(body));
    }
}
