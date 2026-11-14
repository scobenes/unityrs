use super::*;

fn rewritten(body: &[u8]) -> Vec<u8> {
    let (in_text, _, _) = count(body);
    let mut out = Vec::with_capacity(body.len() + GROWTH * in_text);
    rewrite(body, &mut out);
    assert_eq!(out.len(), body.len() + GROWTH * in_text);
    out
}

#[test]
fn words_in_character_data_take_a_reference() {
    let body = b"<R><Key>out/SlowDown.zip</Key><ETag>\"InternalError-SlowDownSlowDown\"</ETag></R>";
    let (in_text, in_raw, scan) = count(body);
    assert_eq!((in_text, in_raw), (4, 4));
    assert_eq!(scan.root(), Root::Other);
    assert_eq!(
        rewritten(body),
        b"<R><Key>out/&#83;lowDown.zip</Key><ETag>\"&#73;nternalError-&#83;lowDown&#83;lowDown\"</ETag></R>"
    );
}

#[test]
fn words_outside_character_data_are_counted_apart() {
    for body in [
        &b"<R a='SlowDown'><Key>k</Key></R>"[..],
        b"<R><!-- SlowDown --><Key>k</Key></R>",
        b"<R><Key><![CDATA[SlowDown]]></Key></R>",
        b"<R><InternalError/></R>",
        b"<?pi InternalError?><R/>",
    ] {
        let (in_text, in_raw, _) = count(body);
        assert_eq!(
            (in_text, in_raw),
            (0, 1),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
    // A word cut by markup is no word in either count.
    assert_eq!(count(b"<R>Slow<!---->Down</R>").0, 0);
    assert_eq!(count(b"<R>Slow<x/>Down</R>").1, 0);
    // Text that is no word stays as it is.
    assert_eq!(
        rewritten(b"<R>SlowDow InternalErro SSlowDown</R>"),
        b"<R>SlowDow InternalErro S&#83;lowDown</R>"
    );
}

#[test]
fn the_root_decides() {
    assert_eq!(
        count(b"<?xml version=\"1.0\"?><!-- <Error> --><Error><Code>InternalError</Code></Error>")
            .2
            .root(),
        Root::Error
    );
    assert_eq!(
        count(b"<CompleteMultipartUploadResult><Error/></CompleteMultipartUploadResult>")
            .2
            .root(),
        Root::Other
    );
    assert_eq!(count(b"").2.root(), Root::None);
}
