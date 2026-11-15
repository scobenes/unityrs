//! Decision 19 permits lewton's infallible allocations only outside batch.
//! Batch never calls the extractor or audio exporter. CR10's third-party
//! admission list in `controller_support/refusals.rs` admits no lewton frame;
//! it remains unchanged. Feature unification may compile lewton into batch
//! test binaries: this guarantee concerns reachability, not linkage.
use std::path::Path;

use proc_macro2::{Spacing, TokenStream, TokenTree};

const FORBIDDEN: &[&str] = &[
    "extractor",
    "export_with_vorbis",
    "decode_with_vorbis",
    "VorbisHeaders",
    "lewton",
];

fn ident_is(token: &TokenTree, name: &str) -> bool {
    if let TokenTree::Ident(ident) = token {
        let ident = ident.to_string();
        ident.strip_prefix("r#").unwrap_or(&ident) == name
    } else {
        false
    }
}

fn forbidden_reference(stream: TokenStream) -> Option<&'static str> {
    let tokens: Vec<_> = stream.into_iter().collect();
    for token in &tokens {
        if let Some(name) = FORBIDDEN.iter().find(|name| ident_is(token, name)) {
            return Some(name);
        }
        if let TokenTree::Group(group) = token
            && let Some(name) = forbidden_reference(group.stream())
        {
            return Some(name);
        }
    }
    tokens.windows(4).find_map(|window| match window {
        [
            audio,
            TokenTree::Punct(first),
            TokenTree::Punct(second),
            export,
        ] if ident_is(audio, "audio")
            && first.as_char() == ':'
            && first.spacing() == Spacing::Joint
            && second.as_char() == ':'
            && ident_is(export, "export") =>
        {
            Some("audio::export")
        }
        _ => None,
    })
}

fn tokenize(source: &str, path: &Path) -> TokenStream {
    source
        .parse::<TokenStream>()
        .unwrap_or_else(|error| panic!("decision 19: cannot tokenize {}: {error}", path.display()))
}

fn scan(dir: &Path) -> (usize, usize) {
    let (mut files, mut tokens) = (0, 0);
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            let (child_files, child_tokens) = scan(&path);
            files += child_files;
            tokens += child_tokens;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files += 1;
            let source = std::fs::read_to_string(&path).unwrap();
            let stream = tokenize(&source, &path);
            // Top-level tokens suffice to rule out an empty scan; the walker
            // also visits every group's contents, including macro arguments.
            tokens += stream.clone().into_iter().count();
            if let Some(token) = forbidden_reference(stream) {
                panic!(
                    "decision 19: batch must never reach Vorbis decoding; {} references {token}",
                    path.display()
                );
            }
        }
    }
    (files, tokens)
}

#[test]
fn decision_19_batch_cannot_reach_vorbis() {
    let (files, tokens) = scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    assert!(files > 0, "no Rust source files scanned");
    assert!(tokens > 0, "no Rust tokens scanned");
}

fn reference_in(source: &str) -> Option<&'static str> {
    forbidden_reference(tokenize(source, Path::new("walker_fixture.rs")))
}

#[test]
fn walker_ignores_comments_and_literals() {
    for token in FORBIDDEN.iter().copied().chain(["audio::export"]) {
        for template in [
            "// $token\nfn safe() {}",
            "/// $token\nfn safe() {}",
            "//! $token\nfn safe() {}",
            "/* $token */ fn safe() {}",
            "/** $token */ fn safe() {}",
            "/*! $token */ fn safe() {}",
            "/* outer /* $token */ still a comment */ fn safe() {}",
            r#"let note = "escaped \" $token // /*";"#,
            r#"let note = b"escaped \" $token // /*";"#,
            r#"let note = c"$token";"#,
            r#"let note = "$token"r;"#,
            r#"let note = "x"r"$token";"#,
            r#"let note = r"$token // /*";"#,
            r###"let note = r##"" $token // /* "# "##suffix;"###,
            r###"let note = br##"" $token // /* "# "##;"###,
            r###"let note = cr##"" $token // /* "# "##;"###,
        ] {
            let source = template.replace("$token", token);
            assert_eq!(reference_in(&source), None, "{source}");
        }
        if token != "audio::export" {
            let source = format!(r#"let note = "x"{token};"#);
            assert_eq!(reference_in(&source), None, "{source}");
        }
    }
}

#[test]
fn walker_matches_whole_identifiers_and_joint_path_separators() {
    for source in [
        "let extractor_paths = 0;",
        "let export_with_vorbis_options = 0;",
        "let decode_with_vorbis_options = 0;",
        "type MyVorbisHeaders = ();",
        "mod lewton_helpers {}",
        "audio::export_more();",
        "my_audio::export();",
        "audio + export;",
        "audio : : export();",
        "audio :/* separator */: export();",
        "audio::(export);",
        "(audio)::export();",
        "extrac/* separator */tor;",
        "let extractoré = 0;",
        "let álewton = 0;",
        "let a\u{301}lewton = 0;",
        "let lewton\u{301} = 0;",
    ] {
        assert_eq!(reference_in(source), None, "{source}");
    }
    for (source, token) in [
        ("use unityrs_export::extractor;", "extractor"),
        ("unityrs_export::extractor::extract_assets();", "extractor"),
        ("audio::export(&clip);", "audio::export"),
        ("audio :: export(&clip);", "audio::export"),
        ("r#audio::r#export(&clip);", "audio::export"),
        ("audio\u{2003}::\u{2003}export();", "audio::export"),
        (
            "unityrs_export::audio /* comment */ :: export();",
            "audio::export",
        ),
        ("lewton::reader()", "lewton"),
        ("r#lewton::x()", "lewton"),
        ("export_with_vorbis();", "export_with_vorbis"),
        ("decode_with_vorbis();", "decode_with_vorbis"),
        ("let _: VorbisHeaders;", "VorbisHeaders"),
        ("use unityrs_export::r#extractor;", "extractor"),
        ("ignore!(lewton::x())", "lewton"),
        ("ignore!({ [lewton::x()] })", "lewton"),
        ("ignore!({ [audio::export()] })", "audio::export"),
        (r#"let q = ['\u{2_2}', '"']; lewton::x();"#, "lewton"),
        (r#"let quote = b'\"'; lewton::reader();"#, "lewton"),
        ("fn read<'a>() { lewton::reader(); }", "lewton"),
        (
            r##"let note = r#"lewton"#; audio::export();"##,
            "audio::export",
        ),
        (r#"let note = "x"r; lewton::x();"#, "lewton"),
    ] {
        assert_eq!(reference_in(source), Some(token), "{source}");
    }
}

#[test]
#[should_panic(expected = "cannot tokenize malformed.rs")]
fn walker_rejects_lexer_errors_with_filename() {
    let _ = tokenize("fn safe() {} /* unterminated", Path::new("malformed.rs"));
}

#[test]
fn walker_scans_shebang_tokens() {
    assert_eq!(reference_in("#!/lewton\nfn safe() {}"), Some("lewton"));
}
