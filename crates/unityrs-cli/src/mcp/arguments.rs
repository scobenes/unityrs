//! Explain the first rejected tool argument using the advertised schema.
use super::schema;
use serde_json::Value;
pub fn check(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    if schema::valid(value, schema) {
        return Ok(());
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!(
                        "Argument {key}=<missing>; allowed: required {}",
                        allowed(
                            schema
                                .get("properties")
                                .and_then(|p| p.get(key))
                                .unwrap_or(&Value::Null)
                        )
                    ));
                }
            }
        }
        if let Some(properties) = schema["properties"].as_object() {
            for (key, value) in object {
                if let Some(s) = properties.get(key) {
                    check(value, s, key)?;
                } else if schema["additionalProperties"] == false {
                    return Err(format!(
                        "Argument {key}={value}; allowed argument names: {}",
                        properties.keys().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
        }
    }
    if let (Some(values), Some(items)) = (value.as_array(), schema.get("items")) {
        for (i, value) in values.iter().enumerate() {
            check(value, items, &format!("{path}[{i}]"))?;
        }
    }
    Err(format!(
        "Argument {path}={value}; allowed: {}",
        allowed(schema)
    ))
}
fn allowed(schema: &Value) -> String {
    let mut parts = vec![schema["type"].as_str().unwrap_or("value").to_owned()];
    for key in ["minimum", "maximum", "enum", "minItems", "minLength"] {
        if let Some(value) = schema.get(key) {
            parts.push(format!("{key}={value}"));
        }
    }
    parts.join(", ")
}

pub fn extraction(arguments: &Value) -> Result<(), String> {
    for key in ["output", "report"] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str)
            && (value.is_empty()
                || unityrs_export::extractor::paths::validate(value.as_bytes()).is_err()
                || (key == "report"
                    && value
                        .rsplit('/')
                        .next()
                        .is_none_or(|leaf| leaf.is_empty() || leaf == ".")))
        {
            return Err(format!(
                "Argument {key}={}; allowed: nonempty relative path without parent components, NUL, backslash or reserved colon; report must end in a filename",
                arguments[key]
            ));
        }
    }
    if let Some(values) = arguments["asset_filter"].as_array() {
        for (i, value) in values.iter().enumerate() {
            let text = value.as_str().unwrap_or_default();
            if text.parse::<i32>().is_err()
                && !unityrs_core::classid::CLASS_NAMES
                    .iter()
                    .any(|(_, name)| *name == text)
            {
                return Err(format!(
                    "Argument asset_filter[{i}]={value}; allowed: known class name or integer class ID in -2147483648..=2147483647"
                ));
            }
        }
    }
    Ok(())
}
