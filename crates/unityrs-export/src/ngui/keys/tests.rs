//! The key rules' and the templates' unit tests (module docs): expansion
//! to the stop and the code point it cuts, `safe`, and each rule in its
//! order.

use super::{
    KeyRule, STOP, Template, TemplateError, TemplateKind, Var, check, check_member, check_prefixed,
    cut_tail, safe, safe_into,
};

fn expanded<'v>(t: &Template, value: impl Fn(Var) -> &'v [u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(STOP);
    let at = out.as_ptr();
    let _ = t.expand(value, &mut out);
    assert_eq!(out.as_ptr(), at, "the expansion grew its buffer");
    out
}

#[test]
fn the_stop_tells_a_cut_code_point_from_an_invalid_byte() {
    // Review round 3's P3: 1,023 bytes, then a four-byte character the
    // stop cuts after two of its bytes: two bytes of a cut code point.
    let t = Template::parse("{atlas}", TemplateKind::AtlasJson).unwrap();
    let mut v = vec![b'a'; 1023];
    v.extend("\u{1f600}".as_bytes());
    let mut out = Vec::with_capacity(STOP);
    assert_eq!(t.expand(|_| &v, &mut out), 2);
    // The same two bytes invalid in the value: one code point each.
    v.truncate(1025);
    v.push(b'z');
    assert_eq!(t.expand(|_| &v, &mut out), 0);
    // A literal cut the same way, and a stop on a boundary.
    let lit = format!("{}\u{e9}x", "b".repeat(1024));
    let t = Template::parse(&lit, TemplateKind::AtlasJson).unwrap();
    assert_eq!(t.expand(|_| b"", &mut out), 1);
    assert_eq!(cut_tail(b"abc", 2), 0);
    assert_eq!(cut_tail("a\u{e9}".as_bytes(), 3), 0);
    assert_eq!(cut_tail(b"a\xff\xff", 2), 0);
    // Review round 4's A3, Codex's case: the code point spans two
    // variables, its lead the last byte of `{atlas}` (1,024 bytes), the
    // stop one byte into `{sprite}`: two bytes cut.
    let t = Template::parse("{atlas}{sprite}", TemplateKind::AtlasSprite).unwrap();
    let mut atlas = [b"a/".repeat(511), b"x".to_vec()].concat();
    atlas.push(0xF0);
    let sprite = b"\x9f\x98\x80z";
    let value = |v: Var| match v {
        Var::Atlas => atlas.as_slice(),
        _ => sprite.as_slice(),
    };
    assert_eq!(t.expand(value, &mut out), 2);
    // The stop on the boundary of the two, the code point past it: its
    // lead and second byte before the stop, read on into `{sprite}`.
    atlas.push(0x9F);
    let value = |v: Var| match v {
        Var::Atlas => atlas.as_slice(),
        _ => &sprite[1..],
    };
    assert_eq!(t.expand(value, &mut out), 2);
    // A `safe_` piece after a lead: ASCII, so the lead is invalid.
    let t = Template::parse("{atlas}{safe_sprite}", TemplateKind::AtlasSprite).unwrap();
    let value = |v: Var| match v {
        Var::Atlas => atlas.as_slice(),
        _ => sprite.as_slice(),
    };
    assert_eq!(t.expand(value, &mut out), 0);
}

#[test]
fn an_expansion_stops_one_byte_past_the_bound() {
    let t = Template::parse("a/{atlas}/b", TemplateKind::AtlasJson).unwrap();
    let big = vec![b'x'; 5000];
    assert_eq!(expanded(&t, |_| &big).len(), STOP);
    assert_eq!(expanded(&t, |_| b"n"), b"a/n/b");
    // A `safe_` variable of a huge value is made only as far as the
    // room: the same bytes as the whole value's `safe()`.
    let t = Template::parse("{safe_atlas}", TemplateKind::AtlasJson).unwrap();
    let huge = [b"..".as_slice(), &vec![b'a'; 1 << 24], b"..."].concat();
    assert_eq!(expanded(&t, |_| &huge), vec![b'a'; STOP]);
}

