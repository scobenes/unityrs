use unityrs_mesh::pyfmt::{PyFmtBuf, PyNum, f4, g9};

#[test]
fn python_precision_sign_specials_and_type_tags() {
    let mut b = PyFmtBuf::default();
    for (x, s) in [
        (0., "0"),
        (-0., "-0"),
        (1e-5, "1E-05"),
        (1e-4, "0.0001"),
        (1e8, "100000000"),
        (1e9, "1E+09"),
        (1.234_567_885, "1.23456788"),
        (f64::NAN, "NAN"),
        (-f64::NAN, "NAN"),
        (f64::INFINITY, "INF"),
        (f64::NEG_INFINITY, "-INF"),
        (f64::from_bits(1), "4.94065646E-324"),
    ] {
        assert_eq!(g9(PyNum::Float(x), &mut b), s, "{x:?}");
    }
    assert_eq!(g9(PyNum::Int(0), &mut b), "0");
    for (x, s) in [
        (0., "0.0000"),
        (-0., "-0.0000"),
        (0.03125, "0.0312"),
        (0.09375, "0.0938"),
        (f64::NAN, "nan"),
        (-f64::NAN, "nan"),
        (f64::INFINITY, "inf"),
    ] {
        assert_eq!(f4(x, &mut b), s);
    }
    assert_eq!(f4(f64::MAX, &mut b).len(), 314);
}

#[test]
fn formats_equal_cpython_oracle_grid() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pyfmt.json")).unwrap();
    let mut b = PyFmtBuf::default();
    for case in fixture["cases"].as_array().unwrap() {
        let n = u64::from_str_radix(case["bits"].as_str().unwrap(), 16).unwrap();
        let x = f64::from_bits(n);
        assert_eq!(g9(PyNum::Float(x), &mut b), case["g9"], "{n:016x}");
        assert_eq!(f4(x, &mut b), case["f4"], "{n:016x}");
    }
}
