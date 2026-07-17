//! Strict JSON decoding: reject duplicate keys and bound tree size.

use crate::error::ProtoError;
use crate::limits::Limits;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::collections::HashSet;

/// Parse JSON text with Phase 0 strictness: no duplicate keys, bounded tree.
pub fn from_str_strict<T: DeserializeOwned>(input: &str, limits: &Limits) -> Result<T, ProtoError> {
    from_slice_strict(input.as_bytes(), limits)
}

/// Parse JSON bytes with Phase 0 strictness.
pub fn from_slice_strict<T: DeserializeOwned>(
    input: &[u8],
    limits: &Limits,
) -> Result<T, ProtoError> {
    let value = parse_value_strict(input, limits)?;
    serde_json::from_value(value).map_err(|e| ProtoError::SchemaViolation(e.to_string()))
}

/// Parse into a bounded `Value` while rejecting duplicate object keys.
pub fn parse_value_strict(input: &[u8], limits: &Limits) -> Result<Value, ProtoError> {
    let text = std::str::from_utf8(input)
        .map_err(|e| ProtoError::InvalidJson(format!("invalid UTF-8: {e}")))?;
    let mut parser = StrictParser::new(text);
    let value = parser.parse_value()?;
    parser.skip_ws();
    if !parser.at_end() {
        return Err(ProtoError::InvalidJson(
            "trailing data after JSON value".into(),
        ));
    }
    let mut nodes = 0usize;
    check_bounds(&value, limits, 0, &mut nodes)?;
    Ok(value)
}

fn check_bounds(
    value: &Value,
    limits: &Limits,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), ProtoError> {
    if depth > limits.json_max_depth {
        return Err(ProtoError::TreeBound(format!(
            "depth {} exceeds max {}",
            depth, limits.json_max_depth
        )));
    }
    *nodes += 1;
    if *nodes > limits.json_max_nodes {
        return Err(ProtoError::TreeBound(format!(
            "node count exceeds max {}",
            limits.json_max_nodes
        )));
    }
    match value {
        Value::Array(items) => {
            for item in items {
                check_bounds(item, limits, depth + 1, nodes)?;
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                check_bounds(item, limits, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct StrictParser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> StrictParser<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, i: 0 }
    }

    fn at_end(&self) -> bool {
        self.i >= self.s.len()
    }

    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), ProtoError> {
        self.skip_ws();
        match self.peek() {
            Some(c) if c == expected => {
                self.i += c.len_utf8();
                Ok(())
            }
            Some(other) => Err(ProtoError::InvalidJson(format!(
                "expected {expected}, found {other}"
            ))),
            None => Err(ProtoError::InvalidJson(format!(
                "expected {expected}, found end"
            ))),
        }
    }

    fn parse_value(&mut self) -> Result<Value, ProtoError> {
        self.skip_ws();
        let ch = self
            .peek()
            .ok_or_else(|| ProtoError::InvalidJson("unexpected end".into()))?;
        match ch {
            'n' => self.parse_literal("null", Value::Null),
            't' => self.parse_literal("true", Value::Bool(true)),
            'f' => self.parse_literal("false", Value::Bool(false)),
            '"' => Ok(Value::String(self.parse_string()?)),
            '[' => self.parse_array(),
            '{' => self.parse_object(),
            '-' | '0'..='9' => self.parse_number(),
            other => Err(ProtoError::InvalidJson(format!(
                "unexpected character: {other}"
            ))),
        }
    }

    fn parse_literal(&mut self, lit: &str, value: Value) -> Result<Value, ProtoError> {
        if self.s[self.i..].starts_with(lit) {
            self.i += lit.len();
            Ok(value)
        } else {
            Err(ProtoError::InvalidJson(format!("expected literal {lit}")))
        }
    }

    fn parse_string(&mut self) -> Result<String, ProtoError> {
        let start = self.i;
        if self.peek() != Some('"') {
            return Err(ProtoError::InvalidJson("expected string".into()));
        }
        self.i += 1;
        let mut escaped = false;
        while let Some(c) = self.peek() {
            self.i += c.len_utf8();
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == '"' {
                let slice = &self.s[start..self.i];
                let v: Value = serde_json::from_str(slice)
                    .map_err(|e| ProtoError::InvalidJson(e.to_string()))?;
                return match v {
                    Value::String(s) => Ok(s),
                    _ => Err(ProtoError::InvalidJson("expected string value".into())),
                };
            }
        }
        Err(ProtoError::InvalidJson("unterminated string".into()))
    }

    fn parse_number(&mut self) -> Result<Value, ProtoError> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E') {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
        let slice = &self.s[start..self.i];
        serde_json::from_str(slice).map_err(|e| ProtoError::InvalidJson(e.to_string()))
    }

    fn parse_array(&mut self) -> Result<Value, ProtoError> {
        self.expect_char('[')?;
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(']') {
            self.i += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.i += 1;
                }
                Some(']') => {
                    self.i += 1;
                    break;
                }
                Some(other) => {
                    return Err(ProtoError::InvalidJson(format!(
                        "expected ',' or ']' in array, found {other}"
                    )));
                }
                None => return Err(ProtoError::InvalidJson("unterminated array".into())),
            }
        }
        Ok(Value::Array(items))
    }

    fn parse_object(&mut self) -> Result<Value, ProtoError> {
        self.expect_char('{')?;
        self.skip_ws();
        let mut map = Map::new();
        let mut seen = HashSet::new();
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            if !seen.insert(key.clone()) {
                return Err(ProtoError::DuplicateKey(key));
            }
            self.skip_ws();
            self.expect_char(':')?;
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.i += 1;
                }
                Some('}') => {
                    self.i += 1;
                    break;
                }
                Some(other) => {
                    return Err(ProtoError::InvalidJson(format!(
                        "expected ',' or '}}' in object, found {other}"
                    )));
                }
                None => return Err(ProtoError::InvalidJson("unterminated object".into())),
            }
        }
        Ok(Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_keys() {
        let limits = Limits::default();
        let err = parse_value_strict(br#"{"a":1,"a":2}"#, &limits).unwrap_err();
        assert!(matches!(err, ProtoError::DuplicateKey(_)));
    }

    #[test]
    fn accepts_unique_keys() {
        let limits = Limits::default();
        let v = parse_value_strict(br#"{"a":1,"b":2}"#, &limits).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn rejects_depth_overflow() {
        let mut limits = Limits::default();
        limits.json_max_depth = 2;
        let err = parse_value_strict(br#"{"a":{"b":{"c":1}}}"#, &limits).unwrap_err();
        assert!(matches!(err, ProtoError::TreeBound(_)));
    }
}
