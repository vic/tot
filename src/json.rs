//! Strict JSON Lines record reader with blame paths. Parse, don't validate:
//! every getter returns a value already inside its legal range or an error naming file:line.key.
use crate::{Error, Result};
use serde_json::{Map, Value};

pub struct Rec<'a> {
    file: &'a str,
    line: usize,
    path: String,
    o: &'a Map<String, Value>,
}

/// One JSONL file: head line checked, then (kind, record) per line.
pub fn records<'a>(file: &'a str, text: &str, vals: &'a mut Vec<Value>) -> Result<Vec<(String, Rec<'a>)>> {
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            return Err(Error::Data { file: file.into(), line: i + 1, path: String::new(), msg: "blank line (JSON Lines forbids it)".into() });
        }
        vals.push(serde_json::from_str(l).map_err(|e| Error::Data { file: file.into(), line: i + 1, path: String::new(), msg: e.to_string() })?);
    }
    let mut out = Vec::new();
    for (i, v) in vals.iter().enumerate() {
        let Value::Object(o) = v else {
            return Err(Error::Data { file: file.into(), line: i + 1, path: String::new(), msg: "line is not an object".into() });
        };
        let r = Rec { file, line: i + 1, path: String::new(), o };
        let t = r.text("t")?.to_string();
        if i == 0 {
            r.allow(&["t", "schema_version", "file"])?;
            if t != "head" {
                return Err(r.err("t", "first line must be the head record"));
            }
            r.int("schema_version", 1, 1)?;
            if r.text("file")? != file.trim_end_matches(".jsonl") {
                return Err(r.err("file", "head names another file"));
            }
        } else {
            out.push((t, r));
        }
    }
    Ok(out)
}

impl<'a> Rec<'a> {
    pub fn err(&self, key: &str, msg: impl Into<String>) -> Error {
        Error::Data { file: self.file.into(), line: self.line, path: format!("{}.{key}", self.path), msg: msg.into() }
    }

    pub fn allow(&self, keys: &[&str]) -> Result<()> {
        match self.o.keys().find(|k| !keys.contains(&k.as_str())) {
            Some(k) => Err(self.err(k, format!("unknown key; allowed: {}", keys.join(" ")))),
            None => Ok(()),
        }
    }

    pub fn has(&self, k: &str) -> bool {
        self.o.contains_key(k)
    }

    fn get(&self, k: &str) -> Result<&'a Value> {
        self.o.get(k).ok_or_else(|| self.err(k, "missing"))
    }

    pub fn num(&self, k: &str, lo: f64, hi: f64) -> Result<f64> {
        let v = self.get(k)?.as_f64().ok_or_else(|| self.err(k, "expected number"))?;
        if (lo..=hi).contains(&v) { Ok(v) } else { Err(self.err(k, format!("{v} outside [{lo}, {hi}]"))) }
    }

    pub fn num_or(&self, k: &str, d: f64, lo: f64, hi: f64) -> Result<f64> {
        if self.has(k) { self.num(k, lo, hi) } else { Ok(d) }
    }

    pub fn int(&self, k: &str, lo: i64, hi: i64) -> Result<i64> {
        let v = self.get(k)?.as_i64().ok_or_else(|| self.err(k, "expected integer"))?;
        if (lo..=hi).contains(&v) { Ok(v) } else { Err(self.err(k, format!("{v} outside [{lo}, {hi}]"))) }
    }

    pub fn int_or(&self, k: &str, d: i64, lo: i64, hi: i64) -> Result<i64> {
        if self.has(k) { self.int(k, lo, hi) } else { Ok(d) }
    }

    pub fn text(&self, k: &str) -> Result<&'a str> {
        self.get(k)?.as_str().ok_or_else(|| self.err(k, "expected string"))
    }

    pub fn flag(&self, k: &str) -> Result<bool> {
        self.get(k)?.as_bool().ok_or_else(|| self.err(k, "expected true|false"))
    }

    /// Closed enum from a string.
    pub fn pick<T: Copy>(&self, k: &str, table: &[(&str, T)]) -> Result<T> {
        let s = self.text(k)?;
        table.iter().find(|(n, _)| *n == s).map(|(_, v)| *v).ok_or_else(|| {
            self.err(k, format!("'{s}' not one of: {}", table.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" ")))
        })
    }

    pub fn list(&self, k: &str) -> Result<&'a [Value]> {
        self.get(k)?.as_array().map(Vec::as_slice).ok_or_else(|| self.err(k, "expected array"))
    }

    pub fn sub(&self, k: &str) -> Result<Rec<'a>> {
        self.at(self.get(k)?, k)
    }

    /// Nested object at `v`, blamed as `<path>.<k>`.
    pub fn at(&self, v: &'a Value, k: &str) -> Result<Rec<'a>> {
        match v {
            Value::Object(o) => Ok(Rec { file: self.file, line: self.line, path: format!("{}.{k}", self.path), o }),
            _ => Err(self.err(k, "expected object")),
        }
    }

    /// Single-key object `{"variant": {...}}` or bare string `"variant"`: (variant, body).
    pub fn variant(&self, k: &str) -> Result<(&'a str, Option<Rec<'a>>)> {
        match self.get(k)? {
            Value::String(s) => Ok((s.as_str(), None)),
            Value::Object(o) if o.len() == 1 => match o.iter().next() {
                Some((name, body)) => Ok((name.as_str(), Some(self.at(body, &format!("{k}.{name}"))?))),
                None => Err(self.err(k, "empty variant")),
            },
            _ => Err(self.err(k, "expected \"name\" or {\"name\": {...}}")),
        }
    }

    pub fn strs(&self, k: &str) -> Result<Vec<&'a str>> {
        self.list(k)?.iter().map(|v| v.as_str().ok_or_else(|| self.err(k, "expected array of strings"))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Result<()> {
        let mut vals = Vec::new();
        let recs = records("t.jsonl", text, &mut vals)?;
        for (_, r) in recs {
            r.allow(&["t", "a"])?;
            r.num("a", 0.0, 1.0)?;
        }
        Ok(())
    }

    const HEAD: &str = "{\"t\":\"head\",\"schema_version\":1,\"file\":\"t\"}\n";

    #[test]
    fn blame_path_names_line_and_key() {
        let e = one(&format!("{HEAD}{{\"t\":\"x\",\"a\":0.5}}\n{{\"t\":\"x\",\"a\":3}}")).unwrap_err().to_string();
        assert_eq!(e, "t.jsonl:3 .a: 3 outside [0, 1]");
        assert!(one(&format!("{HEAD}{{\"t\":\"x\",\"a\":0.5}}")).is_ok());
    }

    #[test]
    fn rejects_unknown_key_blank_line_and_bad_head() {
        assert!(one(&format!("{HEAD}{{\"t\":\"x\",\"a\":0.5,\"zz\":1}}")).unwrap_err().to_string().contains(".zz: unknown key"));
        assert!(one(&format!("{HEAD}\n{{\"t\":\"x\",\"a\":0.5}}")).unwrap_err().to_string().contains("blank line"));
        assert!(one("{\"t\":\"head\",\"schema_version\":2,\"file\":\"t\"}").is_err());
    }
}
