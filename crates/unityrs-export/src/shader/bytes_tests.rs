//! Python current-value lists reach `bytes()` without serialized U8 packing.
use super::{Error, bytes};
use unityrs_core::{Value, limits::Meter};

#[test]
fn current_byte_lists_and_tuples_keep_python_byte_range_errors() {
    let meter = Meter::unlimited();
    for input in [
        Value::List(vec![Value::Int(0), Value::UInt(255), Value::Bool(true)]),
        Value::Tuple(vec![Value::Int(0), Value::UInt(255), Value::Bool(true)].into()),
    ] {
        assert_eq!(bytes(&input, &meter).unwrap().as_ref(), &[0, 255, 1]);
    }
    for input in [Value::Int(-1), Value::UInt(256)] {
        let error = bytes(&Value::List(vec![input]), &meter).unwrap_err();
        assert_eq!(error.exception(), Some("ValueError"));
        assert_eq!(error.to_string(), "bytes must be in range(0, 256)");
    }
    let error = bytes(&Value::List(vec![Value::F64(0f64.to_bits())]), &meter).unwrap_err();
    assert!(matches!(error, Error::IntegerType("float")));
}

#[test]
fn byte_conversion_retains_the_output_charge() {
    let meter = Meter::unlimited();
    let bytes = bytes(
        &Value::List((0..100).map(|_| Value::Int(1)).collect()),
        &meter,
    )
    .unwrap();
    assert!(meter.used() >= 100);
    drop(bytes);
    assert_eq!(meter.used(), 0);
}
