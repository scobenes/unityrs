//! Decode one bounded tree. Values and keys each cost a node (max(32, line cap / 64)),
//! decoded UTF-8 keys and strings share line cap / 2 bytes, and depth is 32
//! (root zero). Only scalar tokens use serde; no unrestricted tree is built.
use serde_json::{Map, Value};

pub struct Error {
    pub id: Option<Value>,
    pub code: i32,
    pub message: &'static str,
}
type Result<T> = std::result::Result<T, Error>;
fn syntax() -> Error {
    Error {
        id: Some(Value::Null),
        code: -32700,
        message: "Parse error",
    }
}
fn limit(message: &'static str) -> Error {
    Error {
        id: Some(Value::Null),
        code: -32600,
        message,
    }
}
pub fn request(bytes: &[u8], cap: usize) -> Result<Value> {
    let mut decoder = Decoder {
        bytes,
        at: 0,
        nodes: (cap / 64).max(32),
        strings: cap / 2,
        params: None,
    };
    let mut value = decoder.value(0, false)?;
    decoder.space();
    if decoder.at != bytes.len() {
        return Err(syntax());
    }
    if let Some((start, end)) = decoder.params {
        let identified = value.get("jsonrpc").is_some_and(|v| v == "2.0")
            && value.get("method").is_some_and(Value::is_string)
            && value
                .get("id")
                .is_none_or(|v| v.is_string() || v.is_i64() || v.is_u64());
        decoder.bytes = bytes.get(start..end).ok_or_else(syntax)?;
        decoder.at = 0;
        let params = decoder
            .value(1, true)
            .and_then(|params| {
                decoder.space();
                if decoder.at == decoder.bytes.len() {
                    Ok(params)
                } else {
                    Err(syntax())
                }
            })
            .map_err(|mut e| {
                if identified {
                    e.id = value.get("id").cloned();
                }
                e
            })?;
        if let Some(object) = value.as_object_mut() {
            object.insert("params".into(), params);
        }
    }
    Ok(value)
}
struct Decoder<'a> {
    bytes: &'a [u8],
    at: usize,
    nodes: usize,
    strings: usize,
    params: Option<(usize, usize)>,
}
impl Decoder<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.at += 1;
        }
    }
    fn node(&mut self) -> Result<()> {
        self.nodes = self
            .nodes
            .checked_sub(1)
            .ok_or_else(|| limit("Request node count exceeds max(32, line cap / 64)"))?;
        Ok(())
    }
    fn take(&mut self, byte: u8) -> bool {
        self.space();
        if self.bytes.get(self.at) != Some(&byte) {
            return false;
        }
        self.at += 1;
        true
    }
    fn string(&mut self) -> Result<String> {
        self.space();
        let start = self.at;
        if !self.take(b'"') {
            return Err(syntax());
        }
        loop {
            match self.bytes.get(self.at) {
                Some(b'"') => {
                    self.at += 1;
                    break;
                }
                Some(b'\\') => self.at += 2,
                Some(_) => self.at += 1,
                None => return Err(syntax()),
            }
        }
        // An escaped Unicode scalar costs at most six source bytes per UTF-8
        // byte. Reject before allocating even a temporary decoded token.
        let token = self.bytes.get(start..self.at).ok_or_else(syntax)?;
        if token.len().saturating_sub(2) > self.strings.saturating_mul(6) {
            return Err(limit("Request string bytes exceed line cap / 2"));
        }
        let text: String = serde_json::from_slice(token).map_err(|_| syntax())?;
        self.strings = self
            .strings
            .checked_sub(text.len())
            .ok_or_else(|| limit("Request string bytes exceed line cap / 2"))?;
        Ok(text)
    }
    fn value(&mut self, depth: usize, under_params: bool) -> Result<Value> {
        if depth > 32 {
            return Err(limit("Request depth exceeds 32"));
        }
        self.node()?;
        self.space();
        match self.bytes.get(self.at) {
            Some(b'"') => self.string().map(Value::String),
            Some(b'{') => self.object(depth, under_params),
            Some(b'[') => self.array(depth, under_params),
            Some(_) => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|b| {
                    !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}')
                }) {
                    self.at += 1;
                }
                let scalar: Value =
                    serde_json::from_slice(self.bytes.get(start..self.at).ok_or_else(syntax)?)
                        .map_err(|_| syntax())?;
                if scalar.is_array() || scalar.is_object() || scalar.is_string() {
                    return Err(syntax());
                }
                Ok(scalar)
            }
            None => Err(syntax()),
        }
    }
    fn array(&mut self, depth: usize, under_params: bool) -> Result<Value> {
        self.at += 1;
        let mut values = Vec::new();
        if self.take(b']') {
            return Ok(Value::Array(values));
        }
        loop {
            let value = self.value(depth + 1, under_params)?;
            values
                .try_reserve(1)
                .map_err(|_| limit("Request allocation refused"))?;
            values.push(value);
            if self.take(b']') {
                break;
            }
            if !self.take(b',') {
                return Err(syntax());
            }
        }
        Ok(Value::Array(values))
    }
    // Locate params without building its tree, so envelope members after it
    // can establish identity. The bounded decoder validates this slice next,
    // using the remaining node/string budget and the original depth.
    fn skip_params(&mut self) -> Result<()> {
        let mut nesting = 0usize;
        let mut quoted = false;
        while let Some(&byte) = self.bytes.get(self.at) {
            if quoted {
                match byte {
                    b'\\' => self.at += 1,
                    b'"' => quoted = false,
                    _ => {}
                }
            } else {
                match byte {
                    b'"' => quoted = true,
                    b'{' | b'[' => nesting += 1,
                    b'}' | b']' if nesting > 0 => nesting -= 1,
                    b'}' | b',' if nesting == 0 => return Ok(()),
                    _ => {}
                }
            }
            self.at += 1;
        }
        Err(syntax())
    }
    fn object(&mut self, depth: usize, under_params: bool) -> Result<Value> {
        self.at += 1;
        let mut values = Map::new();
        if self.take(b'}') {
            return Ok(Value::Object(values));
        }
        loop {
            self.node()?;
            let key = self.string()?;
            if values.contains_key(&key) {
                return Err(Error {
                    id: Some(Value::Null),
                    code: if under_params { -32602 } else { -32600 },
                    message: "Duplicate object key in request",
                });
            }
            if !self.take(b':') {
                return Err(syntax());
            }
            let value = if depth == 0 && key == "params" && !under_params {
                self.space();
                let start = self.at;
                self.skip_params()?;
                self.params = Some((start, self.at));
                Value::Null
            } else {
                self.value(depth + 1, under_params)?
            };
            values.insert(key, value);
            if self.take(b'}') {
                break;
            }
            if !self.take(b',') {
                return Err(syntax());
            }
        }
        Ok(Value::Object(values))
    }
}
