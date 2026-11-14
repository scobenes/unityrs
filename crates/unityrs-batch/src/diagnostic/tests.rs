use super::*;
#[test]
fn masks_short_overlapping_and_cross_boundary_secrets_before_cut() {
    let credentials = OnceLock::new();
    credentials.set(["token", "", ""]).unwrap();
    let secrets = Secrets {
        values: &["abc", "bcd", "é", "x", ""],
        credentials: &credentials,
    };
    let mut output = [0; 128];
    let n = secrets.mask("abcd é token x safe", &mut output);
    assert_eq!(&output[..n], b"*** *** *** *** safe");
    let mut short = [0; 4];
    let n = secrets.mask("ab token", &mut short);
    assert_eq!(&short[..n], b"ab ");
    let mut short = [0; 4];
    let n = secrets.mask("a abcdef", &mut short);
    assert_eq!(&short[..n], b"a ");
}
