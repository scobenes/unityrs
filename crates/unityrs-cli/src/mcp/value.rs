//! MCP request values only. Numbers retain validated wire text rather than
//! enabling `serde_json` features that change parsing throughout the workspace.
use serde_json::value::RawValue;
use std::{collections::BTreeMap, fmt, ops::Index};

pub enum Value {
    Null,
    Bool(bool),
    Number(Box<RawValue>),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

impl Value {
    pub fn get(&self, key: impl AsRef<str>) -> Option<&Self> {
        self.as_object()?.get(key.as_ref())
    }
    pub fn as_object(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(values) => Some(values),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Self>> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }
    pub fn number_text(&self) -> Option<&str> {
        match self {
            Self::Number(number) => Some(number.get()),
            _ => None,
        }
    }
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }
    pub fn is_boolean(&self) -> bool {
        matches!(self, Self::Bool(_))
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl Index<&str> for Value {
    type Output = Self;
    fn index(&self, key: &str) -> &Self {
        self.get(key).unwrap_or(&Self::Null)
    }
}

// Used for argument diagnostics and forwarding batch job documents. Retain
// number literals even inside nested objects; serde still escapes strings.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Number(value) => f.write_str(value.get()),
            Self::String(value) => write!(f, "{}", serde_json::json!(value)),
            Self::Array(values) => {
                f.write_str("[")?;
                for (i, value) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{value}")?;
                }
                f.write_str("]")
            }
            Self::Object(values) => {
                f.write_str("{")?;
                for (i, (key, value)) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{}:{value}", serde_json::json!(key))?;
                }
                f.write_str("}")
            }
        }
    }
}
