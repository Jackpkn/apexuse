use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn object(entries: Vec<(&str, JsonValue)>) -> Self {
        let mut map = BTreeMap::new();
        for (k, v) in entries {
            map.insert(k.to_string(), v);
        }
        JsonValue::Object(map)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Number(n) if n.is_finite() && n.fract() == 0.0 => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        match self {
            JsonValue::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object().and_then(|m| m.get(key))
    }

    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        write_json(self, &mut out);
        out
    }
}

impl fmt::Display for JsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_json_string())
    }
}

fn write_escaped_str(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_json(val: &JsonValue, out: &mut String) {
    match val {
        JsonValue::Null => out.push_str("null"),
        JsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JsonValue::Number(n) => {
            if !n.is_finite() {
                out.push_str("null");
            } else if n.fract() == 0.0 && *n >= (i64::MIN as f64) && *n <= (i64::MAX as f64) {
                out.push_str(&format!("{}", *n as i64));
            } else {
                out.push_str(&format!("{n}"));
            }
        }
        JsonValue::String(s) => write_escaped_str(s, out),
        JsonValue::Array(arr) => {
            out.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        JsonValue::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_escaped_str(k, out);
                out.push(':');
                write_json(v, out);
            }
            out.push('}');
        }
    }
}

pub fn parse_json(input: &str) -> Result<JsonValue, String> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("unexpected trailing content at byte {}", p.pos));
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, expected: &[u8]) -> Result<(), String> {
        if self.bytes[self.pos..].starts_with(expected) {
            self.pos += expected.len();
            Ok(())
        } else {
            Err(format!("expected {:?} at byte {}", expected, self.pos))
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => {
                self.expect(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.expect(b"true")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.expect(b"false")?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'"') => Ok(JsonValue::String(self.parse_string()?)),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(other) => Err(format!("unexpected byte {} at {}", other, self.pos)),
            None => Err("unexpected EOF".into()),
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        if self.pos + 4 > self.bytes.len() {
            return Err("truncated unicode escape".into());
        }
        let s = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
            .map_err(|_| "invalid hex escape".to_string())?;
        self.pos += 4;
        u16::from_str_radix(s, 16).map_err(|_| format!("invalid hex escape: {s}"))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b"\"")?;
        let mut out = String::new();
        while let Some(b) = self.next() {
            match b {
                b'"' => return Ok(out),
                b'\\' => match self.next() {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'b') => out.push('\x08'),
                    Some(b'f') => out.push('\x0c'),
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'u') => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code) {
                            self.expect(b"\\u")?;
                            let low = self.parse_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return Err("invalid surrogate pair".into());
                            }
                            let cp = 0x10000 + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                            let ch = char::from_u32(cp).ok_or_else(|| "invalid codepoint".to_string())?;
                            out.push(ch);
                        } else {
                            let ch = char::from_u32(code as u32).ok_or_else(|| "invalid codepoint".to_string())?;
                            out.push(ch);
                        }
                    }
                    _ => return Err(format!("invalid escape at {}", self.pos)),
                },
                c if c < 0x20 => return Err("unescaped control character in string".into()),
                _ => {
                    // Decode UTF-8 sequence starting at self.pos - 1
                    let start = self.pos - 1;
                    let len = if b < 0x80 {
                        1
                    } else if b & 0xE0 == 0xC0 {
                        2
                    } else if b & 0xF0 == 0xE0 {
                        3
                    } else if b & 0xF8 == 0xF0 {
                        4
                    } else {
                        return Err("invalid UTF-8 leading byte".into());
                    };
                    let end = start + len;
                    if end > self.bytes.len() {
                        return Err("truncated UTF-8 sequence".into());
                    }
                    let slice = std::str::from_utf8(&self.bytes[start..end])
                        .map_err(|_| "invalid UTF-8".to_string())?;
                    out.push_str(slice);
                    self.pos = end;
                }
            }
        }
        Err("unterminated string".into())
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(format!("invalid number at {start}")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("missing fractional digits".into());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("missing exponent digits".into());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| "bad number")?;
        let n: f64 = s.parse().map_err(|_| format!("invalid float: {s}"))?;
        Ok(JsonValue::Number(n))
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.expect(b"[")?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b']') => return Ok(JsonValue::Array(items)),
                _ => return Err(format!("expected ',' or ']' at {}", self.pos)),
            }
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.expect(b"{")?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b":")?;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b'}') => return Ok(JsonValue::Object(map)),
                _ => return Err(format!("expected ',' or '}}' at {}", self.pos)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_unicode_and_nested() {
        let src = r#"{"ok":true,"count":42,"ratio":-1.5,"msg":"hello \u2022 world\n\"hi\"","pair":"\uD83D\uDE80","arr":[null,false,"x"]}"#;
        let parsed = parse_json(src).unwrap();
        assert_eq!(parsed.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(parsed.get("count").and_then(|v| v.as_i64()), Some(42));
        assert_eq!(parsed.get("pair").and_then(|v| v.as_str()), Some("🚀"));
        let reparsed = parse_json(&parsed.to_json_string()).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn rejects_trailing_junk() {
        assert!(parse_json("{}{}").is_err());
        assert!(parse_json(r#"{"a":}"#).is_err());
    }
}
