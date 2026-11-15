//! Deep value seeds shared by the value and typetree targets.
use unityrs_testkit::depth::{Depth, Wrap, Z_ROWS};
use unityrs_testkit::fuzz::{RefType, Row, TreeCase};

/// `depth`'s shapes, a few levels deep, through ref types `A` and `Z`.
pub(super) fn cases() -> Vec<(String, TreeCase)> {
    let to_rows = |rows: &[(u8, &str, &str, i32)]| -> Vec<Row> {
        rows.iter()
            .map(|(l, t, n, m)| Row::new(*l, t.as_bytes(), n.as_bytes(), *m))
            .collect()
    };
    let shapes = [
        Wrap::Class,
        Wrap::Vector,
        Wrap::Aligned,
        Wrap::ClassArray,
        Wrap::Pair,
        Wrap::PairArray,
    ];
    let mut out = Vec::new();
    for (i, w) in shapes.iter().enumerate() {
        let next = shapes[(i + 1) % shapes.len()];
        for wraps in [vec![*w], vec![*w, next]] {
            let d = Depth::new(&wraps);
            let a = to_rows(&d.rows);
            let refs = vec![
                RefType {
                    class: b"A".to_vec(),
                    namespace: b"n".to_vec(),
                    assembly: b"a".to_vec(),
                    rows: Some(a.clone()),
                },
                RefType {
                    class: b"Z".to_vec(),
                    namespace: b"n".to_vec(),
                    assembly: b"a".to_vec(),
                    rows: Some(to_rows(&Z_ROWS)),
                },
            ];
            for n in [1, 3] {
                let case = TreeCase {
                    flags: 0,
                    start: 0,
                    rows: a.clone(),
                    refs: refs.clone(),
                    data: d.data(n),
                };
                out.push((format!("depth-{wraps:?}-{n}"), case));
            }
        }
    }
    out
}
