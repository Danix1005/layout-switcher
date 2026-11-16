//! Мінімальний розбір JSON — лише для відповіді GitHub API про реліз. Без залежностей, з обмеженням глибини.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// Поле об'єкта за назвою (перше з таким ім'ям).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Num(n) if *n >= 0.0 && n.fract() == 0.0 && *n < 9.0e15 => Some(*n as u64),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(v) => Some(v),
            _ => None,
        }
    }
}

const MAX_DEPTH: usize = 32;

pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { s: text, pos: 0 };
    p.skip_ws();
    let v = p.value(0)?;
    p.skip_ws();
    if p.pos != text.len() {
        return Err(p.err("зайві символи після JSON"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, what: &str) -> String {
        format!("JSON: {what} (позиція {})", self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("надто глибока вкладеність"));
        }
        match self.peek() {
            None => Err(self.err("несподіваний кінець")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') if self.eat("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Value::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("несподіваний символ")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.pos += 1; // {
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Obj(items));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("очікував назву поля"));
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("очікував ':'"));
            }
            self.pos += 1;
            self.skip_ws();
            let v = self.value(depth + 1)?;
            items.push((key, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Obj(items));
                }
                _ => return Err(self.err("очікував ',' або '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err(self.err("очікував ',' або ']'")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
            self.pos += 1;
        }
        self.s[start..self.pos].parse::<f64>().map(Value::Num).map_err(|_| self.err("некоректне число"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.pos..self.pos + 4).ok_or_else(|| self.err("обірваний \\u"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.err("некоректний \\u"))?;
        self.pos += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1; // "
        let mut out = String::new();
        loop {
            // шматок до найближчої лапки чи зворотної косої: розрізати рядок по ASCII-байту безпечно
            let rest = &self.s[self.pos..];
            let stop = rest.find(['"', '\\']).ok_or_else(|| self.err("рядок без закриття"))?;
            if rest[..stop].chars().any(|c| (c as u32) < 0x20) {
                return Err(self.err("керівний символ у рядку"));
            }
            out.push_str(&rest[..stop]);
            self.pos += stop;
            let c = self.peek();
            self.pos += 1;
            if c == Some(b'"') {
                return Ok(out);
            }
            let esc = self.peek().ok_or_else(|| self.err("обірваний рядок"))?;
            self.pos += 1;
            match esc {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'b' => out.push('\u{8}'),
                b'f' => out.push('\u{c}'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    let hi = self.hex4()?;
                    let cp = if (0xD800..0xDC00).contains(&hi) {
                        // суррогатна пара
                        if !self.eat("\\u") {
                            return Err(self.err("самотній суррогат"));
                        }
                        let lo = self.hex4()?;
                        if !(0xDC00..0xE000).contains(&lo) {
                            return Err(self.err("некоректний суррогат"));
                        }
                        0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                    } else {
                        hi
                    };
                    out.push(char::from_u32(cp).ok_or_else(|| self.err("некоректний символ \\u"))?);
                }
                _ => return Err(self.err("невідомий escape")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_github_like_document() {
        let v = parse(
            r#"{"tag_name":"v0.9.1","draft":false,"body":"рядок 1\nрядок \"2\"","assets":[{"name":"a.exe","size":12345,
            "browser_download_url":"https://github.com/o/r/releases/download/v0.9.1/a.exe"},{"name":"b","size":0}],"x":null,"n":-1.5e2}"#,
        )
        .unwrap();
        assert_eq!(v.get("tag_name").and_then(Value::as_str), Some("v0.9.1"));
        assert_eq!(v.get("draft").and_then(Value::as_bool), Some(false));
        assert_eq!(v.get("body").and_then(Value::as_str), Some("рядок 1\nрядок \"2\""));
        let assets = v.get("assets").and_then(Value::as_array).unwrap();
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].get("size").and_then(Value::as_u64), Some(12345));
        assert_eq!(v.get("x"), Some(&Value::Null));
        assert_eq!(v.get("n"), Some(&Value::Num(-150.0)));
        assert_eq!(v.get("missing"), None);
    }

    #[test]
    fn handles_unicode_escapes_and_raw_utf8() {
        assert_eq!(parse(r#""\u0416\u043e""#).unwrap(), Value::Str("Жо".into()));
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap(), Value::Str("😀".into()));
        assert_eq!(parse("\"Привіт, світе — ї\"").unwrap(), Value::Str("Привіт, світе — ї".into()));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "", "{", "[1,]", "{\"a\":}", "{\"a\" 1}", "\"abc", "tru", "{\"a\":1} x", "[1 2]", "\"\\x\"", "\"\\ud800\"",
            "\"\\u12\"", "\"a\nb\"", "-",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn depth_is_limited() {
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert!(parse(&deep).is_err());
        let ok = "[".repeat(10) + &"]".repeat(10);
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn numbers_convert_only_when_exact() {
        assert_eq!(parse("42").unwrap().as_u64(), Some(42));
        assert_eq!(parse("4.5").unwrap().as_u64(), None);
        assert_eq!(parse("-3").unwrap().as_u64(), None);
    }
}
