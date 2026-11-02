use std::collections::HashMap;

use crate::clipboard::ClipboardSandbox;
use crate::dom_arena::ZeroCopyDomArena;
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
    CallHost { host_fn_id: u16, argc: u8 },
    Pop,
    Halt,
}

/// Pre-compiled, constant-deduplicated bytecode program.
#[derive(Debug, Clone, Default)]
pub struct CompiledJsProgram {
    pub constants: Vec<JsValue>,
    pub identifiers: Vec<String>,
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

/// Lexical analyzer converting JS source into a flat token stream.
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

/// Recursive-descent Bytecode Compiler (`JsBytecodeCompiler`).
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
                self.program.instructions.push(OpCode::Halt);
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
        // Handle `document.getElementById("id").textContent = expr` or `document.querySelector("sel").textContent = expr`
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
                        if clean_prop == "textContent"
                            || clean_prop == "innerText"
                            || clean_prop == "innerHTML"
                        {
                            self.advance();
                            if self.peek() == &Token::Assign {
                                self.advance();
                                self.compile_expr()?;
                                let host_name = if fn_name == "document.getElementById" {
                                    "__dom_set_text_by_id"
                                } else {
                                    "__dom_set_text_by_sel"
                                };
                                let fn_id = self.program.add_ident(host_name);
                                self.program.instructions.push(OpCode::CallHost {
                                    host_fn_id: fn_id,
                                    argc: 2,
                                });
                                return Ok(());
                            } else {
                                let host_name = if fn_name == "document.getElementById" {
                                    "__dom_get_text_by_id"
                                } else {
                                    "__dom_get_text_by_sel"
                                };
                                let fn_id = self.program.add_ident(host_name);
                                self.program.instructions.push(OpCode::CallHost {
                                    host_fn_id: fn_id,
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
                    self.program.instructions.push(OpCode::CallHost {
                        host_fn_id: fn_id,
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

/// High-Efficiency Bytecode JS Sandbox (`MantaJsSandbox`).
/// - Compiles JS into compact stack bytecode (`CompiledJsProgram`).
/// - Fuel-metered execution (prevents infinite loops and CPU/battery drain).
/// - Standard Chrome globals (`navigator.webdriver = false`, `navigator.cookieEnabled = true`).
/// - Origin-isolated bindings to `ZeroCopyDomArena`, `MantaStorageEngine`, and `ClipboardSandbox`.
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
            JsValue::Str(
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36".to_string(),
            ),
        );
        globals.insert("window.isSecureContext".to_string(), JsValue::Bool(true));

        Self {
            globals,
            fuel_remaining: max_fuel,
            max_fuel,
            console_logs: Vec::new(),
        }
    }

    /// Compiles and executes a script inside the fuel-limited bytecode VM.
    pub fn execute_script(
        &mut self,
        script: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
    ) -> Result<JsValue, &'static str> {
        let mut dummy_storage = MantaStorageEngine::new(64 * 1024, 256 * 1024);
        self.execute_script_with_storage(
            script,
            "https://manta.local",
            dom,
            clipboard,
            &mut dummy_storage,
        )
    }

    /// Compiles and executes a script with full origin-isolated access to Manta's custom storage engine.
    pub fn execute_script_with_storage(
        &mut self,
        script: &str,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
    ) -> Result<JsValue, &'static str> {
        let program = JsBytecodeCompiler::compile(script)?;
        self.run_bytecode(&program, origin_url, dom, clipboard, storage)
    }

    pub fn run_bytecode(
        &mut self,
        program: &CompiledJsProgram,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
    ) -> Result<JsValue, &'static str> {
        self.fuel_remaining = self.max_fuel;
        let mut stack: Vec<JsValue> = Vec::with_capacity(32);
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
                    let name = program
                        .identifiers
                        .get(*idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let val = if name == "document.title" {
                        JsValue::Str(dom.page_title.clone())
                    } else {
                        self.globals
                            .get(name)
                            .cloned()
                            .unwrap_or(JsValue::Undefined)
                    };
                    stack.push(val);
                    pc += 1;
                }
                OpCode::StoreVar(idx) => {
                    let val = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(name) = program.identifiers.get(*idx as usize) {
                        if name == "document.title" {
                            dom.page_title = val.as_str();
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
                OpCode::CallHost { host_fn_id, argc } => {
                    let count = *argc as usize;
                    let mut args = Vec::with_capacity(count);
                    for _ in 0..count {
                        args.push(stack.pop().unwrap_or(JsValue::Undefined));
                    }
                    args.reverse();
                    let fn_name = program
                        .identifiers
                        .get(*host_fn_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let ret = self.invoke_host_fn(fn_name, &args, origin_url, dom, clipboard, storage);
                    stack.push(ret);
                    pc += 1;
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

    fn invoke_host_fn(
        &mut self,
        fn_name: &str,
        args: &[JsValue],
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
    ) -> JsValue {
        match fn_name {
            "__dom_set_text_by_id" => {
                if args.len() >= 2 {
                    let id = args[0].as_str();
                    let val = args[1].as_str();
                    if let Some(nid) = dom.find_by_id(&id) {
                        dom.set_text_content(nid, val);
                    }
                    return args[1].clone();
                }
                JsValue::Undefined
            }
            "__dom_set_text_by_sel" => {
                if args.len() >= 2 {
                    let sel = args[0].as_str();
                    let val = args[1].as_str();
                    if let Some(nid) = dom.query_selector(&sel) {
                        dom.set_text_content(nid, val);
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
            "__dom_get_text_by_sel" => {
                if let Some(sel_arg) = args.first() {
                    if let Some(nid) = dom.query_selector(&sel_arg.as_str()) {
                        return JsValue::Str(dom.text_content(nid));
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
            "navigator.clipboard.writeText" => {
                if let Some(v) = args.first() {
                    clipboard.copy_text(&v.as_str());
                }
                JsValue::Undefined
            }
            "console.log" | "console.info" | "console.warn" | "console.error" => {
                if self.console_logs.len() < 32 {
                    let joined: Vec<String> = args.iter().map(|a| a.as_str()).collect();
                    self.console_logs.push(joined.join(" "));
                }
                JsValue::Undefined
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
    fn test_bytecode_compiler_dom_and_custom_storage() {
        let mut dom = ZeroCopyDomArena::new(64 * 1024);
        let mut tokenizer = StreamingHtmlTokenizer::new();
        tokenizer.feed_chunk(b"<div id=\"status\">Idle</div>", true, |tok| {
            dom.ingest_token(tok);
        });

        let mut vm = MantaJsSandbox::new(5000);
        let mut clip = ClipboardSandbox::new(1024);
        let mut storage = MantaStorageEngine::new(64 * 1024, 256 * 1024);

        let script = r#"
            let bot = navigator.webdriver;
            let count = 0;
            let i = 1;
            while (i <= 5) {
                count += i;
                i += 1;
            }
            if (count == 15) {
                document.getElementById("status").textContent = "Sum: " + count;
            }
            localStorage.setItem("saved_sum", "" + count);
            navigator.clipboard.writeText("Copied from Bytecode VM");
        "#;
        vm.execute_script_with_storage(
            script,
            "https://app.example.com/index.html",
            &mut dom,
            &mut clip,
            &mut storage,
        )
        .unwrap();

        assert_eq!(vm.get_global("bot"), Some(&JsValue::Bool(false)));
        assert_eq!(vm.get_global("count"), Some(&JsValue::Number(15.0)));
        let nid = dom.find_by_id("status").unwrap();
        assert_eq!(dom.text_content(nid), "Sum: 15");
        assert_eq!(
            storage.get_local_storage("https://app.example.com", "saved_sum"),
            Some("15".to_string())
        );
        assert_eq!(clip.paste_text(), "Copied from Bytecode VM");
    }
}