#[test]
fn safe_into_is_safes_prefix() {
    let cases: [&[u8]; 9] = [
        b"..x..",
        b"a..b...",
        b"...",
        b"",
        b"a b/c",
        "\u{e4}tlas".as_bytes(),
        b"bad\xff\xfe..",
        b"\xe2\x82.",
        b".a.",
    ];
    for bytes in cases {
        let whole = safe(bytes);
        for room in 0..=whole.len() + 1 {
            let mut out = b"pre".to_vec();
            safe_into(bytes, room, &mut out);
            let want = &whole.as_bytes()[..room.min(whole.len())];
            assert_eq!(&out[3..], want, "{bytes:?} at {room}");
        }
    }
}

#[test]
fn a_key_past_the_bound_is_decided_on_its_first_1025_bytes() {
    let at = |n: usize, tail: &[u8]| {
        let mut k = vec![b'a'; n];
        for (i, b) in k.iter_mut().enumerate() {
            if i % 200 == 199 {
                *b = b'/';
            }
        }
        k.extend_from_slice(tail);
        check(&k).unwrap_err()
    };
    // A control character within the first 1,025 bytes is reported;
    // one past them is not seen, and the key is too long.
    assert_eq!(
        at(1001, b"\tzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
        KeyRule::ControlChar
    );
    assert_eq!(at(1025, b"\t"), KeyRule::TooLong);
    // The end is past what is read: no trailing slash, no last dot
    // segment, no cut code point.
    assert_eq!(at(1024, b"/"), KeyRule::TooLong);
    assert_eq!(at(1023, b"/."), KeyRule::TooLong);
    assert_eq!(at(1024, "\u{e9}".as_bytes()), KeyRule::TooLong);
    // A dot segment the bytes settle is reported.
    assert_eq!(
        at(1001, b"/../aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        KeyRule::DotSegment
    );
    let mut seg = vec![b'a'; 300];
    seg.extend(vec![b'b'; 900]);
    assert_eq!(check(&seg).unwrap_err(), KeyRule::SegmentTooLong);
}

#[test]
fn a_cut_code_point_still_counts_in_its_segments_length() {
    // Review round 2's C4: 769 bytes of short segments, then 254 bytes
    // and a four-byte character. The stop at 1,025 cuts the character
    // after two of its bytes, so the last segment read is 256 bytes:
    // `segment_too_long`, before `too_long`.
    let key = |head: &[u8], x: usize| {
        let mut k = head.to_vec();
        k.extend(b"a/".repeat(383));
        k.extend(vec![b'x'; x]);
        k.extend("\u{1f600}".as_bytes());
        k
    };
    let long = key(b"aa/", 254);
    assert_eq!(long.len(), 1027);
    assert_eq!(check(&long), Err(KeyRule::SegmentTooLong));
    assert_eq!(check_member(&long), Err(KeyRule::SegmentTooLong));
    // One byte fewer in the segment: 255 bytes read, and too long.
    let short = key(b"aaa/", 253);
    assert_eq!(check(&short), Err(KeyRule::TooLong));
    assert_eq!(check_member(&short), Err(KeyRule::TooLong));
}

#[test]
fn a_key_is_checked_then_checked_again_after_the_prefix() {
    let mut scratch = Vec::with_capacity(STOP);
    let at = scratch.as_ptr();
    assert_eq!(
        check_prefixed(b"a.png", "out/", &mut scratch).unwrap(),
        "a.png"
    );
    assert_eq!(
        check_prefixed(b"a.png", "_unityrs/", &mut scratch).unwrap_err(),
        KeyRule::ReservedPrefix
    );
    let long = format!("{}c", "ab/".repeat(333));
    assert_eq!(
        check_prefixed(long.as_bytes(), &"p/".repeat(20), &mut scratch).unwrap_err(),
        KeyRule::TooLong
    );
    assert_eq!(scratch.as_ptr(), at, "the check grew its buffer");
    // The key's own refusal comes first.
    assert_eq!(
        check_prefixed(b"a//b", "_unityrs/", &mut scratch).unwrap_err(),
        KeyRule::EmptySegment
    );
}

#[test]
fn a_member_path_takes_every_rule_but_the_reserved_ones() {
    assert_eq!(check_member(b"_unityrs/a").unwrap(), "_unityrs/a");
    assert_eq!(
        check_member(b"a/.unityrs-tmp-1").unwrap(),
        "a/.unityrs-tmp-1"
    );
    assert_eq!(check_member(b"a/").unwrap_err(), KeyRule::TrailingSlash);
}

#[test]
fn safe_replaces_per_code_point_and_strips_dots() {
    assert_eq!(safe("\u{e4}tlas".as_bytes()), "_tlas");
    assert_eq!(safe(b"a b/c"), "a_b_c");
    assert_eq!(safe(b"..x.."), "x");
    assert_eq!(safe(b"..."), "unnamed");
    assert_eq!(safe(b""), "unnamed");
    assert_eq!(safe(b"bad\xff\xfe"), "bad__");
    assert_eq!(safe(b"\xe2\x82"), "__");
    assert_eq!(safe("\u{7cbe}\u{7075}".as_bytes()), "__");
    assert_eq!(safe(b"a..b."), "a..b");
}

#[test]
fn templates_parse_braces_and_refuse_unknown_variables() {
    let t = Template::parse("{{x}}/{atlas}-{sprite}.png", TemplateKind::AtlasSprite).unwrap();
    let key = expanded(&t, |v| match v {
        Var::Atlas => b"a",
        Var::Sprite => b"s",
        _ => b"",
    });
    assert_eq!(key, b"{x}/a-s.png");
    assert_eq!(
        Template::parse("{nope}", TemplateKind::AtlasJson),
        Err(TemplateError::Unknown("nope".to_owned()))
    );
    assert_eq!(
        Template::parse("{sprite}.json", TemplateKind::AtlasJson),
        Err(TemplateError::NotAllowed("sprite".to_owned()))
    );
    assert_eq!(
        Template::parse("{name}", TemplateKind::AtlasSprite),
        Err(TemplateError::NotAllowed("name".to_owned()))
    );
    assert_eq!(
        Template::parse("a}", TemplateKind::Texture),
        Err(TemplateError::Brace(1))
    );
    assert_eq!(
        Template::parse("a{b", TemplateKind::Texture),
        Err(TemplateError::Brace(1))
    );
    assert_eq!(
        Template::parse("", TemplateKind::Texture),
        Err(TemplateError::Empty)
    );
    assert!(Template::parse("{type}/{safe_name}_{path_id}", TemplateKind::Texture).is_ok());
}

#[test]
fn the_first_rule_broken_names_the_refusal() {
    let bad = |k: &[u8]| check(k).unwrap_err();
    assert_eq!(bad(b""), KeyRule::Empty);
    assert_eq!(bad(b"a\xff/"), KeyRule::NotUtf8);
    assert_eq!(bad(b"/a\tb"), KeyRule::ControlChar);
    assert_eq!(bad(b"a\x7f"), KeyRule::ControlChar);
    assert_eq!(bad(b"/a//"), KeyRule::LeadingSlash);
    assert_eq!(bad(b"a//"), KeyRule::TrailingSlash);
    assert_eq!(bad(b"a//b"), KeyRule::EmptySegment);
    assert_eq!(bad(b"a/../b"), KeyRule::DotSegment);
    assert_eq!(bad(b"./b"), KeyRule::DotSegment);
    assert_eq!(bad(&[b'x'; 256]), KeyRule::SegmentTooLong);
    let long: Vec<u8> = [b"a/".as_slice(); 513].concat();
    assert_eq!(bad(&long[..1025]), KeyRule::TooLong);
    assert_eq!(bad(b"_unityrs"), KeyRule::ReservedPrefix);
    assert_eq!(bad(b"_unityrs/x"), KeyRule::ReservedPrefix);
    assert_eq!(bad(b"a/.unityrs-tmp-1"), KeyRule::ReservedName);
    assert_eq!(check(b"_unityrsx/a.png").unwrap(), "_unityrsx/a.png");
    assert_eq!(check(b"..a/b.").unwrap(), "..a/b.");
}
