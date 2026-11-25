use std::collections::HashMap;

/// Shared, zero-dependency RFC 8259 JSON parser & serializer used by
/// `storage_engine.rs` and `extensions.rs` (eliminates duplicated codec code).
#[derive(Debug, Clone)]
pub enum JsonVal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(Vec<(String, JsonVal)>),
}

pub fn parse_json(input: &str) -> Option<JsonVal> {
    let b = input.as_bytes();
    let mut cur = 0usize;
    parse_val(b, &mut cur, 0)
}

fn skip_ws(b: &[u8], cur: &mut usize) {
    while *cur < b.len() && b[*cur].is_ascii_whitespace() {
        *cur += 1;
    }
}

fn parse_val(b: &[u8], cur: &mut usize, depth: usize) -> Option<JsonVal> {
    if depth > 64 {
        return None;
    }
    skip_ws(b, cur);
    if *cur >= b.len() {
        return None;
    }
    match b[*cur] {
        b'n' if b[*cur..].starts_with(b"null") => {
            *cur += 4;
            Some(JsonVal::Null)
        }
        b't' if b[*cur..].starts_with(b"true") => {
            *cur += 4;
            Some(JsonVal::Bool(true))
        }
        b'f' if b[*cur..].starts_with(b"false") => {
            *cur += 5;
            Some(JsonVal::Bool(false))
        }
        b'"' => Some(JsonVal::Str(parse_str(b, cur)?)),
        b'[' => {
            *cur += 1;
            let mut items = Vec::new();
            skip_ws(b, cur);
            if *cur < b.len() && b[*cur] == b']' {
                *cur += 1;
                return Some(JsonVal::Arr(items));
            }
            while *cur < b.len() {
                items.push(parse_val(b, cur, depth + 1)?);
                skip_ws(b, cur);
                if *cur < b.len() && b[*cur] == b',' {
                    *cur += 1;
                } else if *cur < b.len() && b[*cur] == b']' {
                    *cur += 1;
                    break;
                } else {
                    return None;
                }
            }
            Some(JsonVal::Arr(items))
        }
        b'{' => {
            *cur += 1;
            let mut pairs = Vec::new();
            skip_ws(b, cur);
            if *cur < b.len() && b[*cur] == b'}' {
                *cur += 1;
                return Some(JsonVal::Obj(pairs));
            }
            while *cur < b.len() {
                skip_ws(b, cur);
                let key = parse_str(b, cur)?;
                skip_ws(b, cur);
                if *cur >= b.len() || b[*cur] != b':' {
                    return None;
                }
                *cur += 1;
                let val = parse_val(b, cur, depth + 1)?;
                pairs.push((key, val));
                skip_ws(b, cur);
                if *cur < b.len() && b[*cur] == b',' {
                    *cur += 1;
                } else if *cur < b.len() && b[*cur] == b'}' {
                    *cur += 1;
                    break;
                } else {
                    return None;
                }
            }
            Some(JsonVal::Obj(pairs))
        }
        _ => {
            let start = *cur;
            while *cur < b.len()
                && (b[*cur].is_ascii_digit() || matches!(b[*cur], b'-' | b'+' | b'.' | b'e' | b'E'))
            {
                *cur += 1;
            }
            let s = std::str::from_utf8(&b[start..*cur]).ok()?;
            if s.contains('.') || s.contains('e') || s.contains('E') {
                Some(JsonVal::Float(s.parse::<f64>().unwrap_or(0.0)))
            } else {
                Some(JsonVal::Int(s.parse::<i64>().unwrap_or(0)))
            }
        }
    }
}

fn parse_str(b: &[u8], cur: &mut usize) -> Option<String> {
    if *cur >= b.len() || b[*cur] != b'"' {
        return None;
    }
    *cur += 1;
    let mut out = String::new();
    while *cur < b.len() {
        let byte = b[*cur];
        if byte == b'"' {
            *cur += 1;
            return Some(out);
        }
        if byte == b'\\' {
            *cur += 1;
            if *cur >= b.len() {
                return None;
            }
            match b[*cur] {
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'b' => out.push('\u{0008}'),
                b'f' => out.push('\u{000C}'),
                b'u' if *cur + 4 < b.len() => {
                    let hex = std::str::from_utf8(&b[*cur + 1..*cur + 5]).ok()?;
                    if let Ok(cp) = u32::from_str_radix(hex, 16) {
                        if let Some(ch) = char::from_u32(cp) {
                            out.push(ch);
                        }
                    }
                    *cur += 4;
                }
                other => out.push(other as char),
            }
            *cur += 1;
        } else {
            let rest = std::str::from_utf8(&b[*cur..]).ok()?;
            let ch = rest.chars().next()?;
            out.push(ch);
            *cur += ch.len_utf8();
        }
    }
    None
}

pub fn json_get<'a>(pairs: &'a [(String, JsonVal)], key: &str) -> Option<&'a JsonVal> {
    pairs.iter().find_map(|(k, v)| if k == key { Some(v) } else { None })
}

pub fn json_get_str(pairs: &[(String, JsonVal)], key: &str) -> Option<String> {
    match json_get(pairs, key)? {
        JsonVal::Str(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn json_get_bool(pairs: &[(String, JsonVal)], key: &str) -> Option<bool> {
    match json_get(pairs, key)? {
        JsonVal::Bool(b) => Some(*b),
        _ => None,
    }
}

pub fn json_get_i64(pairs: &[(String, JsonVal)], key: &str) -> Option<i64> {
    match json_get(pairs, key)? {
        JsonVal::Int(n) => Some(*n),
        JsonVal::Float(f) => Some(*f as i64),
        _ => None,
    }
}

pub fn json_get_str_array(pairs: &[(String, JsonVal)], key: &str) -> Vec<String> {
    let Some(JsonVal::Arr(arr)) = json_get(pairs, key) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| match v {
            JsonVal::Str(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

pub fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

pub fn map_to_json_object(map: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut out = String::from("{");
    for (idx, k) in keys.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let v = &map[*k];
        out.push('"');
        out.push_str(&escape_json_str(k));
        out.push_str("\":\"");
        out.push_str(&escape_json_str(v));
        out.push('"');
    }
    out.push('}');
    out
}

pub fn map_to_pretty_json(map: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut out = String::from("{\n");
    for (idx, k) in keys.iter().enumerate() {
        if idx > 0 {
            out.push_str(",\n");
        }
        let v = &map[*k];
        out.push_str("  \"");
        out.push_str(&escape_json_str(k));
        out.push_str("\": \"");
        out.push_str(&escape_json_str(v));
        out.push('"');
    }
    out.push_str("\n}\n");
    out
}
