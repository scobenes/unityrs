//! Staging is a library capability until runtime/lifetime integration lands.
mod common;
use common::{assert_refused, job, unityrs, write_job};
use unityrs_testkit::scratch::Scratch;

#[test]
fn url_execution_including_an_unused_dependency_stays_78_usage() {
    for sources in [
        r#","inputs":[{"id":"a","url":"https://source.invalid/?query_secret=hidden"}]"#,
        r#","inputs":[{"id":"a","path":"/a"}],"dependencies":[{"id":"unused","url":"https://source.invalid/?query_secret=hidden"}]"#,
    ] {
        let scratch = Scratch::new("cli-url-refused");
        let file = write_job(&scratch, &job(sources));
        let output = unityrs(&["batch", file.to_str().unwrap()]);
        assert_refused(&output, "usage");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("query_secret"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("query_secret"));
    }
}
