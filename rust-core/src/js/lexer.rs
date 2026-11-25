#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Ident(String),
    Number(f64),
    Str(String),
    Regex(String, String),
    KeywordFunction,
    KeywordAsync,
    KeywordAwait,
    KeywordClass,
    KeywordExtends,
    KeywordLet,
    KeywordIf,
    KeywordElse,
    KeywordWhile,
    KeywordDo,
    KeywordFor,
    KeywordOf,
    KeywordIn,
    KeywordBreak,
    KeywordContinue,
    KeywordSwitch,
    KeywordCase,
    KeywordDefault,
    KeywordTry,
    KeywordCatch,
    KeywordFinally,
    KeywordThrow,
    KeywordReturn,
    KeywordTypeof,
    KeywordDelete,
    KeywordInstanceof,
    KeywordNew,
    KeywordTrue,
    KeywordFalse,
    KeywordNull,
    KeywordUndefined,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Dot,
    Spread,
    OptChain,
    Comma,
    Colon,
    Question,
    Nullish,
    Semicolon,
    Arrow,
    Assign,
    PlusAssign,
    MinusAssign,
    MulAssign,
    DivAssign,
    PlusPlus,
    MinusMinus,
    Plus,
    Minus,
    Star,
    Pow,
    Slash,
    Percent,
    EqEq,
    EqEqEq,
    NotEq,
    NotEqEq,
    Lt,
    Lte,
    Gt,
    Gte,
    AndAnd,
    OrOr,
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,
    UShr,
    Bang,
    Eof,
}

impl Token {
    fn can_end_expr(&self) -> bool {
        matches!(
            self,
            Token::Ident(_)
                | Token::Number(_)
                | Token::Str(_)
                | Token::Regex(_, _)
                | Token::KeywordTrue
                | Token::KeywordFalse
                | Token::KeywordNull
                | Token::KeywordUndefined
                | Token::RParen
                | Token::RBracket
                | Token::PlusPlus
                | Token::MinusMinus
        )
    }
}

