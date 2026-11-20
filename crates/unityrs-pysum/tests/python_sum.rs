//! `python_sum` bit for bit against Python 3.15.2's compensated
//! `sum` on random and adversarial vectors (`support/python_sum.json`,
//! written by `tools/parity/vectors/test_anim_python_sum.py`).
use serde_json::Value;
use unityrs_pysum::python_sum;

const VECTORS: &str = include_str!("support/python_sum.json");

fn float(v: &Value) -> f64 {
    f64::from_bits(u64::from_str_radix(v.as_str().unwrap(), 16).unwrap())
}

#[test]
fn python_sum_is_cpythons() {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    let rows = vectors["vectors"].as_array().unwrap();
    assert!(rows.len() > 1900);
    for row in rows {
        let values: Vec<f64> = row["v"].as_array().unwrap().iter().map(float).collect();
        let found = python_sum(values.iter().copied());
        if row["sum"] == "nan" {
            // A NaN's sign and payload are the hardware's.
            assert!(found.is_nan(), "{values:?}: {found}");
        } else {
            assert_eq!(found.to_bits(), float(&row["sum"]).to_bits(), "{values:?}");
        }
    }
    // Plain left-to-right addition differs on cancellation.
    assert_eq!(python_sum([1e16, 1., -1e16]), 1.);
    assert_eq!(python_sum([0.1; 10]), 1.);
}
