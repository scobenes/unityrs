//! Apply our single-file unified diffs at their exact line numbers, without
//! offsets, fuzz, reverse application, or an external patch executable.

fn range(text: &str, sign: char) -> (usize, usize) {
    let (start, count) = text.strip_prefix(sign).unwrap().split_once(',').unwrap();
    (start.parse::<usize>().unwrap() - 1, count.parse().unwrap())
}

pub fn apply(source: &str, patch: &str, name: &str) -> String {
    assert!(source.ends_with('\n') && patch.ends_with('\n'));
    let original: Vec<_> = source.lines().collect();
    let mut lines = patch.lines().skip_while(|l| !l.starts_with("--- "));
    assert_eq!(lines.next().unwrap(), format!("--- a/{name}"));
    assert_eq!(lines.next().unwrap(), format!("+++ b/{name}"));
    let mut lines = lines.peekable();
    let mut result = Vec::new();
    let mut cursor = 0;
    let mut hunks = 0;
    while let Some(header) = lines.next() {
        let mut fields = header.strip_prefix("@@ ").unwrap().split_whitespace();
        let (old_start, old_count) = range(fields.next().unwrap(), '-');
        let (new_start, new_count) = range(fields.next().unwrap(), '+');
        assert_eq!(fields.next(), Some("@@"));
        assert!(old_start >= cursor, "overlapping patch hunks");
        result.extend_from_slice(&original[cursor..old_start]);
        assert_eq!(result.len(), new_start, "patch output position");
        cursor = old_start;
        let mut added = 0;
        while lines.peek().is_some_and(|l| !l.starts_with("@@ ")) {
            let line = lines.next().unwrap();
            let (kind, content) = line.split_at(1);
            match kind {
                " " | "-" => {
                    assert_eq!(original[cursor], content, "patch context: {name}");
                    cursor += 1;
                    if kind == " " {
                        result.push(content);
                        added += 1;
                    }
                }
                "+" => {
                    result.push(content);
                    added += 1;
                }
                _ => panic!("invalid patch line"),
            }
        }
        assert_eq!(cursor - old_start, old_count, "patch source length");
        assert_eq!(added, new_count, "patch output length");
        hunks += 1;
    }
    assert!(hunks > 0, "empty patch");
    result.extend_from_slice(&original[cursor..]);
    result.push("");
    result.join("\n")
}