pub(crate) struct JsLexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsLexer<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    pub(crate) fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::with_capacity(64);
        self.lex_until_brace_depth(&mut tokens, None);
        tokens.push(Token::Eof);
        tokens
    }

    fn lex_until_brace_depth(
        &mut self,
        tokens: &mut Vec<Token>,
        stop_on_rbrace_depth: Option<usize>,
    ) {
        let mut brace_depth = 0usize;

        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b.is_ascii_whitespace() {
                self.pos += 1;
                continue;
            }
            if b == b'/' && self.pos + 1 < self.bytes.len() {
                if self.bytes[self.pos + 1] == b'/' {
                    self.pos += 2;
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    continue;
                }
                if self.bytes[self.pos + 1] == b'*' {
                    self.pos += 2;
                    while self.pos + 1 < self.bytes.len()
                        && !(self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/')
                    {
                        self.pos += 1;
                    }
                    self.pos = (self.pos + 2).min(self.bytes.len());
                    continue;
                }
            }

            if b == b'`' {
                self.pos += 1;
                self.lex_template_literal(tokens);
                continue;
            }

            if b == b'"' || b == b'\'' {
                let quote = b;
                self.pos += 1;
                let mut s = String::new();
                while self.pos < self.bytes.len() {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    self.pos += ch.len_utf8();
                    if ch.len_utf8() == 1 && ch as u8 == quote {
                        break;
                    }
                    if ch == '\\' && self.pos < self.bytes.len() {
                        let esc = self.src[self.pos..].chars().next().unwrap();
                        self.pos += esc.len_utf8();
                        match esc {
                            'n' => s.push('\n'),
                            'r' => s.push('\r'),
                            't' => s.push('\t'),
                            _ => s.push(esc),
                        }
                    } else {
                        s.push(ch);
                    }
                }
                tokens.push(Token::Str(s));
                continue;
            }

            if b == b'/' {
                let is_div = tokens.last().map(|t| t.can_end_expr()).unwrap_or(false);
                if !is_div {
                    self.pos += 1;
                    let mut pat = String::new();
                    let mut in_class = false;
                    while self.pos < self.bytes.len() {
                        let ch = self.src[self.pos..].chars().next().unwrap();
                        self.pos += ch.len_utf8();
                        if ch == '\\' && self.pos < self.bytes.len() {
                            pat.push(ch);
                            let esc = self.src[self.pos..].chars().next().unwrap();
                            self.pos += esc.len_utf8();
                            pat.push(esc);
                            continue;
                        }
                        if ch == '[' {
                            in_class = true;
                        } else if ch == ']' {
                            in_class = false;
                        } else if ch == '/' && !in_class {
                            break;
                        }
                        pat.push(ch);
                    }
                    let mut flags = String::new();
                    while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_alphabetic()
                    {
                        flags.push(self.bytes[self.pos] as char);
                        self.pos += 1;
                    }
                    tokens.push(Token::Regex(pat, flags));
                    continue;
                }
            }

            if b.is_ascii_digit() {
                let start = self.pos;
                if b == b'0' && self.pos + 1 < self.bytes.len() {
                    let next = self.bytes[self.pos + 1];
                    if next == b'x' || next == b'X' {
                        self.pos += 2;
                        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_hexdigit()
                        {
                            self.pos += 1;
                        }
                        let val = u64::from_str_radix(&self.src[start + 2..self.pos], 16)
                            .unwrap_or(0) as f64;
                        tokens.push(Token::Number(val));
                        continue;
                    }
                    if next == b'b' || next == b'B' {
                        self.pos += 2;
                        while self.pos < self.bytes.len()
                            && matches!(self.bytes[self.pos], b'0' | b'1')
                        {
                            self.pos += 1;
                        }
                        let val = u64::from_str_radix(&self.src[start + 2..self.pos], 2)
                            .unwrap_or(0) as f64;
                        tokens.push(Token::Number(val));
                        continue;
                    }
                }
                while self.pos < self.bytes.len()
                    && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'.')
                {
                    self.pos += 1;
                }
                if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'e' | b'E') {
                    self.pos += 1;
                    if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'+' | b'-') {
                        self.pos += 1;
                    }
                    while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                        self.pos += 1;
                    }
                }
                let val = self.src[start..self.pos].parse::<f64>().unwrap_or(0.0);
                tokens.push(Token::Number(val));
                continue;
            }

            if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && (self.bytes[self.pos].is_ascii_alphanumeric()
                        || self.bytes[self.pos] == b'_'
                        || self.bytes[self.pos] == b'$')
                {
                    self.pos += 1;
                }
                let ident = &self.src[start..self.pos];
                let tok = match ident {
                    "function" => Token::KeywordFunction,
                    "async" => Token::KeywordAsync,
                    "await" => Token::KeywordAwait,
                    "class" => Token::KeywordClass,
                    "extends" => Token::KeywordExtends,
                    "let" | "const" | "var" => Token::KeywordLet,
                    "if" => Token::KeywordIf,
                    "else" => Token::KeywordElse,
                    "while" => Token::KeywordWhile,
                    "do" => Token::KeywordDo,
                    "for" => Token::KeywordFor,
                    "of" => Token::KeywordOf,
                    "in" => Token::KeywordIn,
                    "break" => Token::KeywordBreak,
                    "continue" => Token::KeywordContinue,
                    "switch" => Token::KeywordSwitch,
                    "case" => Token::KeywordCase,
                    "default" => Token::KeywordDefault,
                    "try" => Token::KeywordTry,
                    "catch" => Token::KeywordCatch,
                    "finally" => Token::KeywordFinally,
                    "throw" => Token::KeywordThrow,
                    "return" => Token::KeywordReturn,
                    "typeof" => Token::KeywordTypeof,
                    "delete" => Token::KeywordDelete,
                    "instanceof" => Token::KeywordInstanceof,
                    "new" => Token::KeywordNew,
                    "true" => Token::KeywordTrue,
                    "false" => Token::KeywordFalse,
                    "null" => Token::KeywordNull,
                    "undefined" => Token::KeywordUndefined,
                    _ => Token::Ident(ident.to_string()),
                };
                tokens.push(tok);
                continue;
            }

            if b == b'{' {
                brace_depth += 1;
                self.pos += 1;
                tokens.push(Token::LBrace);
                continue;
            }
            if b == b'}' {
                if let Some(stop_d) = stop_on_rbrace_depth {
                    if brace_depth == stop_d {
                        self.pos += 1;
                        return;
                    }
                }
                brace_depth = brace_depth.saturating_sub(1);
                self.pos += 1;
                tokens.push(Token::RBrace);
                continue;
            }

            self.pos += 1;
            let peek = self.bytes.get(self.pos).copied();
            match b {
                b'(' => tokens.push(Token::LParen),
                b')' => tokens.push(Token::RParen),
                b'[' => tokens.push(Token::LBracket),
                b']' => tokens.push(Token::RBracket),
                b'.' => {
                    if peek == Some(b'.') && self.bytes.get(self.pos + 1) == Some(&b'.') {
                        self.pos += 2;
                        tokens.push(Token::Spread);
                    } else {
                        tokens.push(Token::Dot);
                    }
                }
                b',' => tokens.push(Token::Comma),
                b':' => tokens.push(Token::Colon),
                b'?' => {
                    if peek == Some(b'?') {
                        self.pos += 1;
                        tokens.push(Token::Nullish);
                    } else if peek == Some(b'.') {
                        self.pos += 1;
                        tokens.push(Token::OptChain);
                    } else {
                        tokens.push(Token::Question);
                    }
                }
                b';' => tokens.push(Token::Semicolon),
                b'+' => {
                    if peek == Some(b'+') {
                        self.pos += 1;
                        tokens.push(Token::PlusPlus);
                    } else if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::PlusAssign);
                    } else {
                        tokens.push(Token::Plus);
                    }
                }
                b'-' => {
                    if peek == Some(b'-') {
                        self.pos += 1;
                        tokens.push(Token::MinusMinus);
                    } else if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::MinusAssign);
                    } else {
                        tokens.push(Token::Minus);
                    }
                }
                b'*' => {
                    if peek == Some(b'*') {
                        self.pos += 1;
                        tokens.push(Token::Pow);
                    } else if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::MulAssign);
                    } else {
                        tokens.push(Token::Star);
                    }
                }
                b'/' => {
                    if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::DivAssign);
                    } else {
                        tokens.push(Token::Slash);
                    }
                }
                b'%' => tokens.push(Token::Percent),
                b'~' => tokens.push(Token::BitNot),
                b'^' => tokens.push(Token::BitXor),
                b'=' => {
                    if peek == Some(b'>') {
                        self.pos += 1;
                        tokens.push(Token::Arrow);
                    } else if peek == Some(b'=') {
                        self.pos += 1;
                        if self.bytes.get(self.pos).copied() == Some(b'=') {
                            self.pos += 1;
                            tokens.push(Token::EqEqEq);
                        } else {
                            tokens.push(Token::EqEq);
                        }
                    } else {
                        tokens.push(Token::Assign);
                    }
                }
                b'!' => {
                    if peek == Some(b'=') {
                        self.pos += 1;
                        if self.bytes.get(self.pos).copied() == Some(b'=') {
                            self.pos += 1;
                            tokens.push(Token::NotEqEq);
                        } else {
                            tokens.push(Token::NotEq);
                        }
                    } else {
                        tokens.push(Token::Bang);
                    }
                }
                b'<' => {
                    if peek == Some(b'<') {
                        self.pos += 1;
                        tokens.push(Token::Shl);
                    } else if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::Lte);
                    } else {
                        tokens.push(Token::Lt);
                    }
                }
                b'>' => {
                    if peek == Some(b'>') {
                        self.pos += 1;
                        if self.bytes.get(self.pos).copied() == Some(b'>') {
                            self.pos += 1;
                            tokens.push(Token::UShr);
                        } else {
                            tokens.push(Token::Shr);
                        }
                    } else if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::Gte);
                    } else {
                        tokens.push(Token::Gt);
                    }
                }
                b'&' => {
                    if peek == Some(b'&') {
                        self.pos += 1;
                        tokens.push(Token::AndAnd);
                    } else {
                        tokens.push(Token::BitAnd);
                    }
                }
                b'|' => {
                    if peek == Some(b'|') {
                        self.pos += 1;
                        tokens.push(Token::OrOr);
                    } else {
                        tokens.push(Token::BitOr);
                    }
                }
                _ => {}
            }
        }
    }

    fn lex_template_literal(&mut self, tokens: &mut Vec<Token>) {
        tokens.push(Token::LParen);
        let mut first_part = true;
        let mut s = String::new();

        while self.pos < self.bytes.len() {
            let ch = self.src[self.pos..].chars().next().unwrap();
            if ch == '`' {
                self.pos += 1;
                break;
            }
            if ch == '$' && self.bytes.get(self.pos + 1) == Some(&b'{') {
                if !first_part {
                    tokens.push(Token::Plus);
                }
                first_part = false;
                tokens.push(Token::Str(std::mem::take(&mut s)));
                tokens.push(Token::Plus);
                tokens.push(Token::LParen);
                self.pos += 2;
                self.lex_until_brace_depth(tokens, Some(0));
                tokens.push(Token::RParen);
                continue;
            }
            self.pos += ch.len_utf8();
            if ch == '\\' && self.pos < self.bytes.len() {
                let esc = self.src[self.pos..].chars().next().unwrap();
                self.pos += esc.len_utf8();
                match esc {
                    'n' => s.push('\n'),
                    'r' => s.push('\r'),
                    't' => s.push('\t'),
                    _ => s.push(esc),
                }
            } else {
                s.push(ch);
            }
        }

        if !first_part {
            tokens.push(Token::Plus);
        }
        tokens.push(Token::Str(s));
        tokens.push(Token::RParen);
    }
}
