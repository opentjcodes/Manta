use std::collections::HashMap;

use crate::clipboard::ClipboardSandbox;
use crate::dom_arena::ZeroCopyDomArena;
use crate::downloads_engine::MantaDownloadManager;
use crate::storage_engine::MantaStorageEngine;

#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
}

impl JsValue {
    pub fn as_str(&self) -> String {
        match self {
            JsValue::Undefined => "undefined".to_string(),
            JsValue::Null => "null".to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Number(n) => {
                if n.is_nan() {
                    "NaN".to_string()
                } else if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{:.0}", n)
                } else {
                    n.to_string()
                }
            }
            JsValue::Str(s) => s.clone(),
        }
    }

    pub fn as_number(&self) -> f64 {
        match self {
            JsValue::Undefined => f64::NAN,
            JsValue::Null => 0.0,
            JsValue::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            JsValue::Number(n) => *n,
            JsValue::Str(s) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsValue::Str(s) => !s.is_empty(),
        }
    }
}

/// User-defined JavaScript function compiled into bytecode.
#[derive(Debug, Clone, PartialEq)]
pub struct JsUserFunction {
    pub name: String,
    pub params: Vec<u16>,
    pub entry_pc: usize,
}

/// Compact Stack-VM Bytecode Instruction Set (`OpCode`).
#[derive(Debug, Clone, PartialEq)]
pub enum OpCode {
    LoadConst(u16),
    LoadVar(u16),
    StoreVar(u16),
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Not,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
    Jump(usize),
    JumpIfFalse(usize),
    CallFn { fn_name_id: u16, argc: u8 },
    Return,
    Pop,
    Halt,
}

/// Pre-compiled, constant-deduplicated bytecode program.
#[derive(Debug, Clone, Default)]
pub struct CompiledJsProgram {
    pub constants: Vec<JsValue>,
    pub identifiers: Vec<String>,
    pub functions: HashMap<u16, JsUserFunction>,
    pub instructions: Vec<OpCode>,
}

impl CompiledJsProgram {
    fn add_const(&mut self, val: JsValue) -> u16 {
        if let Some(pos) = self.constants.iter().position(|c| c == &val) {
            return pos as u16;
        }
        let idx = self.constants.len() as u16;
        self.constants.push(val);
        idx
    }

    fn add_ident(&mut self, name: &str) -> u16 {
        if let Some(pos) = self.identifiers.iter().position(|i| i == name) {
            return pos as u16;
        }
        let idx = self.identifiers.len() as u16;
        self.identifiers.push(name.to_string());
        idx
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Number(f64),
    Str(String),
    KeywordFunction,
    KeywordLet,
    KeywordIf,
    KeywordElse,
    KeywordWhile,
    KeywordReturn,
    KeywordTrue,
    KeywordFalse,
    KeywordNull,
    KeywordUndefined,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Assign,
    PlusAssign,
    MinusAssign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
    AndAnd,
    OrOr,
    Bang,
    Eof,
}

struct JsLexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> JsLexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().peekable(),
        }
    }

    fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::with_capacity(64);
        while let Some(&ch) = self.chars.peek() {
            if ch.is_whitespace() {
                self.chars.next();
                continue;
            }
            if ch == '/' {
                self.chars.next();
                if self.chars.peek() == Some(&'/') {
                    while let Some(c) = self.chars.next() {
                        if c == '\n' {
                            break;
                        }
                    }
                    continue;
                } else if self.chars.peek() == Some(&'*') {
                    self.chars.next();
                    while let Some(c) = self.chars.next() {
                        if c == '*' && self.chars.peek() == Some(&'/') {
                            self.chars.next();
                            break;
                        }
                    }
                    continue;
                } else {
                    tokens.push(Token::Slash);
                    continue;
                }
            }
            if ch == '"' || ch == '\'' || ch == '`' {
                let quote = self.chars.next().unwrap();
                let mut s = String::new();
                while let Some(c) = self.chars.next() {
                    if c == '\\' {
                        if let Some(esc) = self.chars.next() {
                            match esc {
                                'n' => s.push('\n'),
                                'r' => s.push('\r'),
                                't' => s.push('\t'),
                                _ => s.push(esc),
                            }
                        }
                    } else if c == quote {
                        break;
                    } else {
                        s.push(c);
                    }
                }
                tokens.push(Token::Str(s));
                continue;
            }
            if ch.is_ascii_digit() {
                let mut num_str = String::new();
                while let Some(&c) = self.chars.peek() {
                    if c.is_ascii_digit() || c == '.' {
                        num_str.push(c);
                        self.chars.next();
                    } else {
                        break;
                    }
                }
                let val = num_str.parse::<f64>().unwrap_or(0.0);
                tokens.push(Token::Number(val));
                continue;
            }
            if ch.is_ascii_alphabetic() || ch == '_' || ch == '$' {
                let mut ident = String::new();
                while let Some(&c) = self.chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.' {
                        ident.push(c);
                        self.chars.next();
                    } else {
                        break;
                    }
                }
                let tok = match ident.as_str() {
                    "function" => Token::KeywordFunction,
                    "let" | "const" | "var" => Token::KeywordLet,
                    "if" => Token::KeywordIf,
                    "else" => Token::KeywordElse,
                    "while" => Token::KeywordWhile,
                    "return" => Token::KeywordReturn,
                    "true" => Token::KeywordTrue,
                    "false" => Token::KeywordFalse,
                    "null" => Token::KeywordNull,
                    "undefined" => Token::KeywordUndefined,
                    _ => Token::Ident(ident),
                };
                tokens.push(tok);
                continue;
            }

            self.chars.next();
            match ch {
                '(' => tokens.push(Token::LParen),
                ')' => tokens.push(Token::RParen),
                '{' => tokens.push(Token::LBrace),
                '}' => tokens.push(Token::RBrace),
                ',' => tokens.push(Token::Comma),
                ';' => tokens.push(Token::Semicolon),
                '+' => {
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        tokens.push(Token::PlusAssign);
                    } else {
                        tokens.push(Token::Plus);
                    }
                }
                '-' => {
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        tokens.push(Token::MinusAssign);
                    } else {
                        tokens.push(Token::Minus);
                    }
                }
                '*' => tokens.push(Token::Star),
                '%' => tokens.push(Token::Percent),
                '=' => {
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        if self.chars.peek() == Some(&'=') {
                            self.chars.next();
                        }
                        tokens.push(Token::EqEq);
                    } else {
                        tokens.push(Token::Assign);
                    }
                }
                '!' => {
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        if self.chars.peek() == Some(&'=') {
                            self.chars.next();
                        }
                        tokens.push(Token::NotEq);
                    } else {
                        tokens.push(Token::Bang);
                    }
                }
                '<' => {
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        tokens.push(Token::Lte);
                    } else {
                        tokens.push(Token::Lt);
                    }
                }
                '>' => {
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        tokens.push(Token::Gte);
                    } else {
                        tokens.push(Token::Gt);
                    }
                }
                '&' => {
                    if self.chars.peek() == Some(&'&') {
                        self.chars.next();
                        tokens.push(Token::AndAnd);
                    }
                }
                '|' => {
                    if self.chars.peek() == Some(&'|') {
                        self.chars.next();
                        tokens.push(Token::OrOr);
                    }
                }
                _ => {}
            }
        }
        tokens.push(Token::Eof);
        tokens
    }
}

