use super::*;
use std::os::unix::ffi::OsStringExt;

fn env<'a>(vars: &'a [(&'a str, OsString)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |name| {
        vars.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.clone())
    }
}

fn os(text: &str) -> OsString {
    text.into()
}

fn base() -> Vec<(&'static str, OsString)> {
    vec![
        ("AWS_ACCESS_KEY_ID", os("AKIDEXAMPLE")),
        (
            "AWS_SECRET_ACCESS_KEY",
            os("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
        ),
    ]
}

fn with(name: &'static str, value: OsString) -> Vec<(&'static str, OsString)> {
    let mut vars: Vec<_> = base().into_iter().filter(|(n, _)| *n != name).collect();
    vars.push((name, value));
    vars
}

fn without(name: &str) -> Vec<(&'static str, OsString)> {
    base().into_iter().filter(|(n, _)| *n != name).collect()
}

fn deadline(text: &str) -> DateTime {
    formats::date_time(text).unwrap()
}

#[test]
fn the_two_keys_and_a_token_when_set_and_not_empty() {
    let creds = read(env(&base()), None).unwrap();
    assert_eq!((creds.key_id.len(), creds.secret.len()), (11, 40));
    assert!(creds.token.is_none());
    let creds = read(env(&with("AWS_SESSION_TOKEN", os(""))), None).unwrap();
    assert!(creds.token.is_none());
    let creds = read(env(&with("AWS_SESSION_TOKEN", os("FwoGZX//token=="))), None).unwrap();
    assert_eq!(creds.token.as_ref().map(Secret::len), Some(15));
    let _ = creds.into_provider();
}

#[test]
fn either_key_unset_or_empty_is_missing_and_comes_first() {
    for vars in [
        without("AWS_ACCESS_KEY_ID"),
        without("AWS_SECRET_ACCESS_KEY"),
        with("AWS_ACCESS_KEY_ID", os("")),
        with("AWS_SECRET_ACCESS_KEY", os("")),
        vec![],
    ] {
        assert_eq!(
            read(env(&vars), None).unwrap_err(),
            Preflight::CredentialsMissing
        );
    }
    // Missing before invalid: a bad key id beside a missing secret.
    let mut vars = without("AWS_SECRET_ACCESS_KEY");
    vars[0].1 = OsString::from_vec(vec![0xff]);
    assert_eq!(
        read(env(&vars), None).unwrap_err(),
        Preflight::CredentialsMissing
    );
}

#[test]
fn caps_bytes_and_encoding_are_invalid() {
    let invalid = [
        with("AWS_ACCESS_KEY_ID", os(&"A".repeat(KEY_MAX + 1))),
        with("AWS_SECRET_ACCESS_KEY", os(&"s".repeat(KEY_MAX + 1))),
        with("AWS_SESSION_TOKEN", os(&"t".repeat(TOKEN_MAX + 1))),
        with("AWS_ACCESS_KEY_ID", OsString::from_vec(vec![b'A', 0xff])),
        with("AWS_SECRET_ACCESS_KEY", OsString::from_vec(vec![0xc3])),
        with("AWS_SESSION_TOKEN", OsString::from_vec(vec![0x80])),
        with("AWS_ACCESS_KEY_ID", os("AKID\nX")),
        with("AWS_SESSION_TOKEN", os("tok\u{7f}en")),
        // UTF-8, but `HeaderValue::from_str` would refuse it by panicking.
        with("AWS_ACCESS_KEY_ID", os("AKIDé")),
        with("AWS_SESSION_TOKEN", os("token\u{2028}")),
        // Every credential is printable ASCII, the secret too.
        with("AWS_SECRET_ACCESS_KEY", os("secré")),
        with("AWS_SECRET_ACCESS_KEY", os("sec\u{7f}ret")),
        with("AWS_SECRET_ACCESS_KEY", os("sec\nret")),
        with("AWS_CREDENTIAL_EXPIRATION", os("tomorrow")),
        with("AWS_CREDENTIAL_EXPIRATION", os("")),
        with("AWS_CREDENTIAL_EXPIRATION", os("2026-11-13T25:00:00Z")),
    ];
    for vars in invalid {
        assert_eq!(
            read(env(&vars), None).unwrap_err(),
            Preflight::CredentialsInvalid,
            "{:?}",
            vars.last()
        );
    }
    // At the caps, the whole printable range, and tabs and spaces pass.
    let printable: String = (0x20_u8..0x7f).map(char::from).collect();
    for vars in [
        with("AWS_ACCESS_KEY_ID", os(&"A".repeat(KEY_MAX))),
        with("AWS_SECRET_ACCESS_KEY", os(&"s".repeat(KEY_MAX))),
        with("AWS_SECRET_ACCESS_KEY", os(&printable)),
        with("AWS_ACCESS_KEY_ID", os(&printable)),
        with("AWS_SECRET_ACCESS_KEY", os("a b\tc")),
        with("AWS_SESSION_TOKEN", os(&"t".repeat(TOKEN_MAX))),
        with("AWS_SESSION_TOKEN", os("a b\tc")),
    ] {
        assert!(read(env(&vars), None).is_ok());
    }
}

#[test]
fn an_expiration_before_the_deadline_is_refused() {
    let at = deadline("2026-11-13T12:00:00Z");
    for (expiration, verdict) in [
        (
            "2026-11-13T11:59:59.999Z",
            Err(Preflight::CredentialsExpire),
        ),
        ("2026-11-13T12:00:00Z", Ok(())),
        ("2026-11-13T13:00:00+01:00", Ok(())),
        (
            "2026-11-13T12:59:59+01:00",
            Err(Preflight::CredentialsExpire),
        ),
        ("2026-11-13T07:00:00-05:00", Ok(())),
        ("2026-11-14T00:00:00z", Ok(())),
    ] {
        let vars = with("AWS_CREDENTIAL_EXPIRATION", os(expiration));
        assert_eq!(
            read(env(&vars), Some(&at)).map(|_| ()),
            verdict,
            "{expiration}"
        );
        // Without a deadline any valid expiration passes.
        assert!(read(env(&vars), None).is_ok());
    }
}

#[test]
fn instants_follow_the_calendar_and_offsets() {
    assert_eq!(instant(&deadline("1970-01-01T00:00:00Z")), 0);
    assert_eq!(
        instant(&deadline("2000-03-01T00:00:00Z")),
        951_868_800 * 1_000_000_000
    );
    assert_eq!(
        instant(&deadline("2016-12-31T23:59:60Z")),
        instant(&deadline("2017-01-01T00:00:00Z"))
    );
    assert_eq!(
        instant(&deadline("2026-11-13T00:00:00+14:00")),
        instant(&deadline("2026-11-12T10:00:00Z"))
    );
    assert_eq!(instant(&deadline("1969-12-31T23:59:59.5Z")), -500_000_000);
}

#[test]
fn a_credential_renders_its_length_alone() {
    let mut vars = base();
    vars.push(("AWS_SESSION_TOKEN", os("sentinel-token")));
    let creds = read(env(&vars), None).unwrap();
    let text = format!(
        "{creds:?} {} {} {}",
        creds.key_id,
        creds.secret,
        creds.token.as_ref().unwrap()
    );
    assert_eq!(
        text,
        "Credentials { key_id: <11 bytes>, secret: <40 bytes>, token: Some(<14 bytes>) } <11 bytes> <40 bytes> <14 bytes>"
    );
}
