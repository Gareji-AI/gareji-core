//! Structured JSON Pointer redaction for audit records.

use serde_json::Value;
use thiserror::Error;

const REDACTED: &str = "<redacted>";

/// Redaction policy constructed from repeatable CLI flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedactionPolicy {
    /// RFC 6901 JSON Pointers applied to request/response records.
    pub pointers: Vec<String>,
}

/// Invalid pointer syntax (input category before spawn).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RedactionError {
    /// Pointer failed RFC 6901 syntactic validation.
    #[error("invalid RFC 6901 JSON Pointer: {0}")]
    InvalidPointer(String),
}

impl RedactionPolicy {
    /// Parse pointers; empty list means explicit no-redaction policy.
    pub fn from_pointers(pointers: Vec<String>) -> Result<Self, RedactionError> {
        for p in &pointers {
            validate_pointer(p)?;
        }
        Ok(Self { pointers })
    }

    /// Whether zero pointers were configured.
    pub fn is_noop(&self) -> bool {
        self.pointers.is_empty()
    }

    /// Apply redaction in place; returns count of replaced values.
    pub fn apply(&self, value: &mut Value) -> usize {
        let mut count = 0;
        for pointer in &self.pointers {
            if redact_pointer(value, pointer) {
                count += 1;
            }
        }
        count
    }
}

fn validate_pointer(pointer: &str) -> Result<(), RedactionError> {
    if pointer.is_empty() {
        return Err(RedactionError::InvalidPointer("empty pointer".into()));
    }
    if !pointer.starts_with('/') {
        return Err(RedactionError::InvalidPointer(format!(
            "must start with '/': {pointer}"
        )));
    }
    // Tokens may contain ~0 / ~1 escapes; reject bare ~ not followed by 0/1.
    let mut chars = pointer.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.peek() {
                Some('0' | '1') => {
                    chars.next();
                }
                _ => {
                    return Err(RedactionError::InvalidPointer(format!(
                        "invalid ~ escape in {pointer}"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn decode_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn redact_pointer(value: &mut Value, pointer: &str) -> bool {
    let tokens: Vec<String> = if pointer == "/" {
        // Pointer "/" refers to the empty-key member? Per RFC 6901, "" is whole doc,
        // "/" is key "". We treat whole-doc only for "".
        vec![String::new()]
    } else {
        pointer
            .trim_start_matches('/')
            .split('/')
            .map(decode_token)
            .collect()
    };
    if pointer.is_empty() {
        *value = Value::String(REDACTED.into());
        return true;
    }
    let mut cur = value;
    for (idx, token) in tokens.iter().enumerate() {
        let last = idx + 1 == tokens.len();
        match cur {
            Value::Object(map) => {
                if last {
                    if map.contains_key(token) {
                        map.insert(token.clone(), Value::String(REDACTED.into()));
                        return true;
                    }
                    return false;
                }
                match map.get_mut(token) {
                    Some(next) => cur = next,
                    None => return false,
                }
            }
            Value::Array(items) => {
                let Ok(i) = token.parse::<usize>() else {
                    return false;
                };
                if i >= items.len() {
                    return false;
                }
                if last {
                    items[i] = Value::String(REDACTED.into());
                    return true;
                }
                cur = &mut items[i];
            }
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_invalid_pointer_syntax() {
        assert!(RedactionPolicy::from_pointers(vec!["secrets".into()]).is_err());
        assert!(RedactionPolicy::from_pointers(vec!["/~".into()]).is_err());
    }

    #[test]
    fn redacts_configured_fields() {
        let policy = RedactionPolicy::from_pointers(vec!["/token".into()]).unwrap();
        let mut v = json!({"token": "secret", "ok": true});
        assert_eq!(policy.apply(&mut v), 1);
        assert_eq!(v["token"], REDACTED);
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn zero_pointers_is_noop() {
        let policy = RedactionPolicy::from_pointers(vec![]).unwrap();
        assert!(policy.is_noop());
        let mut v = json!({"token": "secret"});
        assert_eq!(policy.apply(&mut v), 0);
    }
}