/// Recursive-descent Bytecode Compiler (`JsBytecodeCompiler`) supporting user functions,
/// control flow, DOM manipulation, Form/Input value bindings, Cookies, MantaDB, and Virtual Files.
pub struct JsBytecodeCompiler {
    tokens: Vec<Token>,
    pos: usize,
    program: CompiledJsProgram,
}

impl JsBytecodeCompiler {
    pub fn compile(source: &str) -> Result<CompiledJsProgram, &'static str> {
        let mut lexer = JsLexer::new(source);
        let tokens = lexer.tokenize();
        let mut compiler = Self {
            tokens,
            pos: 0,
            program: CompiledJsProgram::default(),
        };

        while compiler.peek() != &Token::Eof {
            compiler.compile_statement()?;
        }
        compiler.program.instructions.push(OpCode::Halt);
        Ok(compiler.program)
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn consume_optional_semicolons(&mut self) {
        while self.peek() == &Token::Semicolon {
            self.advance();
        }
    }

    fn compile_statement(&mut self) -> Result<(), &'static str> {
        self.consume_optional_semicolons();
        match self.peek().clone() {
            Token::Eof | Token::RBrace => Ok(()),
            Token::KeywordFunction => {
                self.advance();
                let Token::Ident(fn_name) = self.advance() else {
                    return Err("Expected function name");
                };
                let fn_id = self.program.add_ident(&fn_name);
                if self.peek() == &Token::LParen {
                    self.advance();
                }
                let mut params = Vec::new();
                while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                    if let Token::Ident(param_name) = self.advance() {
                        params.push(self.program.add_ident(&param_name));
                    }
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                }
                if self.peek() == &Token::RParen {
                    self.advance();
                }

                let skip_jump_idx = self.program.instructions.len();
                self.program.instructions.push(OpCode::Jump(0));

                let entry_pc = self.program.instructions.len();
                self.program.functions.insert(
                    fn_id,
                    JsUserFunction {
                        name: fn_name,
                        params,
                        entry_pc,
                    },
                );

                self.compile_statement()?;
                let undef_c = self.program.add_const(JsValue::Undefined);
                self.program.instructions.push(OpCode::LoadConst(undef_c));
                self.program.instructions.push(OpCode::Return);

                let after_fn = self.program.instructions.len();
                self.program.instructions[skip_jump_idx] = OpCode::Jump(after_fn);
                Ok(())
            }
            Token::LBrace => {
                self.advance();
                while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
                    self.compile_statement()?;
                }
                if self.peek() == &Token::RBrace {
                    self.advance();
                }
                Ok(())
            }
            Token::KeywordLet => {
                self.advance();
                if let Token::Ident(var_name) = self.advance() {
                    let var_id = self.program.add_ident(&var_name);
                    if self.peek() == &Token::Assign {
                        self.advance();
                        self.compile_expr()?;
                    } else {
                        let undef_id = self.program.add_const(JsValue::Undefined);
                        self.program.instructions.push(OpCode::LoadConst(undef_id));
                    }
                    self.program.instructions.push(OpCode::StoreVar(var_id));
                    self.program.instructions.push(OpCode::Pop);
                    self.consume_optional_semicolons();
                    Ok(())
                } else {
                    Err("Expected variable identifier after let/const/var")
                }
            }
            Token::KeywordIf => {
                self.advance();
                if self.peek() == &Token::LParen {
                    self.advance();
                }
                self.compile_expr()?;
                if self.peek() == &Token::RParen {
                    self.advance();
                }
                let jump_false_idx = self.program.instructions.len();
                self.program.instructions.push(OpCode::JumpIfFalse(0));

                self.compile_statement()?;

                if self.peek() == &Token::KeywordElse {
                    self.advance();
                    let jump_end_idx = self.program.instructions.len();
                    self.program.instructions.push(OpCode::Jump(0));
                    let else_start = self.program.instructions.len();
                    self.program.instructions[jump_false_idx] = OpCode::JumpIfFalse(else_start);
                    self.compile_statement()?;
                    let after_else = self.program.instructions.len();
                    self.program.instructions[jump_end_idx] = OpCode::Jump(after_else);
                } else {
                    let after_then = self.program.instructions.len();
                    self.program.instructions[jump_false_idx] = OpCode::JumpIfFalse(after_then);
                }
                Ok(())
            }
            Token::KeywordWhile => {
                self.advance();
                let loop_start = self.program.instructions.len();
                if self.peek() == &Token::LParen {
                    self.advance();
                }
                self.compile_expr()?;
                if self.peek() == &Token::RParen {
                    self.advance();
                }
                let exit_jump_idx = self.program.instructions.len();
                self.program.instructions.push(OpCode::JumpIfFalse(0));

                self.compile_statement()?;
                self.program.instructions.push(OpCode::Jump(loop_start));
                let loop_end = self.program.instructions.len();
                self.program.instructions[exit_jump_idx] = OpCode::JumpIfFalse(loop_end);
                Ok(())
            }
            Token::KeywordReturn => {
                self.advance();
                if self.peek() != &Token::Semicolon && self.peek() != &Token::RBrace {
                    self.compile_expr()?;
                } else {
                    let c = self.program.add_const(JsValue::Undefined);
                    self.program.instructions.push(OpCode::LoadConst(c));
                }
                self.program.instructions.push(OpCode::Return);
                self.consume_optional_semicolons();
                Ok(())
            }
            _ => {
                self.compile_expr()?;
                self.program.instructions.push(OpCode::Pop);
                self.consume_optional_semicolons();
                Ok(())
            }
        }
    }

    fn compile_expr(&mut self) -> Result<(), &'static str> {
        self.compile_assignment()
    }

    fn compile_assignment(&mut self) -> Result<(), &'static str> {
        if let Token::Ident(ref name) = self.peek().clone() {
            if name == "document.getElementById" || name == "document.querySelector" {
                let saved_pos = self.pos;
                let fn_name = name.clone();
                self.advance();
                if self.peek() == &Token::LParen {
                    self.advance();
                    self.compile_expr()?;
                    if self.peek() == &Token::RParen {
                        self.advance();
                    }
                    if let Token::Ident(prop) = self.peek().clone() {
                        let clean_prop = prop.trim_start_matches('.');
                        let is_value_prop = clean_prop == "value";
                        let is_text_prop = clean_prop == "textContent"
                            || clean_prop == "innerText"
                            || clean_prop == "innerHTML";

                        if is_value_prop || is_text_prop {
                            self.advance();
                            if self.peek() == &Token::Assign {
                                self.advance();
                                self.compile_expr()?;
                                let host_name = match (fn_name.as_str(), is_value_prop) {
                                    ("document.getElementById", true) => "__dom_set_value_by_id",
                                    ("document.getElementById", false) => "__dom_set_text_by_id",
                                    (_, true) => "__dom_set_value_by_sel",
                                    (_, false) => "__dom_set_text_by_sel",
                                };
                                let fn_id = self.program.add_ident(host_name);
                                self.program.instructions.push(OpCode::CallFn {
                                    fn_name_id: fn_id,
                                    argc: 2,
                                });
                                return Ok(());
                            } else {
                                let host_name = match (fn_name.as_str(), is_value_prop) {
                                    ("document.getElementById", true) => "__dom_get_value_by_id",
                                    ("document.getElementById", false) => "__dom_get_text_by_id",
                                    (_, true) => "__dom_get_value_by_sel",
                                    (_, false) => "__dom_get_text_by_sel",
                                };
                                let fn_id = self.program.add_ident(host_name);
                                self.program.instructions.push(OpCode::CallFn {
                                    fn_name_id: fn_id,
                                    argc: 1,
                                });
                                return Ok(());
                            }
                        }
                    }
                }
                self.pos = saved_pos;
            }
        }

        if let Token::Ident(name) = self.peek().clone() {
            let next_tok = self.tokens.get(self.pos + 1).cloned();
            if next_tok == Some(Token::Assign)
                || next_tok == Some(Token::PlusAssign)
                || next_tok == Some(Token::MinusAssign)
            {
                self.advance();
                let op = self.advance();
                let var_id = self.program.add_ident(&name);
                if op == Token::PlusAssign {
                    self.program.instructions.push(OpCode::LoadVar(var_id));
                    self.compile_expr()?;
                    self.program.instructions.push(OpCode::Add);
                } else if op == Token::MinusAssign {
                    self.program.instructions.push(OpCode::LoadVar(var_id));
                    self.compile_expr()?;
                    self.program.instructions.push(OpCode::Sub);
                } else {
                    self.compile_expr()?;
                }
                self.program.instructions.push(OpCode::StoreVar(var_id));
                return Ok(());
            }
        }

        self.compile_logical_or()
    }

    fn compile_logical_or(&mut self) -> Result<(), &'static str> {
        self.compile_logical_and()?;
        while self.peek() == &Token::OrOr {
            self.advance();
            self.compile_logical_and()?;
            self.program.instructions.push(OpCode::Or);
        }
        Ok(())
    }

    fn compile_logical_and(&mut self) -> Result<(), &'static str> {
        self.compile_equality()?;
        while self.peek() == &Token::AndAnd {
            self.advance();
            self.compile_equality()?;
            self.program.instructions.push(OpCode::And);
        }
        Ok(())
    }

    fn compile_equality(&mut self) -> Result<(), &'static str> {
        self.compile_comparison()?;
        while matches!(self.peek(), Token::EqEq | Token::NotEq) {
            let op = self.advance();
            self.compile_comparison()?;
            match op {
                Token::EqEq => self.program.instructions.push(OpCode::Eq),
                Token::NotEq => self.program.instructions.push(OpCode::Neq),
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_comparison(&mut self) -> Result<(), &'static str> {
        self.compile_additive()?;
        while matches!(self.peek(), Token::Lt | Token::Lte | Token::Gt | Token::Gte) {
            let op = self.advance();
            self.compile_additive()?;
            match op {
                Token::Lt => self.program.instructions.push(OpCode::Lt),
                Token::Lte => self.program.instructions.push(OpCode::Lte),
                Token::Gt => self.program.instructions.push(OpCode::Gt),
                Token::Gte => self.program.instructions.push(OpCode::Gte),
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_additive(&mut self) -> Result<(), &'static str> {
        self.compile_multiplicative()?;
        while matches!(self.peek(), Token::Plus | Token::Minus) {
            let op = self.advance();
            self.compile_multiplicative()?;
            match op {
                Token::Plus => self.program.instructions.push(OpCode::Add),
                Token::Minus => self.program.instructions.push(OpCode::Sub),
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_multiplicative(&mut self) -> Result<(), &'static str> {
        self.compile_unary()?;
        while matches!(self.peek(), Token::Star | Token::Slash | Token::Percent) {
            let op = self.advance();
            self.compile_unary()?;
            match op {
                Token::Star => self.program.instructions.push(OpCode::Mul),
                Token::Slash => self.program.instructions.push(OpCode::Div),
                Token::Percent => self.program.instructions.push(OpCode::Mod),
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_unary(&mut self) -> Result<(), &'static str> {
        if self.peek() == &Token::Minus {
            self.advance();
            self.compile_unary()?;
            self.program.instructions.push(OpCode::Neg);
            return Ok(());
        }
        if self.peek() == &Token::Bang {
            self.advance();
            self.compile_unary()?;
            self.program.instructions.push(OpCode::Not);
            return Ok(());
        }
        self.compile_primary()
    }

    fn compile_primary(&mut self) -> Result<(), &'static str> {
        match self.advance() {
            Token::Number(n) => {
                let idx = self.program.add_const(JsValue::Number(n));
                self.program.instructions.push(OpCode::LoadConst(idx));
                Ok(())
            }
            Token::Str(s) => {
                let idx = self.program.add_const(JsValue::Str(s));
                self.program.instructions.push(OpCode::LoadConst(idx));
                Ok(())
            }
            Token::KeywordTrue => {
                let idx = self.program.add_const(JsValue::Bool(true));
                self.program.instructions.push(OpCode::LoadConst(idx));
                Ok(())
            }
            Token::KeywordFalse => {
                let idx = self.program.add_const(JsValue::Bool(false));
                self.program.instructions.push(OpCode::LoadConst(idx));
                Ok(())
            }
            Token::KeywordNull => {
                let idx = self.program.add_const(JsValue::Null);
                self.program.instructions.push(OpCode::LoadConst(idx));
                Ok(())
            }
            Token::KeywordUndefined => {
                let idx = self.program.add_const(JsValue::Undefined);
                self.program.instructions.push(OpCode::LoadConst(idx));
                Ok(())
            }
            Token::LParen => {
                self.compile_expr()?;
                if self.peek() == &Token::RParen {
                    self.advance();
                }
                Ok(())
            }
            Token::Ident(name) => {
                if self.peek() == &Token::LParen {
                    self.advance();
                    let mut argc = 0u8;
                    while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                        self.compile_expr()?;
                        argc = argc.saturating_add(1);
                        if self.peek() == &Token::Comma {
                            self.advance();
                        }
                    }
                    if self.peek() == &Token::RParen {
                        self.advance();
                    }
                    let fn_id = self.program.add_ident(&name);
                    self.program.instructions.push(OpCode::CallFn {
                        fn_name_id: fn_id,
                        argc,
                    });
                    Ok(())
                } else {
                    let var_id = self.program.add_ident(&name);
                    self.program.instructions.push(OpCode::LoadVar(var_id));
                    Ok(())
                }
            }
            _ => {
                let idx = self.program.add_const(JsValue::Undefined);
                self.program.instructions.push(OpCode::LoadConst(idx));
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CallFrame {
    return_pc: usize,
    locals: HashMap<u16, JsValue>,
}

/// High-Efficiency Bytecode JS Sandbox (`MantaJsSandbox`).
/// - Compiles JS into compact stack bytecode (`CompiledJsProgram`) with user functions & call frames.
/// - Fuel-metered execution (prevents infinite loops and CPU/battery drain).
/// - Direct bindings to `ZeroCopyDomArena`, `MantaStorageEngine` (`localStorage`, `sessionStorage`,
///   `document.cookie`, `MantaDB`, `MantaFS`, `MantaForm` drafts), `MantaDownloadManager`, and `ClipboardSandbox`.
pub struct MantaJsSandbox {
    globals: HashMap<String, JsValue>,
    fuel_remaining: u32,
    max_fuel: u32,
    pub console_logs: Vec<String>,
}

impl MantaJsSandbox {
    pub fn new(max_fuel: u32) -> Self {
        let mut globals = HashMap::with_capacity(16);
        globals.insert("navigator.webdriver".to_string(), JsValue::Bool(false));
        globals.insert("navigator.cookieEnabled".to_string(), JsValue::Bool(true));
        globals.insert(
            "navigator.userAgent".to_string(),
            JsValue::Str("Manta/2.5 (Native Rust Core; W3C Standard)".to_string()),
        );
        globals.insert("window.isSecureContext".to_string(), JsValue::Bool(true));

        Self {
            globals,
            fuel_remaining: max_fuel,
            max_fuel,
            console_logs: Vec::new(),
        }
    }

    pub fn execute_script(
        &mut self,
        script: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
    ) -> Result<JsValue, &'static str> {
        let mut dummy_storage = MantaStorageEngine::new(64 * 1024, 256 * 1024);
        let mut dummy_downloads = MantaDownloadManager::new("", 1024 * 1024);
        self.execute_script_full(
            script,
            "https://manta.local",
            dom,
            clipboard,
            &mut dummy_storage,
            &mut dummy_downloads,
        )
    }

    pub fn execute_script_with_storage(
        &mut self,
        script: &str,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
    ) -> Result<JsValue, &'static str> {
        let mut dummy_downloads = MantaDownloadManager::new("", 10 * 1024 * 1024);
        self.execute_script_full(
            script,
            origin_url,
            dom,
            clipboard,
            storage,
            &mut dummy_downloads,
        )
    }

    pub fn execute_script_full(
        &mut self,
        script: &str,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        let program = JsBytecodeCompiler::compile(script)?;
        self.run_bytecode(&program, origin_url, dom, clipboard, storage, downloads)
    }

    pub fn run_bytecode(
        &mut self,
        program: &CompiledJsProgram,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        self.fuel_remaining = self.max_fuel;
        let mut stack: Vec<JsValue> = Vec::with_capacity(64);
        let mut call_frames: Vec<CallFrame> = Vec::with_capacity(16);
        let mut last_popped = JsValue::Undefined;
        let mut pc = 0usize;

        while pc < program.instructions.len() {
            if self.fuel_remaining == 0 {
                return Err("JS execution terminated: instruction fuel limit exceeded");
            }
            self.fuel_remaining -= 1;

            match &program.instructions[pc] {
                OpCode::LoadConst(idx) => {
                    let val = program
                        .constants
                        .get(*idx as usize)
                        .cloned()
                        .unwrap_or(JsValue::Undefined);
                    stack.push(val);
                    pc += 1;
                }
                OpCode::LoadVar(idx) => {
                    if let Some(frame) = call_frames.last() {
                        if let Some(local_val) = frame.locals.get(idx) {
                            stack.push(local_val.clone());
                            pc += 1;
                            continue;
                        }
                    }
                    let name = program
                        .identifiers
                        .get(*idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let val = match name {
                        "document.title" => JsValue::Str(dom.page_title.clone()),
                        "document.cookie" => {
                            JsValue::Str(storage.get_cookie_header_for_url(origin_url, false))
                        }
                        "window.location.href" | "location.href" => {
                            JsValue::Str(origin_url.to_string())
                        }
                        _ => self
                            .globals
                            .get(name)
                            .cloned()
                            .unwrap_or(JsValue::Undefined),
                    };
                    stack.push(val);
                    pc += 1;
                }
                OpCode::StoreVar(idx) => {
                    let val = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(frame) = call_frames.last_mut() {
                        if frame.locals.contains_key(idx) {
                            frame.locals.insert(*idx, val);
                            pc += 1;
                            continue;
                        }
                    }
                    if let Some(name) = program.identifiers.get(*idx as usize) {
                        if name == "document.title" {
                            dom.page_title = val.as_str();
                        } else if name == "document.cookie" {
                            storage.set_cookie_from_header(origin_url, &val.as_str());
                        } else {
                            self.globals.insert(name.clone(), val);
                        }
                    }
                    pc += 1;
                }
                OpCode::Add => {
                    let right = stack.pop().unwrap_or(JsValue::Undefined);
                    let left = stack.pop().unwrap_or(JsValue::Undefined);
                    let res = match (&left, &right) {
                        (JsValue::Number(a), JsValue::Number(b)) => JsValue::Number(a + b),
                        _ => JsValue::Str(format!("{}{}", left.as_str(), right.as_str())),
                    };
                    stack.push(res);
                    pc += 1;
                }
                OpCode::Sub => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a - b));
                    pc += 1;
                }
                OpCode::Mul => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a * b));
                    pc += 1;
                }
                OpCode::Div => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a / b));
                    pc += 1;
                }
                OpCode::Mod => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(a % b));
                    pc += 1;
                }
                OpCode::Neg => {
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Number(-a));
                    pc += 1;
                }
                OpCode::Not => {
                    let a = stack.pop().unwrap_or(JsValue::Undefined).is_truthy();
                    stack.push(JsValue::Bool(!a));
                    pc += 1;
                }
                OpCode::Eq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(a == b));
                    pc += 1;
                }
                OpCode::Neq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(a != b));
                    pc += 1;
                }
                OpCode::Lt => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a < b));
                    pc += 1;
                }
                OpCode::Lte => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a <= b));
                    pc += 1;
                }
                OpCode::Gt => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a > b));
                    pc += 1;
                }
                OpCode::Gte => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    let a = stack.pop().unwrap_or(JsValue::Undefined).as_number();
                    stack.push(JsValue::Bool(a >= b));
                    pc += 1;
                }
                OpCode::And => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(if a.is_truthy() { b } else { a });
                    pc += 1;
                }
                OpCode::Or => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(if a.is_truthy() { a } else { b });
                    pc += 1;
                }
                OpCode::Jump(target) => {
                    pc = *target;
                }
                OpCode::JumpIfFalse(target) => {
                    let cond = stack.pop().unwrap_or(JsValue::Undefined);
                    if !cond.is_truthy() {
                        pc = *target;
                    } else {
                        pc += 1;
                    }
                }
                OpCode::CallFn { fn_name_id, argc } => {
                    let count = *argc as usize;
                    let mut args = Vec::with_capacity(count);
                    for _ in 0..count {
                        args.push(stack.pop().unwrap_or(JsValue::Undefined));
                    }
                    args.reverse();

                    if let Some(user_fn) = program.functions.get(fn_name_id) {
                        if call_frames.len() >= 256 {
                            return Err("JS call stack overflow");
                        }
                        let mut locals = HashMap::with_capacity(user_fn.params.len());
                        for (i, &param_id) in user_fn.params.iter().enumerate() {
                            let val = args.get(i).cloned().unwrap_or(JsValue::Undefined);
                            locals.insert(param_id, val);
                        }
                        call_frames.push(CallFrame {
                            return_pc: pc + 1,
                            locals,
                        });
                        pc = user_fn.entry_pc;
                        continue;
                    }

                    let fn_name = program
                        .identifiers
                        .get(*fn_name_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let ret = self.invoke_host_fn(
                        fn_name,
                        &args,
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    );
                    stack.push(ret);
                    pc += 1;
                }
                OpCode::Return => {
                    let ret_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(frame) = call_frames.pop() {
                        stack.push(ret_val);
                        pc = frame.return_pc;
                    } else {
                        stack.push(ret_val);
                        break;
                    }
                }
                OpCode::Pop => {
                    if let Some(v) = stack.pop() {
                        last_popped = v;
                    }
                    pc += 1;
                }
                OpCode::Halt => {
                    break;
                }
            }
        }

        Ok(stack.pop().unwrap_or(last_popped))
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_host_fn(
        &mut self,
        fn_name: &str,
        args: &[JsValue],
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> JsValue {
        match fn_name {
            "__dom_set_text_by_id" => {
                if args.len() >= 2 {
                    if let Some(nid) = dom.find_by_id(&args[0].as_str()) {
                        dom.set_text_content(nid, args[1].as_str());
                    }
                    return args[1].clone();
                }
                JsValue::Undefined
            }
            "__dom_set_value_by_id" => {
                if args.len() >= 2 {
                    let field_id = args[0].as_str();
                    let val_str = args[1].as_str();
                    if let Some(nid) = dom.find_by_id(&field_id) {
                        let _ = dom.forms.set_value(nid, &val_str);
                        storage.save_form_draft(origin_url, &field_id, &val_str);
                    }
                    return args[1].clone();
                }
                JsValue::Undefined
            }
            "__dom_set_text_by_sel" => {
                if args.len() >= 2 {
                    if let Some(nid) = dom.query_selector(&args[0].as_str()) {
                        dom.set_text_content(nid, args[1].as_str());
                    }
                    return args[1].clone();
                }
                JsValue::Undefined
            }
            "__dom_set_value_by_sel" => {
                if args.len() >= 2 {
                    let sel = args[0].as_str();
                    let val_str = args[1].as_str();
                    if let Some(nid) = dom.query_selector(&sel) {
                        let _ = dom.forms.set_value(nid, &val_str);
                        storage.save_form_draft(origin_url, &sel, &val_str);
                    }
                    return args[1].clone();
                }
                JsValue::Undefined
            }
            "__dom_get_text_by_id" => {
                if let Some(id_arg) = args.first() {
                    if let Some(nid) = dom.find_by_id(&id_arg.as_str()) {
                        return JsValue::Str(dom.text_content(nid));
                    }
                }
                JsValue::Null
            }
            "__dom_get_value_by_id" => {
                if let Some(id_arg) = args.first() {
                    if let Some(nid) = dom.find_by_id(&id_arg.as_str()) {
                        if let Some(ctrl) = dom.forms.get_control(nid) {
                            return JsValue::Str(ctrl.value.clone());
                        }
                    }
                }
                JsValue::Null
            }
            "__dom_get_text_by_sel" => {
                if let Some(sel_arg) = args.first() {
                    if let Some(nid) = dom.query_selector(&sel_arg.as_str()) {
                        return JsValue::Str(dom.text_content(nid));
                    }
                }
                JsValue::Null
            }
            "__dom_get_value_by_sel" => {
                if let Some(sel_arg) = args.first() {
                    if let Some(nid) = dom.query_selector(&sel_arg.as_str()) {
                        if let Some(ctrl) = dom.forms.get_control(nid) {
                            return JsValue::Str(ctrl.value.clone());
                        }
                    }
                }
                JsValue::Null
            }
            "localStorage.setItem" => {
                if args.len() >= 2 {
                    storage.set_local_storage(origin_url, &args[0].as_str(), &args[1].as_str());
                }
                JsValue::Undefined
            }
            "localStorage.getItem" => {
                if let Some(k) = args.first() {
                    return match storage.get_local_storage(origin_url, &k.as_str()) {
                        Some(v) => JsValue::Str(v),
                        None => JsValue::Null,
                    };
                }
                JsValue::Null
            }
            "localStorage.removeItem" => {
                if let Some(k) = args.first() {
                    storage.remove_local_storage(origin_url, &k.as_str());
                }
                JsValue::Undefined
            }
            "localStorage.clear" => {
                storage.clear_local_storage_only(origin_url);
                JsValue::Undefined
            }
            "sessionStorage.setItem" => {
                if args.len() >= 2 {
                    storage.set_session_storage(origin_url, &args[0].as_str(), &args[1].as_str());
                }
                JsValue::Undefined
            }
            "sessionStorage.getItem" => {
                if let Some(k) = args.first() {
                    return match storage.get_session_storage(origin_url, &k.as_str()) {
                        Some(v) => JsValue::Str(v),
                        None => JsValue::Null,
                    };
                }
                JsValue::Null
            }
            "sessionStorage.removeItem" => {
                if let Some(k) = args.first() {
                    storage.remove_session_storage(origin_url, &k.as_str());
                }
                JsValue::Undefined
            }
            "sessionStorage.clear" => {
                storage.clear_session_storage_only(origin_url);
                JsValue::Undefined
            }
            "MantaForm.saveDraft" => {
                if args.len() >= 2 {
                    let ok = storage.save_form_draft(origin_url, &args[0].as_str(), &args[1].as_str());
                    return JsValue::Bool(ok);
                }
                JsValue::Bool(false)
            }
            "MantaForm.getDraft" => {
                if let Some(k) = args.first() {
                    return match storage.get_form_draft(origin_url, &k.as_str()) {
                        Some(v) => JsValue::Str(v),
                        None => JsValue::Null,
                    };
                }
                JsValue::Null
            }
            "MantaDB.putObject" | "indexedDB.put" => {
                if args.len() >= 3 {
                    let ok = storage.db.put_object(
                        origin_url,
                        &args[0].as_str(),
                        &args[1].as_str(),
                        &args[2].as_str(),
                    );
                    if ok && storage.auto_commit_on_write {
                        let _ = storage.save_to_backing_file();
                    }
                    return JsValue::Bool(ok);
                }
                JsValue::Bool(false)
            }
            "MantaDB.getObject" | "indexedDB.get" => {
                if args.len() >= 2 {
                    return match storage.db.get_object(
                        origin_url,
                        &args[0].as_str(),
                        &args[1].as_str(),
                    ) {
                        Some(v) => JsValue::Str(v),
                        None => JsValue::Null,
                    };
                }
                JsValue::Null
            }
            "MantaDB.deleteObject" | "indexedDB.delete" => {
                if args.len() >= 2 {
                    let ok = storage.db.delete_object(
                        origin_url,
                        &args[0].as_str(),
                        &args[1].as_str(),
                    );
                    if ok && storage.auto_commit_on_write {
                        let _ = storage.save_to_backing_file();
                    }
                    return JsValue::Bool(ok);
                }
                JsValue::Bool(false)
            }
            "MantaFS.writeFile" => {
                if args.len() >= 2 {
                    let vpath = args[0].as_str();
                    let content = args[1].as_str();
                    let mime = args
                        .get(2)
                        .map(|a| a.as_str())
                        .unwrap_or_else(|| "text/plain".to_string());
                    let ok =
                        storage
                            .db
                            .save_virtual_file(origin_url, &vpath, &mime, content.as_bytes());
                    if ok && storage.auto_commit_on_write {
                        let _ = storage.save_to_backing_file();
                    }
                    return JsValue::Bool(ok);
                }
                JsValue::Bool(false)
            }
            "MantaFS.readFile" => {
                if let Some(p) = args.first() {
                    if let Some(entry) = storage.db.read_virtual_file(origin_url, &p.as_str()) {
                        return JsValue::Str(String::from_utf8_lossy(&entry.data).into_owned());
                    }
                }
                JsValue::Null
            }
            "MantaFS.deleteFile" => {
                if let Some(p) = args.first() {
                    let ok = storage.db.delete_virtual_file(origin_url, &p.as_str());
                    if ok && storage.auto_commit_on_write {
                        let _ = storage.save_to_backing_file();
                    }
                    return JsValue::Bool(ok);
                }
                JsValue::Bool(false)
            }
            "MantaDownloads.save" => {
                if args.len() >= 2 {
                    let filename = args[0].as_str();
                    let content = args[1].as_str();
                    let mime = args
                        .get(2)
                        .map(|a| a.as_str())
                        .unwrap_or_else(|| "text/plain".to_string());
                    if let Ok(dl_id) = downloads.save_direct_bytes(
                        origin_url,
                        &filename,
                        &mime,
                        content.as_bytes(),
                    ) {
                        return JsValue::Number(dl_id as f64);
                    }
                }
                JsValue::Number(-1.0)
            }
            "navigator.clipboard.writeText" => {
                if let Some(v) = args.first() {
                    clipboard.copy_text(&v.as_str());
                }
                JsValue::Undefined
            }
            "navigator.clipboard.readText" => JsValue::Str(clipboard.paste_text().to_string()),
            "console.log" | "console.info" | "console.warn" | "console.error" => {
                if self.console_logs.len() < 64 {
                    let joined: Vec<String> = args.iter().map(|a| a.as_str()).collect();
                    self.console_logs.push(joined.join(" "));
                }
                JsValue::Undefined
            }
            "parseInt" => {
                let n = args
                    .first()
                    .and_then(|a| a.as_str().trim().parse::<i64>().ok())
                    .map(|v| v as f64)
                    .unwrap_or(f64::NAN);
                JsValue::Number(n)
            }
            "parseFloat" => {
                let n = args.first().map(|a| a.as_number()).unwrap_or(f64::NAN);
                JsValue::Number(n)
            }
            "Math.max" => {
                let max_val = args
                    .iter()
                    .map(|a| a.as_number())
                    .fold(f64::NEG_INFINITY, f64::max);
                JsValue::Number(max_val)
            }
            "Math.min" => {
                let min_val = args
                    .iter()
                    .map(|a| a.as_number())
                    .fold(f64::INFINITY, f64::min);
                JsValue::Number(min_val)
            }
            "Math.floor" => {
                let n = args.first().map(|a| a.as_number()).unwrap_or(0.0);
                JsValue::Number(n.floor())
            }
            "Math.abs" => {
                let n = args.first().map(|a| a.as_number()).unwrap_or(0.0);
                JsValue::Number(n.abs())
            }
            _ => JsValue::Undefined,
        }
    }

    pub fn get_global(&self, name: &str) -> Option<&JsValue> {
        self.globals.get(name)
    }

    pub fn reset(&mut self) {
        self.globals.clear();
        self.console_logs.clear();
        self.fuel_remaining = self.max_fuel;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_bytecode_vm_user_functions_inputs_cookies_mantadb_and_vfs() {
        let mut dom = ZeroCopyDomArena::new(64 * 1024);
        let mut tokenizer = StreamingHtmlTokenizer::new();
        tokenizer.feed_chunk(
            b"<div id=\"status\">Idle</div><input id=\"email\" type=\"email\" value=\"\" />",
            true,
            |tok| dom.ingest_token(tok),
        );

        let mut vm = MantaJsSandbox::new(10_000);
        let mut clip = ClipboardSandbox::new(1024);
        let mut storage = MantaStorageEngine::new(128 * 1024, 512 * 1024);
        let mut downloads = MantaDownloadManager::new("", 1024 * 1024);

        let script = r#"
            function multiplyAndAdd(a, b, c) {
                return a * b + c;
            }
            let result = multiplyAndAdd(6, 7, 8);
            document.getElementById("status").textContent = "Computed: " + result;
            document.getElementById("email").value = "dev@manta.browser";
            document.cookie = "session_id=manta_999; Path=/";
            MantaDB.putObject("users", "u1", "{\"email\":\"dev@manta.browser\"}");
            MantaFS.writeFile("/config/profile.json", "{\"active\":true}");
            MantaDownloads.save("export.txt", "Exported result: " + result);
        "#;

        vm.execute_script_full(
            script,
            "https://app.manta.dev/index.html",
            &mut dom,
            &mut clip,
            &mut storage,
            &mut downloads,
        )
        .unwrap();

        assert_eq!(vm.get_global("result"), Some(&JsValue::Number(50.0)));
        let email_id = dom.find_by_id("email").unwrap();
        assert_eq!(
            dom.forms.get_control(email_id).unwrap().value,
            "dev@manta.browser"
        );
        assert_eq!(
            storage.get_form_draft("https://app.manta.dev", "email"),
            Some("dev@manta.browser".to_string())
        );
        assert_eq!(
            storage.get_cookie_header_for_url("https://app.manta.dev", false),
            "session_id=manta_999"
        );
        assert_eq!(
            storage
                .db
                .get_object("https://app.manta.dev", "users", "u1"),
            Some("{\"email\":\"dev@manta.browser\"}".to_string())
        );
        assert!(storage
            .db
            .read_virtual_file("https://app.manta.dev", "/config/profile.json")
            .is_some());
        assert!(downloads.get_download(1).is_some());
    }
}
