use std::collections::{HashMap, VecDeque};

use crate::clipboard::ClipboardSandbox;
use crate::dom_arena::{NodeId, ZeroCopyDomArena};
use crate::downloads_engine::MantaDownloadManager;
use crate::storage_engine::MantaStorageEngine;

const MAX_VM_SCOPES: usize = 32_768;
const MAX_VM_HEAP_CONTAINERS: usize = 16_384;

#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Object(u32),
    Array(u32),
    DomNode(NodeId),
    Fn(u32),
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
                } else if n.is_infinite() {
                    if *n > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
                } else if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{:.0}", n)
                } else {
                    n.to_string()
                }
            }
            JsValue::Str(s) => s.clone(),
            JsValue::Object(_) => "[object Object]".to_string(),
            JsValue::Array(_) => "[object Array]".to_string(),
            JsValue::DomNode(id) => format!("[object Element#{}]", id),
            JsValue::Fn(_) => "function() { [native/bytecode] }".to_string(),
        }
    }

    pub fn as_number(&self) -> f64 {
        match self {
            JsValue::Undefined => f64::NAN,
            JsValue::Null | JsValue::Bool(false) => 0.0,
            JsValue::Bool(true) => 1.0,
            JsValue::Number(n) => *n,
            JsValue::Str(s) => {
                let t = s.trim();
                if t.is_empty() {
                    0.0
                } else {
                    t.parse::<f64>().unwrap_or(f64::NAN)
                }
            }
            _ => f64::NAN,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsValue::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn type_of(&self) -> &'static str {
        match self {
            JsValue::Undefined => "undefined",
            JsValue::Null => "object",
            JsValue::Bool(_) => "boolean",
            JsValue::Number(_) => "number",
            JsValue::Str(_) => "string",
            JsValue::Object(_) | JsValue::Array(_) | JsValue::DomNode(_) => "object",
            JsValue::Fn(_) => "function",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JsFunctionDef {
    pub name: Option<String>,
    pub params: Vec<u16>,
    pub entry_pc: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpCode {
    LoadConst(u16),
    LoadIdent(u16),
    DeclVar(u16),
    StoreVar(u16),
    MakeArray(u16),
    MakeObject(u16),
    MakeClosure(u16),
    GetProp(u16),
    SetProp(u16),
    GetIndex,
    SetIndex,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Not,
    TypeOf,
    Eq,
    Neq,
    StrictEq,
    StrictNeq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfTrue(usize),
    PushTry(usize),
    PopTry,
    Throw,
    CallValue { argc: u8 },
    CallMethod { prop_id: u16, argc: u8 },
    Return,
    Pop,
    Dup,
    Halt,
}

#[derive(Debug, Clone, Default)]
pub struct CompiledJsProgram {
    pub constants: Vec<JsValue>,
    pub identifiers: Vec<String>,
    pub functions: Vec<JsFunctionDef>,
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
    KeywordFor,
    KeywordOf,
    KeywordBreak,
    KeywordContinue,
    KeywordSwitch,
    KeywordCase,
    KeywordDefault,
    KeywordTry,
    KeywordCatch,
    KeywordThrow,
    KeywordReturn,
    KeywordTypeof,
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
    Comma,
    Colon,
    Question,
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
    Bang,
    Eof,
}

struct JsLexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsLexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::with_capacity(64);
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b.is_ascii_whitespace() {
                self.pos += 1;
                continue;
            }
            // Comments
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

            // Strings ('...', "...", `...`)
            if b == b'"' || b == b'\'' || b == b'`' {
                let quote = b;
                self.pos += 1;
                let mut s = String::new();
                while self.pos < self.bytes.len() {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    self.pos += ch.len_utf8();
                    if ch as u8 == quote && ch.len_utf8() == 1 {
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

            // Numbers
            if b.is_ascii_digit() {
                let start = self.pos;
                if b == b'0' && self.pos + 1 < self.bytes.len()
                    && (self.bytes[self.pos + 1] == b'x' || self.bytes[self.pos + 1] == b'X')
                {
                    self.pos += 2;
                    while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_hexdigit() {
                        self.pos += 1;
                    }
                    let val = u64::from_str_radix(&self.src[start + 2..self.pos], 16).unwrap_or(0) as f64;
                    tokens.push(Token::Number(val));
                    continue;
                }
                while self.pos < self.bytes.len()
                    && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'.')
                {
                    self.pos += 1;
                }
                let val = self.src[start..self.pos].parse::<f64>().unwrap_or(0.0);
                tokens.push(Token::Number(val));
                continue;
            }

            // Identifiers & Keywords (Dot is now separate!)
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
                    "let" | "const" | "var" => Token::KeywordLet,
                    "if" => Token::KeywordIf,
                    "else" => Token::KeywordElse,
                    "while" => Token::KeywordWhile,
                    "for" => Token::KeywordFor,
                    "of" => Token::KeywordOf,
                    "break" => Token::KeywordBreak,
                    "continue" => Token::KeywordContinue,
                    "switch" => Token::KeywordSwitch,
                    "case" => Token::KeywordCase,
                    "default" => Token::KeywordDefault,
                    "try" => Token::KeywordTry,
                    "catch" => Token::KeywordCatch,
                    "throw" => Token::KeywordThrow,
                    "return" => Token::KeywordReturn,
                    "typeof" => Token::KeywordTypeof,
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

            self.pos += 1;
            let peek = self.bytes.get(self.pos).copied();
            match b {
                b'(' => tokens.push(Token::LParen),
                b')' => tokens.push(Token::RParen),
                b'{' => tokens.push(Token::LBrace),
                b'}' => tokens.push(Token::RBrace),
                b'[' => tokens.push(Token::LBracket),
                b']' => tokens.push(Token::RBracket),
                b'.' => tokens.push(Token::Dot),
                b',' => tokens.push(Token::Comma),
                b':' => tokens.push(Token::Colon),
                b'?' => tokens.push(Token::Question),
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
                    if peek == Some(b'=') {
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
                    if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::Lte);
                    } else {
                        tokens.push(Token::Lt);
                    }
                }
                b'>' => {
                    if peek == Some(b'=') {
                        self.pos += 1;
                        tokens.push(Token::Gte);
                    } else {
                        tokens.push(Token::Gt);
                    }
                }
                b'&' if peek == Some(b'&') => {
                    self.pos += 1;
                    tokens.push(Token::AndAnd);
                }
                b'|' if peek == Some(b'|') => {
                    self.pos += 1;
                    tokens.push(Token::OrOr);
                }
                _ => {}
            }
        }
        tokens.push(Token::Eof);
        tokens
    }
}

#[derive(Debug, Clone)]
enum LValue {
    Var(u16),
    Prop { obj_start_pc: usize, prop_id: u16 },
    Index,
}

struct LoopPatchContext {
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
}

/// Complete Lexical-Scoped JavaScript Bytecode Compiler (`JsBytecodeCompiler`):
/// Supports `function`, arrow functions `=>`, closures, objects `{}` / arrays `[]`,
/// `for` / `for..of` / `while` / `break` / `continue`, `switch`, `try/catch/throw`,
/// ternary `?:`, member chains `a.b[c].d()`, and `typeof` / `new`.
pub struct JsBytecodeCompiler {
    tokens: Vec<Token>,
    pos: usize,
    program: CompiledJsProgram,
    loop_stack: Vec<LoopPatchContext>,
}

impl JsBytecodeCompiler {
    pub fn compile(source: &str) -> Result<CompiledJsProgram, &'static str> {
        let mut lexer = JsLexer::new(source);
        let tokens = lexer.tokenize();
        let mut compiler = Self {
            tokens,
            pos: 0,
            program: CompiledJsProgram::default(),
            loop_stack: Vec::new(),
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

    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens.get(self.pos + offset).unwrap_or(&Token::Eof)
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
                let fn_name = match self.peek().clone() {
                    Token::Ident(n) => {
                        self.advance();
                        Some(n)
                    }
                    _ => None,
                };
                let fn_idx = self.compile_function_body(fn_name.clone())?;
                self.program.instructions.push(OpCode::MakeClosure(fn_idx));
                if let Some(name) = fn_name {
                    let id = self.program.add_ident(&name);
                    self.program.instructions.push(OpCode::DeclVar(id));
                }
                self.program.instructions.push(OpCode::Pop);
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
                loop {
                    let Token::Ident(var_name) = self.advance() else {
                        return Err("Expected variable identifier");
                    };
                    let var_id = self.program.add_ident(&var_name);
                    if self.peek() == &Token::Assign {
                        self.advance();
                        self.compile_expr()?;
                    } else {
                        let undef_id = self.program.add_const(JsValue::Undefined);
                        self.program.instructions.push(OpCode::LoadConst(undef_id));
                    }
                    self.program.instructions.push(OpCode::DeclVar(var_id));
                    self.program.instructions.push(OpCode::Pop);
                    if self.peek() == &Token::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.consume_optional_semicolons();
                Ok(())
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

                self.loop_stack.push(LoopPatchContext {
                    break_jumps: Vec::new(),
                    continue_jumps: Vec::new(),
                });
                self.compile_statement()?;
                let ctx = self.loop_stack.pop().unwrap();

                for cj in ctx.continue_jumps {
                    self.program.instructions[cj] = OpCode::Jump(loop_start);
                }
                self.program.instructions.push(OpCode::Jump(loop_start));
                let loop_end = self.program.instructions.len();
                self.program.instructions[exit_jump_idx] = OpCode::JumpIfFalse(loop_end);
                for bj in ctx.break_jumps {
                    self.program.instructions[bj] = OpCode::Jump(loop_end);
                }
                Ok(())
            }
            Token::KeywordFor => self.compile_for_statement(),
            Token::KeywordBreak => {
                self.advance();
                let j_idx = self.program.instructions.len();
                self.program.instructions.push(OpCode::Jump(0));
                if let Some(ctx) = self.loop_stack.last_mut() {
                    ctx.break_jumps.push(j_idx);
                }
                self.consume_optional_semicolons();
                Ok(())
            }
            Token::KeywordContinue => {
                self.advance();
                let j_idx = self.program.instructions.len();
                self.program.instructions.push(OpCode::Jump(0));
                if let Some(ctx) = self.loop_stack.last_mut() {
                    ctx.continue_jumps.push(j_idx);
                }
                self.consume_optional_semicolons();
                Ok(())
            }
            Token::KeywordSwitch => self.compile_switch_statement(),
            Token::KeywordTry => {
                self.advance();
                let try_idx = self.program.instructions.len();
                self.program.instructions.push(OpCode::PushTry(0));
                self.compile_statement()?;
                self.program.instructions.push(OpCode::PopTry);
                let jump_after_catch = self.program.instructions.len();
                self.program.instructions.push(OpCode::Jump(0));

                if self.peek() == &Token::KeywordCatch {
                    let catch_pc = self.program.instructions.len();
                    self.program.instructions[try_idx] = OpCode::PushTry(catch_pc);
                    self.advance();
                    if self.peek() == &Token::LParen {
                        self.advance();
                        if let Token::Ident(err_name) = self.advance() {
                            let err_id = self.program.add_ident(&err_name);
                            self.program.instructions.push(OpCode::DeclVar(err_id));
                        }
                        if self.peek() == &Token::RParen {
                            self.advance();
                        }
                    }
                    self.program.instructions.push(OpCode::Pop);
                    self.compile_statement()?;
                } else {
                    let after_pc = self.program.instructions.len();
                    self.program.instructions[try_idx] = OpCode::PushTry(after_pc);
                    self.program.instructions.push(OpCode::Pop);
                }
                let end_pc = self.program.instructions.len();
                self.program.instructions[jump_after_catch] = OpCode::Jump(end_pc);
                Ok(())
            }
            Token::KeywordThrow => {
                self.advance();
                self.compile_expr()?;
                self.program.instructions.push(OpCode::Throw);
                self.consume_optional_semicolons();
                Ok(())
            }
            Token::KeywordReturn => {
                self.advance();
                if self.peek() != &Token::Semicolon
                    && self.peek() != &Token::RBrace
                    && self.peek() != &Token::Eof
                {
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

    fn compile_for_statement(&mut self) -> Result<(), &'static str> {
        self.advance(); // 'for'
        if self.peek() == &Token::LParen {
            self.advance();
        }

        // Check `for (let x of arr)` or `for (x of arr)`
        let is_for_of = (self.peek() == &Token::KeywordLet
            && matches!(self.peek_at(1), Token::Ident(_))
            && self.peek_at(2) == &Token::KeywordOf)
            || (matches!(self.peek(), Token::Ident(_)) && self.peek_at(1) == &Token::KeywordOf);

        if is_for_of {
            let is_decl = self.peek() == &Token::KeywordLet;
            if is_decl {
                self.advance();
            }
            let Token::Ident(item_name) = self.advance() else {
                return Err("Expected loop variable");
            };
            self.advance(); // 'of'
            self.compile_expr()?;
            if self.peek() == &Token::RParen {
                self.advance();
            }

            let arr_tmp_id = self.program.add_ident("__forof_arr");
            let idx_tmp_id = self.program.add_ident("__forof_idx");
            let len_prop = self.program.add_ident("length");
            let item_id = self.program.add_ident(&item_name);

            self.program.instructions.push(OpCode::DeclVar(arr_tmp_id));
            self.program.instructions.push(OpCode::Pop);
            let zero_c = self.program.add_const(JsValue::Number(0.0));
            let one_c = self.program.add_const(JsValue::Number(1.0));
            self.program.instructions.push(OpCode::LoadConst(zero_c));
            self.program.instructions.push(OpCode::DeclVar(idx_tmp_id));
            self.program.instructions.push(OpCode::Pop);

            let loop_cond = self.program.instructions.len();
            self.program.instructions.push(OpCode::LoadIdent(idx_tmp_id));
            self.program.instructions.push(OpCode::LoadIdent(arr_tmp_id));
            self.program.instructions.push(OpCode::GetProp(len_prop));
            self.program.instructions.push(OpCode::Lt);
            let exit_jump = self.program.instructions.len();
            self.program.instructions.push(OpCode::JumpIfFalse(0));

            self.program.instructions.push(OpCode::LoadIdent(arr_tmp_id));
            self.program.instructions.push(OpCode::LoadIdent(idx_tmp_id));
            self.program.instructions.push(OpCode::GetIndex);
            if is_decl {
                self.program.instructions.push(OpCode::DeclVar(item_id));
            } else {
                self.program.instructions.push(OpCode::StoreVar(item_id));
            }
            self.program.instructions.push(OpCode::Pop);

            self.loop_stack.push(LoopPatchContext {
                break_jumps: Vec::new(),
                continue_jumps: Vec::new(),
            });
            self.compile_statement()?;
            let ctx = self.loop_stack.pop().unwrap();

            let inc_pc = self.program.instructions.len();
            for cj in ctx.continue_jumps {
                self.program.instructions[cj] = OpCode::Jump(inc_pc);
            }
            self.program.instructions.push(OpCode::LoadIdent(idx_tmp_id));
            self.program.instructions.push(OpCode::LoadConst(one_c));
            self.program.instructions.push(OpCode::Add);
            self.program.instructions.push(OpCode::StoreVar(idx_tmp_id));
            self.program.instructions.push(OpCode::Pop);
            self.program.instructions.push(OpCode::Jump(loop_cond));

            let end_pc = self.program.instructions.len();
            self.program.instructions[exit_jump] = OpCode::JumpIfFalse(end_pc);
            for bj in ctx.break_jumps {
                self.program.instructions[bj] = OpCode::Jump(end_pc);
            }
            return Ok(());
        }

        // Standard C-style `for (init; cond; step) body`
        if self.peek() != &Token::Semicolon {
            if self.peek() == &Token::KeywordLet {
                self.compile_statement()?;
            } else {
                self.compile_expr()?;
                self.program.instructions.push(OpCode::Pop);
                self.consume_optional_semicolons();
            }
        } else {
            self.advance();
        }

        let cond_pc = self.program.instructions.len();
        let exit_jump = if self.peek() != &Token::Semicolon {
            self.compile_expr()?;
            let j = self.program.instructions.len();
            self.program.instructions.push(OpCode::JumpIfFalse(0));
            Some(j)
        } else {
            None
        };
        if self.peek() == &Token::Semicolon {
            self.advance();
        }

        // Capture step expression tokens by compiling into jump-over block
        let jump_to_body = self.program.instructions.len();
        self.program.instructions.push(OpCode::Jump(0));

        let step_pc = self.program.instructions.len();
        if self.peek() != &Token::RParen {
            self.compile_expr()?;
            self.program.instructions.push(OpCode::Pop);
        }
        if self.peek() == &Token::RParen {
            self.advance();
        }
        self.program.instructions.push(OpCode::Jump(cond_pc));

        let body_pc = self.program.instructions.len();
        self.program.instructions[jump_to_body] = OpCode::Jump(body_pc);

        self.loop_stack.push(LoopPatchContext {
            break_jumps: Vec::new(),
            continue_jumps: Vec::new(),
        });
        self.compile_statement()?;
        let ctx = self.loop_stack.pop().unwrap();

        for cj in ctx.continue_jumps {
            self.program.instructions[cj] = OpCode::Jump(step_pc);
        }
        self.program.instructions.push(OpCode::Jump(step_pc));

        let end_pc = self.program.instructions.len();
        if let Some(ej) = exit_jump {
            self.program.instructions[ej] = OpCode::JumpIfFalse(end_pc);
        }
        for bj in ctx.break_jumps {
            self.program.instructions[bj] = OpCode::Jump(end_pc);
        }
        Ok(())
    }

    fn compile_switch_statement(&mut self) -> Result<(), &'static str> {
        self.advance(); // 'switch'
        if self.peek() == &Token::LParen {
            self.advance();
        }
        self.compile_expr()?;
        if self.peek() == &Token::RParen {
            self.advance();
        }
        let switch_var = self.program.add_ident("__switch_val");
        self.program.instructions.push(OpCode::DeclVar(switch_var));
        self.program.instructions.push(OpCode::Pop);

        if self.peek() == &Token::LBrace {
            self.advance();
        }

        self.loop_stack.push(LoopPatchContext {
            break_jumps: Vec::new(),
            continue_jumps: Vec::new(),
        });
        let mut next_case_patch: Option<usize> = None;
        let mut fallthrough_patch: Option<usize> = None;

        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            if self.peek() == &Token::KeywordCase {
                if let Some(ft) = fallthrough_patch.take() {
                    // Fallthrough from previous case straight into body
                    let body_skip = self.program.instructions.len();
                    self.program.instructions.push(OpCode::Jump(0));
                    let test_pc = self.program.instructions.len();
                    if let Some(nc) = next_case_patch.take() {
                        self.program.instructions[nc] = OpCode::JumpIfFalse(test_pc);
                    }
                    self.program.instructions[ft] = OpCode::Jump(test_pc);
                    let _ = body_skip;
                    // Re-adjust cleanly: patch next_case to test_pc, fallthrough jumps after test
                    self.advance();
                    self.program.instructions.push(OpCode::LoadIdent(switch_var));
                    self.compile_expr()?;
                    self.program.instructions.push(OpCode::StrictEq);
                    if self.peek() == &Token::Colon {
                        self.advance();
                    }
                    next_case_patch = Some(self.program.instructions.len());
                    self.program.instructions.push(OpCode::JumpIfFalse(0));
                    let body_pc = self.program.instructions.len();
                    self.program.instructions[body_skip] = OpCode::Jump(body_pc);
                } else {
                    let test_pc = self.program.instructions.len();
                    if let Some(nc) = next_case_patch.take() {
                        self.program.instructions[nc] = OpCode::JumpIfFalse(test_pc);
                    }
                    self.advance();
                    self.program.instructions.push(OpCode::LoadIdent(switch_var));
                    self.compile_expr()?;
                    self.program.instructions.push(OpCode::StrictEq);
                    if self.peek() == &Token::Colon {
                        self.advance();
                    }
                    next_case_patch = Some(self.program.instructions.len());
                    self.program.instructions.push(OpCode::JumpIfFalse(0));
                }

                while !matches!(
                    self.peek(),
                    Token::KeywordCase | Token::KeywordDefault | Token::RBrace | Token::Eof
                ) {
                    self.compile_statement()?;
                }
                if matches!(self.peek(), Token::KeywordCase | Token::KeywordDefault) {
                    let j = self.program.instructions.len();
                    self.program.instructions.push(OpCode::Jump(0));
                    fallthrough_patch = Some(j);
                }
            } else if self.peek() == &Token::KeywordDefault {
                let def_pc = self.program.instructions.len();
                if let Some(nc) = next_case_patch.take() {
                    self.program.instructions[nc] = OpCode::JumpIfFalse(def_pc);
                }
                if let Some(ft) = fallthrough_patch.take() {
                    self.program.instructions[ft] = OpCode::Jump(def_pc);
                }
                self.advance();
                if self.peek() == &Token::Colon {
                    self.advance();
                }
                while !matches!(self.peek(), Token::KeywordCase | Token::RBrace | Token::Eof) {
                    self.compile_statement()?;
                }
            } else {
                self.advance();
            }
        }
        if self.peek() == &Token::RBrace {
            self.advance();
        }
        let end_pc = self.program.instructions.len();
        if let Some(nc) = next_case_patch {
            self.program.instructions[nc] = OpCode::JumpIfFalse(end_pc);
        }
        if let Some(ft) = fallthrough_patch {
            self.program.instructions[ft] = OpCode::Jump(end_pc);
        }
        let ctx = self.loop_stack.pop().unwrap();
        for bj in ctx.break_jumps {
            self.program.instructions[bj] = OpCode::Jump(end_pc);
        }
        Ok(())
    }

    fn compile_function_body(&mut self, name: Option<String>) -> Result<u16, &'static str> {
        if self.peek() == &Token::LParen {
            self.advance();
        }
        let mut params = Vec::new();
        while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
            if let Token::Ident(p) = self.advance() {
                params.push(self.program.add_ident(&p));
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
        let fn_idx = self.program.functions.len() as u16;
        self.program.functions.push(JsFunctionDef {
            name,
            params,
            entry_pc,
        });

        self.compile_statement()?;
        let undef_c = self.program.add_const(JsValue::Undefined);
        self.program.instructions.push(OpCode::LoadConst(undef_c));
        self.program.instructions.push(OpCode::Return);

        let after_fn = self.program.instructions.len();
        self.program.instructions[skip_jump_idx] = OpCode::Jump(after_fn);
        Ok(fn_idx)
    }

    fn compile_expr(&mut self) -> Result<(), &'static str> {
        self.compile_assignment()
    }

    fn compile_assignment(&mut self) -> Result<(), &'static str> {
        // Arrow function detection: `x => ...` or `(a, b) => ...`
        if self.is_arrow_function_ahead() {
            return self.compile_arrow_function();
        }

        // Fast LHS parsing for assignment (`x = y`, `obj.prop = y`, `arr[i] = y`)
        let saved_pos = self.pos;
        let saved_instr_len = self.program.instructions.len();

        if let Some(lval) = self.try_compile_lvalue()? {
            if matches!(
                self.peek(),
                Token::Assign
                    | Token::PlusAssign
                    | Token::MinusAssign
                    | Token::MulAssign
                    | Token::DivAssign
            ) {
                let op = self.advance();
                match lval {
                    LValue::Var(var_id) => {
                        if op != Token::Assign {
                            self.program.instructions.push(OpCode::LoadIdent(var_id));
                            self.compile_assignment()?;
                            match op {
                                Token::PlusAssign => self.program.instructions.push(OpCode::Add),
                                Token::MinusAssign => self.program.instructions.push(OpCode::Sub),
                                Token::MulAssign => self.program.instructions.push(OpCode::Mul),
                                Token::DivAssign => self.program.instructions.push(OpCode::Div),
                                _ => {}
                            }
                        } else {
                            self.compile_assignment()?;
                        }
                        self.program.instructions.push(OpCode::StoreVar(var_id));
                        return Ok(());
                    }
                    LValue::Prop { prop_id, .. } => {
                        if op != Token::Assign {
                            self.program.instructions.push(OpCode::Dup);
                            self.program.instructions.push(OpCode::GetProp(prop_id));
                            self.compile_assignment()?;
                            match op {
                                Token::PlusAssign => self.program.instructions.push(OpCode::Add),
                                Token::MinusAssign => self.program.instructions.push(OpCode::Sub),
                                Token::MulAssign => self.program.instructions.push(OpCode::Mul),
                                Token::DivAssign => self.program.instructions.push(OpCode::Div),
                                _ => {}
                            }
                        } else {
                            self.compile_assignment()?;
                        }
                        self.program.instructions.push(OpCode::SetProp(prop_id));
                        return Ok(());
                    }
                    LValue::Index => {
                        if op == Token::Assign {
                            self.compile_assignment()?;
                            self.program.instructions.push(OpCode::SetIndex);
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Not an assignment -> rewind cleanly and compile ternary expression
        self.pos = saved_pos;
        self.program.instructions.truncate(saved_instr_len);
        self.compile_ternary()
    }

    fn is_arrow_function_ahead(&self) -> bool {
        if matches!(self.peek(), Token::Ident(_)) && self.peek_at(1) == &Token::Arrow {
            return true;
        }
        if self.peek() == &Token::LParen {
            let mut idx = self.pos + 1;
            let mut depth = 1usize;
            while idx < self.tokens.len() && depth > 0 {
                match self.tokens[idx] {
                    Token::LParen => depth += 1,
                    Token::RParen => depth -= 1,
                    Token::Eof => return false,
                    _ => {}
                }
                idx += 1;
            }
            return self.tokens.get(idx) == Some(&Token::Arrow);
        }
        false
    }

    fn compile_arrow_function(&mut self) -> Result<(), &'static str> {
        let mut params = Vec::new();
        if let Token::Ident(p) = self.peek().clone() {
            self.advance();
            params.push(self.program.add_ident(&p));
        } else if self.peek() == &Token::LParen {
            self.advance();
            while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                if let Token::Ident(p) = self.advance() {
                    params.push(self.program.add_ident(&p));
                }
                if self.peek() == &Token::Comma {
                    self.advance();
                }
            }
            if self.peek() == &Token::RParen {
                self.advance();
            }
        }
        if self.peek() == &Token::Arrow {
            self.advance();
        }

        let skip_jump = self.program.instructions.len();
        self.program.instructions.push(OpCode::Jump(0));

        let entry_pc = self.program.instructions.len();
        let fn_idx = self.program.functions.len() as u16;
        self.program.functions.push(JsFunctionDef {
            name: None,
            params,
            entry_pc,
        });

        if self.peek() == &Token::LBrace {
            self.compile_statement()?;
            let undef = self.program.add_const(JsValue::Undefined);
            self.program.instructions.push(OpCode::LoadConst(undef));
            self.program.instructions.push(OpCode::Return);
        } else {
            self.compile_expr()?;
            self.program.instructions.push(OpCode::Return);
        }

        let after = self.program.instructions.len();
        self.program.instructions[skip_jump] = OpCode::Jump(after);
        self.program.instructions.push(OpCode::MakeClosure(fn_idx));
        Ok(())
    }

    fn try_compile_lvalue(&mut self) -> Result<Option<LValue>, &'static str> {
        let Token::Ident(name) = self.peek().clone() else {
            return Ok(None);
        };
        if matches!(
            self.peek_at(1),
            Token::Assign
                | Token::PlusAssign
                | Token::MinusAssign
                | Token::MulAssign
                | Token::DivAssign
        ) {
            self.advance();
            return Ok(Some(LValue::Var(self.program.add_ident(&name))));
        }

        // Member chain ending in `.prop` or `[idx]`
        if !matches!(self.peek_at(1), Token::Dot | Token::LBracket | Token::LParen) {
            return Ok(None);
        }

        let start_pc = self.program.instructions.len();
        self.advance();
        let root_id = self.program.add_ident(&name);
        self.program.instructions.push(OpCode::LoadIdent(root_id));
        let mut current_lval: Option<LValue> = None;

        loop {
            if self.peek() == &Token::Dot {
                if let Some(LValue::Prop { prop_id, .. }) = current_lval.take() {
                    self.program.instructions.push(OpCode::GetProp(prop_id));
                } else if let Some(LValue::Index) = current_lval.take() {
                    self.program.instructions.push(OpCode::GetIndex);
                }
                self.advance();
                let Token::Ident(prop) = self.advance() else {
                    return Ok(None);
                };
                let pid = self.program.add_ident(&prop);
                current_lval = Some(LValue::Prop {
                    obj_start_pc: start_pc,
                    prop_id: pid,
                });
            } else if self.peek() == &Token::LBracket {
                if let Some(LValue::Prop { prop_id, .. }) = current_lval.take() {
                    self.program.instructions.push(OpCode::GetProp(prop_id));
                } else if let Some(LValue::Index) = current_lval.take() {
                    self.program.instructions.push(OpCode::GetIndex);
                }
                self.advance();
                self.compile_expr()?;
                if self.peek() == &Token::RBracket {
                    self.advance();
                }
                current_lval = Some(LValue::Index);
            } else if self.peek() == &Token::LParen {
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
                match current_lval.take() {
                    Some(LValue::Prop { prop_id, .. }) => {
                        self.program
                            .instructions
                            .push(OpCode::CallMethod { prop_id, argc });
                    }
                    _ => {
                        self.program.instructions.push(OpCode::CallValue { argc });
                    }
                }
            } else {
                break;
            }
        }
        Ok(current_lval)
    }

    fn compile_ternary(&mut self) -> Result<(), &'static str> {
        self.compile_logical_or()?;
        if self.peek() == &Token::Question {
            self.advance();
            let jf = self.program.instructions.len();
            self.program.instructions.push(OpCode::JumpIfFalse(0));
            self.compile_expr()?;
            if self.peek() == &Token::Colon {
                self.advance();
            }
            let je = self.program.instructions.len();
            self.program.instructions.push(OpCode::Jump(0));
            let else_pc = self.program.instructions.len();
            self.program.instructions[jf] = OpCode::JumpIfFalse(else_pc);
            self.compile_expr()?;
            let end_pc = self.program.instructions.len();
            self.program.instructions[je] = OpCode::Jump(end_pc);
        }
        Ok(())
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
        while matches!(
            self.peek(),
            Token::EqEq | Token::EqEqEq | Token::NotEq | Token::NotEqEq
        ) {
            let op = self.advance();
            self.compile_comparison()?;
            match op {
                Token::EqEq => self.program.instructions.push(OpCode::Eq),
                Token::EqEqEq => self.program.instructions.push(OpCode::StrictEq),
                Token::NotEq => self.program.instructions.push(OpCode::Neq),
                Token::NotEqEq => self.program.instructions.push(OpCode::StrictNeq),
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
        match self.peek() {
            Token::Minus => {
                self.advance();
                self.compile_unary()?;
                self.program.instructions.push(OpCode::Neg);
                Ok(())
            }
            Token::Bang => {
                self.advance();
                self.compile_unary()?;
                self.program.instructions.push(OpCode::Not);
                Ok(())
            }
            Token::KeywordTypeof => {
                self.advance();
                self.compile_unary()?;
                self.program.instructions.push(OpCode::TypeOf);
                Ok(())
            }
            Token::KeywordNew => {
                self.advance();
                self.compile_postfix()
            }
            Token::PlusPlus | Token::MinusMinus => {
                let is_inc = self.advance() == Token::PlusPlus;
                if let Token::Ident(name) = self.advance() {
                    let id = self.program.add_ident(&name);
                    let one = self.program.add_const(JsValue::Number(1.0));
                    self.program.instructions.push(OpCode::LoadIdent(id));
                    self.program.instructions.push(OpCode::LoadConst(one));
                    self.program.instructions.push(if is_inc { OpCode::Add } else { OpCode::Sub });
                    self.program.instructions.push(OpCode::StoreVar(id));
                    Ok(())
                } else {
                    Err("Expected identifier after ++/--")
                }
            }
            _ => self.compile_postfix(),
        }
    }

    fn compile_postfix(&mut self) -> Result<(), &'static str> {
        if let Token::Ident(name) = self.peek().clone() {
            if matches!(self.peek_at(1), Token::PlusPlus | Token::MinusMinus) {
                self.advance();
                let is_inc = self.advance() == Token::PlusPlus;
                let id = self.program.add_ident(&name);
                let one = self.program.add_const(JsValue::Number(1.0));
                self.program.instructions.push(OpCode::LoadIdent(id));
                self.program.instructions.push(OpCode::Dup);
                self.program.instructions.push(OpCode::LoadConst(one));
                self.program.instructions.push(if is_inc { OpCode::Add } else { OpCode::Sub });
                self.program.instructions.push(OpCode::StoreVar(id));
                self.program.instructions.push(OpCode::Pop);
                return Ok(());
            }
        }

        self.compile_primary()?;

        loop {
            if self.peek() == &Token::Dot {
                self.advance();
                let Token::Ident(prop) = self.advance() else {
                    return Err("Expected property name after `.`");
                };
                let prop_id = self.program.add_ident(&prop);
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
                    self.program
                        .instructions
                        .push(OpCode::CallMethod { prop_id, argc });
                } else {
                    self.program.instructions.push(OpCode::GetProp(prop_id));
                }
            } else if self.peek() == &Token::LBracket {
                self.advance();
                self.compile_expr()?;
                if self.peek() == &Token::RBracket {
                    self.advance();
                }
                self.program.instructions.push(OpCode::GetIndex);
            } else if self.peek() == &Token::LParen {
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
                self.program.instructions.push(OpCode::CallValue { argc });
            } else {
                break;
            }
        }
        Ok(())
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
            Token::LBracket => {
                let mut count = 0u16;
                while self.peek() != &Token::RBracket && self.peek() != &Token::Eof {
                    self.compile_expr()?;
                    count = count.saturating_add(1);
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                }
                if self.peek() == &Token::RBracket {
                    self.advance();
                }
                self.program.instructions.push(OpCode::MakeArray(count));
                Ok(())
            }
            Token::LBrace => {
                let mut pairs = 0u16;
                while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
                    let key_str = match self.advance() {
                        Token::Ident(k) | Token::Str(k) => k,
                        Token::Number(n) => JsValue::Number(n).as_str(),
                        _ => break,
                    };
                    let key_c = self.program.add_const(JsValue::Str(key_str));
                    self.program.instructions.push(OpCode::LoadConst(key_c));
                    if self.peek() == &Token::Colon {
                        self.advance();
                        self.compile_expr()?;
                    } else {
                        // ES6 shorthand `{ a }`
                        let id = self.program.add_ident(
                            &self.program.constants[key_c as usize].as_str(),
                        );
                        self.program.instructions.push(OpCode::LoadIdent(id));
                    }
                    pairs = pairs.saturating_add(1);
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                }
                if self.peek() == &Token::RBrace {
                    self.advance();
                }
                self.program.instructions.push(OpCode::MakeObject(pairs));
                Ok(())
            }
            Token::KeywordFunction => {
                let name = if let Token::Ident(n) = self.peek().clone() {
                    self.advance();
                    Some(n)
                } else {
                    None
                };
                let fn_idx = self.compile_function_body(name)?;
                self.program.instructions.push(OpCode::MakeClosure(fn_idx));
                Ok(())
            }
            Token::Ident(name) => {
                let id = self.program.add_ident(&name);
                self.program.instructions.push(OpCode::LoadIdent(id));
                Ok(())
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
struct ScopeFrame {
    parent: Option<u32>,
    vars: HashMap<String, JsValue>,
}

#[derive(Debug, Clone)]
enum Callable {
    UserClosure { fn_idx: u16, env_scope: u32 },
    Builtin(&'static str),
}

#[derive(Debug, Clone)]
struct CallFrame {
    return_pc: usize,
    prev_scope: u32,
    try_depth: usize,
}

#[derive(Debug, Clone)]
struct TryHandler {
    catch_pc: usize,
    stack_len: usize,
    call_depth: usize,
    scope_id: u32,
}

#[derive(Debug, Clone)]
struct Microtask {
    cb: JsValue,
    arg: JsValue,
}

/// High-Efficiency Lexical-Scoped Bytecode JS VM (`MantaJsSandbox`):
/// - Closures, Objects, Arrays, `for`/`for..of`/`try-catch`/`switch`/Arrow Functions/`Promise`/`JSON`.
/// - Fuel-metered + heap-bounded execution (zero runaway loops or OOM).
/// - Live DOM manipulation (`getElementById`, `querySelector`, `querySelectorAll`, `createElement`,
///   `appendChild`, `removeChild`, `classList`, `addEventListener`, `dispatchEvent`).
pub struct MantaJsSandbox {
    scopes: Vec<ScopeFrame>,
    objects: Vec<HashMap<String, JsValue>>,
    arrays: Vec<Vec<JsValue>>,
    callables: Vec<Callable>,
    microtasks: VecDeque<Microtask>,
    fuel_remaining: u32,
    max_fuel: u32,
    pub console_logs: Vec<String>,
}

impl MantaJsSandbox {
    pub fn new(max_fuel: u32) -> Self {
        let mut vm = Self {
            scopes: vec![ScopeFrame {
                parent: None,
                vars: HashMap::with_capacity(32),
            }],
            objects: Vec::with_capacity(32),
            arrays: Vec::with_capacity(16),
            callables: Vec::with_capacity(64),
            microtasks: VecDeque::new(),
            fuel_remaining: max_fuel,
            max_fuel,
            console_logs: Vec::new(),
        };
        vm.install_default_globals();
        vm
    }

    fn install_default_globals(&mut self) {
        self.register_builtin_fn("parseInt", "parseInt");
        self.register_builtin_fn("parseFloat", "parseFloat");
        self.register_builtin_fn("isNaN", "isNaN");
        self.register_builtin_fn("Number", "Number");
        self.register_builtin_fn("String", "String");
        self.register_builtin_fn("Boolean", "Boolean");
        self.register_builtin_fn("queueMicrotask", "queueMicrotask");
        self.register_builtin_fn("setTimeout", "setTimeout");
        self.register_builtin_fn("clearTimeout", "clearTimeout");

        // Namespaces mapped cleanly
        for ns in [
            "document",
            "window",
            "navigator",
            "location",
            "console",
            "localStorage",
            "sessionStorage",
            "MantaDB",
            "indexedDB",
            "MantaFS",
            "MantaForm",
            "MantaDownloads",
            "Math",
            "JSON",
            "Object",
            "Array",
            "Promise",
            "Date",
        ] {
            let mut map = HashMap::new();
            map.insert("__manta_ns".to_string(), JsValue::Str(ns.to_string()));
            if ns == "navigator" {
                map.insert("webdriver".to_string(), JsValue::Bool(false));
                map.insert("cookieEnabled".to_string(), JsValue::Bool(true));
                map.insert(
                    "userAgent".to_string(),
                    JsValue::Str("Manta/3.0 (Native Rust+C++ Engine; W3C Standard)".to_string()),
                );
                let clip_id = self.alloc_obj_map({
                    let mut cm = HashMap::new();
                    cm.insert("__manta_ns".to_string(), JsValue::Str("clipboard".to_string()));
                    cm
                });
                map.insert("clipboard".to_string(), JsValue::Object(clip_id));
            } else if ns == "window" {
                map.insert("isSecureContext".to_string(), JsValue::Bool(true));
            }
            let oid = self.alloc_obj_map(map);
            self.scopes[0].vars.insert(ns.to_string(), JsValue::Object(oid));
        }
    }

    fn register_builtin_fn(&mut self, global_name: &str, id: &'static str) {
        let cid = self.callables.len() as u32;
        self.callables.push(Callable::Builtin(id));
        self.scopes[0]
            .vars
            .insert(global_name.to_string(), JsValue::Fn(cid));
    }

    fn alloc_obj_map(&mut self, map: HashMap<String, JsValue>) -> u32 {
        let id = self.objects.len() as u32;
        self.objects.push(map);
        id
    }

    fn alloc_obj(&mut self) -> Result<u32, &'static str> {
        if self.objects.len() >= MAX_VM_HEAP_CONTAINERS {
            return Err("JS heap object limit reached");
        }
        let id = self.objects.len() as u32;
        self.objects.push(HashMap::new());
        Ok(id)
    }

    fn alloc_arr(&mut self, items: Vec<JsValue>) -> Result<u32, &'static str> {
        if self.arrays.len() >= MAX_VM_HEAP_CONTAINERS {
            return Err("JS heap array limit reached");
        }
        let id = self.arrays.len() as u32;
        self.arrays.push(items);
        Ok(id)
    }

    fn lookup_var(&self, mut scope_id: u32, name: &str) -> JsValue {
        loop {
            let Some(frame) = self.scopes.get(scope_id as usize) else {
                return JsValue::Undefined;
            };
            if let Some(val) = frame.vars.get(name) {
                return val.clone();
            }
            match frame.parent {
                Some(p) => scope_id = p,
                None => return JsValue::Undefined,
            }
        }
    }

    fn store_var(&mut self, mut scope_id: u32, name: &str, val: JsValue) {
        loop {
            if let Some(frame) = self.scopes.get_mut(scope_id as usize) {
                if frame.vars.contains_key(name) {
                    frame.vars.insert(name.to_string(), val);
                    return;
                }
                if let Some(p) = frame.parent {
                    scope_id = p;
                    continue;
                }
            }
            break;
        }
        if let Some(global) = self.scopes.get_mut(0) {
            global.vars.insert(name.to_string(), val);
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
        self.fuel_remaining = self.max_fuel;
        let res = self.run_bytecode_at(
            &program,
            0,
            0,
            origin_url,
            dom,
            clipboard,
            storage,
            downloads,
        )?;

        // Drain scheduled Promises & microtasks within remaining fuel
        let mut tasks_run = 0usize;
        while let Some(task) = self.microtasks.pop_front() {
            if tasks_run >= 1024 {
                break;
            }
            tasks_run += 1;
            let _ = self.invoke_callable_val(
                &program,
                &task.cb,
                &[task.arg],
                origin_url,
                dom,
                clipboard,
                storage,
                downloads,
            );
        }

        Ok(res)
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_callable_val(
        &mut self,
        program: &CompiledJsProgram,
        callee: &JsValue,
        args: &[JsValue],
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        let JsValue::Fn(cid) = *callee else {
            return Ok(JsValue::Undefined);
        };
        let Some(c) = self.callables.get(cid as usize).cloned() else {
            return Ok(JsValue::Undefined);
        };
        match c {
            Callable::Builtin(name) => Ok(self.invoke_builtin_fn(name, args)),
            Callable::UserClosure { fn_idx, env_scope } => {
                let def = program
                    .functions
                    .get(fn_idx as usize)
                    .ok_or("Missing function def")?;
                if self.scopes.len() >= MAX_VM_SCOPES {
                    return Err("JS scope memory limit exceeded");
                }
                let mut vars = HashMap::with_capacity(def.params.len());
                for (i, &pid) in def.params.iter().enumerate() {
                    let pname = program
                        .identifiers
                        .get(pid as usize)
                        .cloned()
                        .unwrap_or_default();
                    vars.insert(pname, args.get(i).cloned().unwrap_or(JsValue::Undefined));
                }
                let new_scope = self.scopes.len() as u32;
                self.scopes.push(ScopeFrame {
                    parent: Some(env_scope),
                    vars,
                });
                self.run_bytecode_at(
                    program,
                    def.entry_pc,
                    new_scope,
                    origin_url,
                    dom,
                    clipboard,
                    storage,
                    downloads,
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bytecode_at(
        &mut self,
        program: &CompiledJsProgram,
        start_pc: usize,
        initial_scope: u32,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        let mut stack: Vec<JsValue> = Vec::with_capacity(64);
        let mut call_frames: Vec<CallFrame> = Vec::with_capacity(16);
        let mut try_stack: Vec<TryHandler> = Vec::with_capacity(4);
        let mut current_scope = initial_scope;
        let mut last_popped = JsValue::Undefined;
        let mut pc = start_pc;

        while pc < program.instructions.len() {
            if self.fuel_remaining == 0 {
                return Err("JS execution terminated: instruction fuel limit exceeded");
            }
            self.fuel_remaining -= 1;

            match &program.instructions[pc] {
                OpCode::LoadConst(idx) => {
                    stack.push(
                        program
                            .constants
                            .get(*idx as usize)
                            .cloned()
                            .unwrap_or(JsValue::Undefined),
                    );
                    pc += 1;
                }
                OpCode::LoadIdent(idx) => {
                    let name = program
                        .identifiers
                        .get(*idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    stack.push(self.lookup_var(current_scope, name));
                    pc += 1;
                }
                OpCode::DeclVar(idx) => {
                    let val = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(name) = program.identifiers.get(*idx as usize) {
                        if let Some(frame) = self.scopes.get_mut(current_scope as usize) {
                            frame.vars.insert(name.clone(), val);
                        }
                    }
                    pc += 1;
                }
                OpCode::StoreVar(idx) => {
                    let val = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    if let Some(name) = program.identifiers.get(*idx as usize) {
                        self.store_var(current_scope, name, val);
                    }
                    pc += 1;
                }
                OpCode::MakeArray(count) => {
                    let n = *count as usize;
                    let start = stack.len().saturating_sub(n);
                    let items: Vec<JsValue> = stack.drain(start..).collect();
                    let aid = self.alloc_arr(items)?;
                    stack.push(JsValue::Array(aid));
                    pc += 1;
                }
                OpCode::MakeObject(pairs) => {
                    let n = (*pairs as usize) * 2;
                    let start = stack.len().saturating_sub(n);
                    let flat: Vec<JsValue> = stack.drain(start..).collect();
                    let mut map = HashMap::with_capacity(flat.len() / 2);
                    for pair in flat.chunks_exact(2) {
                        map.insert(pair[0].as_str(), pair[1].clone());
                    }
                    let oid = self.alloc_obj_map(map);
                    stack.push(JsValue::Object(oid));
                    pc += 1;
                }
                OpCode::MakeClosure(fn_idx) => {
                    let cid = self.callables.len() as u32;
                    self.callables.push(Callable::UserClosure {
                        fn_idx: *fn_idx,
                        env_scope: current_scope,
                    });
                    stack.push(JsValue::Fn(cid));
                    pc += 1;
                }
                OpCode::GetProp(prop_id) => {
                    let obj_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let prop = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let res = self.get_property(&obj_val, prop, origin_url, dom, storage);
                    stack.push(res);
                    pc += 1;
                }
                OpCode::SetProp(prop_id) => {
                    let val = stack.pop().unwrap_or(JsValue::Undefined);
                    let obj_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let prop = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    self.set_property(&obj_val, prop, val.clone(), origin_url, dom, storage);
                    stack.push(val);
                    pc += 1;
                }
                OpCode::GetIndex => {
                    let idx_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let target = stack.pop().unwrap_or(JsValue::Undefined);
                    let res = match target {
                        JsValue::Array(aid) => {
                            let i = idx_val.as_number();
                            if i >= 0.0 && i.is_finite() {
                                self.arrays
                                    .get(aid as usize)
                                    .and_then(|a| a.get(i as usize))
                                    .cloned()
                                    .unwrap_or(JsValue::Undefined)
                            } else {
                                JsValue::Undefined
                            }
                        }
                        JsValue::Object(oid) => self
                            .objects
                            .get(oid as usize)
                            .and_then(|o| o.get(&idx_val.as_str()))
                            .cloned()
                            .unwrap_or(JsValue::Undefined),
                        JsValue::Str(s) => {
                            let i = idx_val.as_number() as usize;
                            s.chars()
                                .nth(i)
                                .map(|c| JsValue::Str(c.to_string()))
                                .unwrap_or(JsValue::Undefined)
                        }
                        _ => JsValue::Undefined,
                    };
                    stack.push(res);
                    pc += 1;
                }
                OpCode::SetIndex => {
                    let val = stack.pop().unwrap_or(JsValue::Undefined);
                    let idx_val = stack.pop().unwrap_or(JsValue::Undefined);
                    let target = stack.pop().unwrap_or(JsValue::Undefined);
                    match target {
                        JsValue::Array(aid) => {
                            let idx = idx_val.as_number() as usize;
                            if let Some(arr) = self.arrays.get_mut(aid as usize) {
                                if idx < 4096 {
                                    while arr.len() <= idx {
                                        arr.push(JsValue::Undefined);
                                    }
                                    arr[idx] = val.clone();
                                }
                            }
                        }
                        JsValue::Object(oid) => {
                            if let Some(obj) = self.objects.get_mut(oid as usize) {
                                obj.insert(idx_val.as_str(), val.clone());
                            }
                        }
                        _ => {}
                    }
                    stack.push(val);
                    pc += 1;
                }
                OpCode::Add => {
                    let r = stack.pop().unwrap_or(JsValue::Undefined);
                    let l = stack.pop().unwrap_or(JsValue::Undefined);
                    let res = match (&l, &r) {
                        (JsValue::Str(_), _) | (_, JsValue::Str(_)) => {
                            JsValue::Str(format!("{}{}", l.as_str(), r.as_str()))
                        }
                        _ => JsValue::Number(l.as_number() + r.as_number()),
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
                OpCode::TypeOf => {
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Str(a.type_of().to_string()));
                    pc += 1;
                }
                OpCode::StrictEq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(a == b));
                    pc += 1;
                }
                OpCode::StrictNeq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(a != b));
                    pc += 1;
                }
                OpCode::Eq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(abstract_eq(&a, &b)));
                    pc += 1;
                }
                OpCode::Neq => {
                    let b = stack.pop().unwrap_or(JsValue::Undefined);
                    let a = stack.pop().unwrap_or(JsValue::Undefined);
                    stack.push(JsValue::Bool(!abstract_eq(&a, &b)));
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
                    pc = if !cond.is_truthy() { *target } else { pc + 1 };
                }
                OpCode::JumpIfTrue(target) => {
                    let cond = stack.pop().unwrap_or(JsValue::Undefined);
                    pc = if cond.is_truthy() { *target } else { pc + 1 };
                }
                OpCode::PushTry(catch_pc) => {
                    try_stack.push(TryHandler {
                        catch_pc: *catch_pc,
                        stack_len: stack.len(),
                        call_depth: call_frames.len(),
                        scope_id: current_scope,
                    });
                    pc += 1;
                }
                OpCode::PopTry => {
                    try_stack.pop();
                    pc += 1;
                }
                OpCode::Throw => {
                    let err_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(handler) = try_stack.pop() {
                        stack.truncate(handler.stack_len);
                        call_frames.truncate(handler.call_depth);
                        current_scope = handler.scope_id;
                        stack.push(err_val);
                        pc = handler.catch_pc;
                    } else {
                        return Err("Uncaught JS exception");
                    }
                }
                OpCode::CallValue { argc } => {
                    let n = *argc as usize;
                    let start = stack.len().saturating_sub(n);
                    let args: Vec<JsValue> = stack.drain(start..).collect();
                    let callee = stack.pop().unwrap_or(JsValue::Undefined);
                    if let JsValue::Fn(cid) = callee {
                        match self.callables.get(cid as usize).cloned() {
                            Some(Callable::UserClosure { fn_idx, env_scope }) => {
                                if call_frames.len() >= 256 {
                                    return Err("JS call stack overflow");
                                }
                                if self.scopes.len() >= MAX_VM_SCOPES {
                                    return Err("JS scope memory limit exceeded");
                                }
                                let def = program
                                    .functions
                                    .get(fn_idx as usize)
                                    .ok_or("Missing function def")?;
                                let mut vars = HashMap::with_capacity(def.params.len());
                                for (i, &pid) in def.params.iter().enumerate() {
                                    let pname = program
                                        .identifiers
                                        .get(pid as usize)
                                        .cloned()
                                        .unwrap_or_default();
                                    vars.insert(
                                        pname,
                                        args.get(i).cloned().unwrap_or(JsValue::Undefined),
                                    );
                                }
                                let new_scope = self.scopes.len() as u32;
                                self.scopes.push(ScopeFrame {
                                    parent: Some(env_scope),
                                    vars,
                                });
                                call_frames.push(CallFrame {
                                    return_pc: pc + 1,
                                    prev_scope: current_scope,
                                    try_depth: try_stack.len(),
                                });
                                current_scope = new_scope;
                                pc = def.entry_pc;
                                continue;
                            }
                            Some(Callable::Builtin(bname)) => {
                                let ret = self.invoke_builtin_fn(bname, &args);
                                stack.push(ret);
                            }
                            None => stack.push(JsValue::Undefined),
                        }
                    } else {
                        stack.push(JsValue::Undefined);
                    }
                    pc += 1;
                }
                OpCode::CallMethod { prop_id, argc } => {
                    let n = *argc as usize;
                    let start = stack.len().saturating_sub(n);
                    let args: Vec<JsValue> = stack.drain(start..).collect();
                    let receiver = stack.pop().unwrap_or(JsValue::Undefined);
                    let method_name = program
                        .identifiers
                        .get(*prop_id as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let ret = self.invoke_method(
                        program,
                        &receiver,
                        method_name,
                        &args,
                        origin_url,
                        dom,
                        clipboard,
                        storage,
                        downloads,
                    )?;
                    stack.push(ret);
                    pc += 1;
                }
                OpCode::Return => {
                    let ret_val = stack.pop().unwrap_or(JsValue::Undefined);
                    if let Some(frame) = call_frames.pop() {
                        try_stack.truncate(frame.try_depth);
                        current_scope = frame.prev_scope;
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
                OpCode::Dup => {
                    let top = stack.last().cloned().unwrap_or(JsValue::Undefined);
                    stack.push(top);
                    pc += 1;
                }
                OpCode::Halt => break,
            }
        }

        Ok(stack.pop().unwrap_or(last_popped))
    }

    fn get_property(
        &mut self,
        obj: &JsValue,
        prop: &str,
        origin_url: &str,
        dom: &ZeroCopyDomArena,
        storage: &MantaStorageEngine,
    ) -> JsValue {
        match obj {
            JsValue::Str(s) => match prop {
                "length" => JsValue::Number(s.chars().count() as f64),
                _ => JsValue::Undefined,
            },
            JsValue::Array(aid) => match prop {
                "length" => JsValue::Number(
                    self.arrays.get(*aid as usize).map(|a| a.len()).unwrap_or(0) as f64,
                ),
                _ => JsValue::Undefined,
            },
            JsValue::DomNode(nid) => match prop {
                "id" => dom
                    .node_id_attr(*nid)
                    .map(|s| JsValue::Str(s.to_string()))
                    .unwrap_or_else(|| JsValue::Str(String::new())),
                "tagName" => dom
                    .node_tag(*nid)
                    .map(|t| JsValue::Str(t.to_ascii_uppercase()))
                    .unwrap_or_else(|| JsValue::Str(String::new())),
                "textContent" | "innerText" | "innerHTML" => JsValue::Str(dom.text_content(*nid)),
                "value" => dom
                    .forms
                    .get_control(*nid)
                    .map(|c| JsValue::Str(c.value.clone()))
                    .unwrap_or_else(|| JsValue::Str(dom.get_attribute(*nid, "value").unwrap_or_default())),
                "checked" => JsValue::Bool(
                    dom.forms
                        .get_control(*nid)
                        .map(|c| c.checked)
                        .unwrap_or(false),
                ),
                "classList" => {
                    let mut m = HashMap::new();
                    m.insert("__manta_ns".to_string(), JsValue::Str("classList".to_string()));
                    m.insert("__nid".to_string(), JsValue::DomNode(*nid));
                    JsValue::Object(self.alloc_obj_map(m))
                }
                "style" => {
                    let mut m = HashMap::new();
                    m.insert("__manta_ns".to_string(), JsValue::Str("dom_style".to_string()));
                    m.insert("__nid".to_string(), JsValue::DomNode(*nid));
                    JsValue::Object(self.alloc_obj_map(m))
                }
                _ => dom
                    .get_attribute(*nid, prop)
                    .map(JsValue::Str)
                    .unwrap_or(JsValue::Undefined),
            },
            JsValue::Object(oid) => {
                if let Some(ns) = self
                    .objects
                    .get(*oid as usize)
                    .and_then(|m| m.get("__manta_ns"))
                    .map(|v| v.as_str())
                {
                    match (ns.as_str(), prop) {
                        ("document", "title") => return JsValue::Str(dom.page_title.clone()),
                        ("document", "cookie") => {
                            return JsValue::Str(storage.get_cookie_header_for_url(origin_url, false));
                        }
                        ("location", "href") => return JsValue::Str(origin_url.to_string()),
                        ("window", "location") => {
                            return self.scopes[0]
                                .vars
                                .get("location")
                                .cloned()
                                .unwrap_or(JsValue::Undefined);
                        }
                        ("Math", "PI") => return JsValue::Number(std::f64::consts::PI),
                        ("Math", "E") => return JsValue::Number(std::f64::consts::E),
                        _ => {}
                    }
                }
                self.objects
                    .get(*oid as usize)
                    .and_then(|m| m.get(prop))
                    .cloned()
                    .unwrap_or(JsValue::Undefined)
            }
            _ => JsValue::Undefined,
        }
    }

    fn set_property(
        &mut self,
        obj: &JsValue,
        prop: &str,
        val: JsValue,
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        storage: &mut MantaStorageEngine,
    ) {
        match obj {
            JsValue::DomNode(nid) => match prop {
                "textContent" | "innerText" | "innerHTML" => {
                    dom.set_text_content(*nid, val.as_str());
                }
                "value" => {
                    let s = val.as_str();
                    let _ = dom.forms.set_value(*nid, &s);
                    let key = dom
                        .node_id_attr(*nid)
                        .map(|id| id.to_string())
                        .or_else(|| dom.forms.get_control(*nid).map(|c| c.name.clone()))
                        .unwrap_or_else(|| format!("node_{}", nid));
                    storage.save_form_draft(origin_url, &key, &s);
                }
                "id" => {
                    dom.set_attribute(*nid, "id", &val.as_str());
                }
                _ => {
                    dom.set_attribute(*nid, prop, &val.as_str());
                }
            },
            JsValue::Object(oid) => {
                if let Some(ns) = self
                    .objects
                    .get(*oid as usize)
                    .and_then(|m| m.get("__manta_ns"))
                    .map(|v| v.as_str())
                {
                    match (ns.as_str(), prop) {
                        ("document", "title") => {
                            dom.page_title = val.as_str();
                            return;
                        }
                        ("document", "cookie") => {
                            storage.set_cookie_from_header(origin_url, &val.as_str());
                            return;
                        }
                        ("dom_style", css_prop) => {
                            if let Some(JsValue::DomNode(nid)) =
                                self.objects.get(*oid as usize).and_then(|m| m.get("__nid")).copied()
                            {
                                let prev = dom.get_attribute(nid, "style").unwrap_or_default();
                                let updated = if prev.is_empty() {
                                    format!("{}: {};", css_prop, val.as_str())
                                } else {
                                    format!("{}; {}: {};", prev.trim_end_matches(';'), css_prop, val.as_str())
                                };
                                dom.set_attribute(nid, "style", &updated);
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                if let Some(m) = self.objects.get_mut(*oid as usize) {
                    m.insert(prop.to_string(), val);
                }
            }
            _ => {}
        }
    }

    fn invoke_builtin_fn(&mut self, name: &str, args: &[JsValue]) -> JsValue {
        match name {
            "parseInt" => {
                let s = args.first().map(|a| a.as_str()).unwrap_or_default();
                let t = s.trim();
                let radix = args.get(1).map(|a| a.as_number() as u32).unwrap_or(10);
                if radix == 16 || t.starts_with("0x") || t.starts_with("0X") {
                    let hex = t.trim_start_matches("0x").trim_start_matches("0X");
                    JsValue::Number(i64::from_str_radix(hex, 16).map(|n| n as f64).unwrap_or(f64::NAN))
                } else {
                    JsValue::Number(t.parse::<i64>().map(|n| n as f64).unwrap_or(f64::NAN))
                }
            }
            "parseFloat" => JsValue::Number(args.first().map(|a| a.as_number()).unwrap_or(f64::NAN)),
            "isNaN" => JsValue::Bool(args.first().map(|a| a.as_number().is_nan()).unwrap_or(true)),
            "Number" => JsValue::Number(args.first().map(|a| a.as_number()).unwrap_or(0.0)),
            "String" => JsValue::Str(args.first().map(|a| a.as_str()).unwrap_or_default()),
            "Boolean" => JsValue::Bool(args.first().map(|a| a.is_truthy()).unwrap_or(false)),
            "queueMicrotask" | "setTimeout" => {
                if let Some(cb) = args.first().cloned() {
                    if self.microtasks.len() < 1024 {
                        self.microtasks.push_back(Microtask {
                            cb,
                            arg: JsValue::Undefined,
                        });
                    }
                }
                JsValue::Number(1.0)
            }
            "clearTimeout" => JsValue::Undefined,
            _ => JsValue::Undefined,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_method(
        &mut self,
        program: &CompiledJsProgram,
        receiver: &JsValue,
        method: &str,
        args: &[JsValue],
        origin_url: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<JsValue, &'static str> {
        match receiver {
            JsValue::Str(s) => match method {
                "trim" => Ok(JsValue::Str(s.trim().to_string())),
                "toLowerCase" => Ok(JsValue::Str(s.to_ascii_lowercase())),
                "toUpperCase" => Ok(JsValue::Str(s.to_ascii_uppercase())),
                "includes" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Bool(s.contains(&sub)))
                }
                "startsWith" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Bool(s.starts_with(&sub)))
                }
                "endsWith" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Bool(s.ends_with(&sub)))
                }
                "indexOf" => {
                    let sub = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Number(
                        s.find(&sub).map(|i| i as f64).unwrap_or(-1.0),
                    ))
                }
                "replace" => {
                    let pat = args.first().map(|a| a.as_str()).unwrap_or_default();
                    let rep = args.get(1).map(|a| a.as_str()).unwrap_or_default();
                    Ok(JsValue::Str(s.replacen(&pat, &rep, 1)))
                }
                "split" => {
                    let sep = args.first().map(|a| a.as_str()).unwrap_or_default();
                    let items: Vec<JsValue> = if sep.is_empty() {
                        s.chars().map(|c| JsValue::Str(c.to_string())).collect()
                    } else {
                        s.split(&sep).map(|p| JsValue::Str(p.to_string())).collect()
                    };
                    Ok(JsValue::Array(self.alloc_arr(items)?))
                }
                "slice" | "substring" => {
                    let len = s.chars().count();
                    let start = args.first().map(|a| a.as_number() as isize).unwrap_or(0);
                    let end = args
                        .get(1)
                        .map(|a| a.as_number() as isize)
                        .unwrap_or(len as isize);
                    let st = if start < 0 {
                        (len as isize + start).max(0) as usize
                    } else {
                        (start as usize).min(len)
                    };
                    let en = if end < 0 {
                        (len as isize + end).max(0) as usize
                    } else {
                        (end as usize).min(len)
                    };
                    let sliced: String = if st < en {
                        s.chars().skip(st).take(en - st).collect()
                    } else {
                        String::new()
                    };
                    Ok(JsValue::Str(sliced))
                }
                _ => Ok(JsValue::Undefined),
            },
            JsValue::Array(aid) => {
                let items_snapshot = self.arrays.get(*aid as usize).cloned().unwrap_or_default();
                match method {
                    "push" => {
                        let new_len = if let Some(arr) = self.arrays.get_mut(*aid as usize) {
                            arr.extend_from_slice(args);
                            arr.len()
                        } else {
                            0
                        };
                        Ok(JsValue::Number(new_len as f64))
                    }
                    "pop" => Ok(self
                        .arrays
                        .get_mut(*aid as usize)
                        .and_then(|a| a.pop())
                        .unwrap_or(JsValue::Undefined)),
                    "shift" => {
                        if let Some(arr) = self.arrays.get_mut(*aid as usize) {
                            if !arr.is_empty() {
                                return Ok(arr.remove(0));
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    "join" => {
                        let sep = args
                            .first()
                            .map(|a| a.as_str())
                            .unwrap_or_else(|| ",".to_string());
                        let joined: Vec<String> = items_snapshot.iter().map(|v| v.as_str()).collect();
                        Ok(JsValue::Str(joined.join(&sep)))
                    }
                    "includes" => {
                        let target = args.first().cloned().unwrap_or(JsValue::Undefined);
                        Ok(JsValue::Bool(items_snapshot.contains(&target)))
                    }
                    "indexOf" => {
                        let target = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let pos = items_snapshot
                            .iter()
                            .position(|x| x == &target)
                            .map(|i| i as f64)
                            .unwrap_or(-1.0);
                        Ok(JsValue::Number(pos))
                    }
                    "reverse" => {
                        if let Some(arr) = self.arrays.get_mut(*aid as usize) {
                            arr.reverse();
                        }
                        Ok(JsValue::Array(*aid))
                    }
                    "slice" => {
                        let len = items_snapshot.len();
                        let s = args.first().map(|a| a.as_number() as isize).unwrap_or(0);
                        let e = args.get(1).map(|a| a.as_number() as isize).unwrap_or(len as isize);
                        let st = if s < 0 { (len as isize + s).max(0) as usize } else { (s as usize).min(len) };
                        let en = if e < 0 { (len as isize + e).max(0) as usize } else { (e as usize).min(len) };
                        let sliced = if st < en { items_snapshot[st..en].to_vec() } else { Vec::new() };
                        Ok(JsValue::Array(self.alloc_arr(sliced)?))
                    }
                    "map" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let mut out = Vec::with_capacity(items_snapshot.len());
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            let mapped = self.invoke_callable_val(
                                program,
                                &cb,
                                &[item, JsValue::Number(idx as f64)],
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            out.push(mapped);
                        }
                        Ok(JsValue::Array(self.alloc_arr(out)?))
                    }
                    "filter" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let mut out = Vec::new();
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            let keep = self.invoke_callable_val(
                                program,
                                &cb,
                                &[item.clone(), JsValue::Number(idx as f64)],
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            if keep.is_truthy() {
                                out.push(item);
                            }
                        }
                        Ok(JsValue::Array(self.alloc_arr(out)?))
                    }
                    "forEach" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            self.invoke_callable_val(
                                program,
                                &cb,
                                &[item, JsValue::Number(idx as f64)],
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                        }
                        Ok(JsValue::Undefined)
                    }
                    "find" => {
                        let cb = args.first().cloned().unwrap_or(JsValue::Undefined);
                        for (idx, item) in items_snapshot.into_iter().enumerate() {
                            let matched = self.invoke_callable_val(
                                program,
                                &cb,
                                &[item.clone(), JsValue::Number(idx as f64)],
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )?;
                            if matched.is_truthy() {
                                return Ok(item);
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    _ => Ok(JsValue::Undefined),
                }
            }
            JsValue::DomNode(nid) => match method {
                "getAttribute" => {
                    let k = args.first().map(|a| a.as_str()).unwrap_or_default();
                    Ok(dom
                        .get_attribute(*nid, &k)
                        .map(JsValue::Str)
                        .unwrap_or(JsValue::Null))
                }
                "setAttribute" => {
                    if args.len() >= 2 {
                        dom.set_attribute(*nid, &args[0].as_str(), &args[1].as_str());
                    }
                    Ok(JsValue::Undefined)
                }
                "removeAttribute" => {
                    if let Some(k) = args.first() {
                        dom.remove_attribute(*nid, &k.as_str());
                    }
                    Ok(JsValue::Undefined)
                }
                "appendChild" => {
                    if let Some(JsValue::DomNode(child_id)) = args.first() {
                        dom.append_child(*nid, *child_id);
                        return Ok(JsValue::DomNode(*child_id));
                    }
                    Ok(JsValue::Undefined)
                }
                "removeChild" => {
                    if let Some(JsValue::DomNode(child_id)) = args.first() {
                        dom.remove_child(*nid, *child_id);
                        return Ok(JsValue::DomNode(*child_id));
                    }
                    Ok(JsValue::Undefined)
                }
                "addEventListener" => {
                    if args.len() >= 2 {
                        let ev = args[0].as_str().to_ascii_lowercase();
                        match &args[1] {
                            JsValue::Fn(cid) => {
                                dom.add_event_listener(*nid, &ev, &format!("__manta_fn:{}", cid));
                            }
                            JsValue::Str(code) => {
                                dom.add_event_listener(*nid, &ev, code);
                            }
                            _ => {}
                        }
                    }
                    Ok(JsValue::Undefined)
                }
                "dispatchEvent" => {
                    let ev_type = args
                        .first()
                        .map(|a| a.as_str())
                        .unwrap_or_else(|| "click".to_string());
                    for target in dom.event_bubbling_path(*nid) {
                        for handler in dom.get_event_listeners(target, &ev_type) {
                            if let Some(cid_str) = handler.strip_prefix("__manta_fn:") {
                                if let Ok(cid) = cid_str.parse::<u32>() {
                                    let _ = self.invoke_callable_val(
                                        program,
                                        &JsValue::Fn(cid),
                                        &[JsValue::DomNode(*nid)],
                                        origin_url,
                                        dom,
                                        clipboard,
                                        storage,
                                        downloads,
                                    )?;
                                }
                            } else if let Ok(inline_prog) = JsBytecodeCompiler::compile(&handler) {
                                let _ = self.run_bytecode_at(
                                    &inline_prog,
                                    0,
                                    0,
                                    origin_url,
                                    dom,
                                    clipboard,
                                    storage,
                                    downloads,
                                );
                            }
                        }
                    }
                    Ok(JsValue::Bool(true))
                }
                "focus" => {
                    dom.forms.focus_control(*nid);
                    Ok(JsValue::Undefined)
                }
                _ => Ok(JsValue::Undefined),
            },
            JsValue::Object(oid) => {
                let ns = self
                    .objects
                    .get(*oid as usize)
                    .and_then(|m| m.get("__manta_ns"))
                    .map(|v| v.as_str())
                    .unwrap_or_default();

                match (ns.as_str(), method) {
                    ("classList", "add") => {
                        if let Some(JsValue::DomNode(nid)) = self
                            .objects
                            .get(*oid as usize)
                            .and_then(|m| m.get("__nid"))
                            .copied()
                        {
                            for a in args {
                                dom.add_class(nid, &a.as_str());
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("classList", "remove") => {
                        if let Some(JsValue::DomNode(nid)) = self
                            .objects
                            .get(*oid as usize)
                            .and_then(|m| m.get("__nid"))
                            .copied()
                        {
                            for a in args {
                                dom.remove_class(nid, &a.as_str());
                            }
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("classList", "contains") => {
                        if let Some(JsValue::DomNode(nid)) = self
                            .objects
                            .get(*oid as usize)
                            .and_then(|m| m.get("__nid"))
                            .copied()
                        {
                            let cls = args.first().map(|a| a.as_str()).unwrap_or_default();
                            return Ok(JsValue::Bool(dom.node_has_class(nid, &cls)));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("classList", "toggle") => {
                        if let Some(JsValue::DomNode(nid)) = self
                            .objects
                            .get(*oid as usize)
                            .and_then(|m| m.get("__nid"))
                            .copied()
                        {
                            let cls = args.first().map(|a| a.as_str()).unwrap_or_default();
                            if dom.node_has_class(nid, &cls) {
                                dom.remove_class(nid, &cls);
                                return Ok(JsValue::Bool(false));
                            } else {
                                dom.add_class(nid, &cls);
                                return Ok(JsValue::Bool(true));
                            }
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("document", "getElementById") => {
                        let id = args.first().map(|a| a.as_str()).unwrap_or_default();
                        Ok(dom
                            .find_by_id(&id)
                            .map(JsValue::DomNode)
                            .unwrap_or(JsValue::Null))
                    }
                    ("document", "querySelector") => {
                        let sel = args.first().map(|a| a.as_str()).unwrap_or_default();
                        Ok(dom
                            .query_selector(&sel)
                            .map(JsValue::DomNode)
                            .unwrap_or(JsValue::Null))
                    }
                    ("document", "querySelectorAll") => {
                        let sel = args.first().map(|a| a.as_str()).unwrap_or_default();
                        let nodes: Vec<JsValue> = dom
                            .query_selector_all(&sel)
                            .into_iter()
                            .map(JsValue::DomNode)
                            .collect();
                        Ok(JsValue::Array(self.alloc_arr(nodes)?))
                    }
                    ("document", "createElement") => {
                        let tag = args
                            .first()
                            .map(|a| a.as_str())
                            .unwrap_or_else(|| "div".to_string());
                        Ok(JsValue::DomNode(dom.create_element(&tag)))
                    }
                    ("localStorage", "setItem") => {
                        if args.len() >= 2 {
                            storage.set_local_storage(origin_url, &args[0].as_str(), &args[1].as_str());
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("localStorage", "getItem") => {
                        if let Some(k) = args.first() {
                            return Ok(storage
                                .get_local_storage(origin_url, &k.as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("localStorage", "removeItem") => {
                        if let Some(k) = args.first() {
                            storage.remove_local_storage(origin_url, &k.as_str());
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("localStorage", "clear") => {
                        storage.clear_local_storage_only(origin_url);
                        Ok(JsValue::Undefined)
                    }
                    ("sessionStorage", "setItem") => {
                        if args.len() >= 2 {
                            storage.set_session_storage(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                            );
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("sessionStorage", "getItem") => {
                        if let Some(k) = args.first() {
                            return Ok(storage
                                .get_session_storage(origin_url, &k.as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("sessionStorage", "removeItem") => {
                        if let Some(k) = args.first() {
                            storage.remove_session_storage(origin_url, &k.as_str());
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("sessionStorage", "clear") => {
                        storage.clear_session_storage_only(origin_url);
                        Ok(JsValue::Undefined)
                    }
                    ("MantaForm", "saveDraft") => {
                        if args.len() >= 2 {
                            return Ok(JsValue::Bool(storage.save_form_draft(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                            )));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaForm", "getDraft") => {
                        if let Some(k) = args.first() {
                            return Ok(storage
                                .get_form_draft(origin_url, &k.as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("MantaDB" | "indexedDB", "putObject" | "put") => {
                        if args.len() >= 3 {
                            let ok = storage.put_idb_object(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                                &args[2].as_str(),
                            );
                            return Ok(JsValue::Bool(ok));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaDB" | "indexedDB", "getObject" | "get") => {
                        if args.len() >= 2 {
                            return Ok(storage
                                .db
                                .get_object(origin_url, &args[0].as_str(), &args[1].as_str())
                                .map(JsValue::Str)
                                .unwrap_or(JsValue::Null));
                        }
                        Ok(JsValue::Null)
                    }
                    ("MantaDB" | "indexedDB", "deleteObject" | "delete") => {
                        if args.len() >= 2 {
                            return Ok(JsValue::Bool(storage.delete_idb_object(
                                origin_url,
                                &args[0].as_str(),
                                &args[1].as_str(),
                            )));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaFS", "writeFile") => {
                        if args.len() >= 2 {
                            let mime = args
                                .get(2)
                                .map(|a| a.as_str())
                                .unwrap_or_else(|| "text/plain".to_string());
                            let ok = storage.save_vfs_file(
                                origin_url,
                                &args[0].as_str(),
                                &mime,
                                args[1].as_str().as_bytes(),
                            );
                            return Ok(JsValue::Bool(ok));
                        }
                        Ok(JsValue::Bool(false))
                    }
                    ("MantaFS", "readFile") => {
                        if let Some(p) = args.first() {
                            if let Some(entry) = storage.db.read_virtual_file(origin_url, &p.as_str()) {
                                return Ok(JsValue::Str(
                                    String::from_utf8_lossy(&entry.data).into_owned(),
                                ));
                            }
                        }
                        Ok(JsValue::Null)
                    }
                    ("MantaDownloads", "save") => {
                        if args.len() >= 2 {
                            let mime = args
                                .get(2)
                                .map(|a| a.as_str())
                                .unwrap_or_else(|| "text/plain".to_string());
                            if let Ok(dl_id) = downloads.save_direct_bytes(
                                origin_url,
                                &args[0].as_str(),
                                &mime,
                                args[1].as_str().as_bytes(),
                            ) {
                                return Ok(JsValue::Number(dl_id as f64));
                            }
                        }
                        Ok(JsValue::Number(-1.0))
                    }
                    ("clipboard", "writeText") => {
                        if let Some(v) = args.first() {
                            clipboard.copy_text(&v.as_str());
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("clipboard", "readText") => Ok(JsValue::Str(clipboard.paste_text().to_string())),
                    ("console", "log" | "info" | "warn" | "error") => {
                        if self.console_logs.len() < 64 {
                            let joined: Vec<String> = args.iter().map(|a| a.as_str()).collect();
                            self.console_logs.push(joined.join(" "));
                        }
                        Ok(JsValue::Undefined)
                    }
                    ("Math", "max") => Ok(JsValue::Number(
                        args.iter()
                            .map(|a| a.as_number())
                            .fold(f64::NEG_INFINITY, f64::max),
                    )),
                    ("Math", "min") => Ok(JsValue::Number(
                        args.iter()
                            .map(|a| a.as_number())
                            .fold(f64::INFINITY, f64::min),
                    )),
                    ("Math", "floor") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).floor(),
                    )),
                    ("Math", "ceil") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).ceil(),
                    )),
                    ("Math", "round") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).round(),
                    )),
                    ("Math", "abs") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).abs(),
                    )),
                    ("Math", "sqrt") => Ok(JsValue::Number(
                        args.first().map(|a| a.as_number()).unwrap_or(0.0).sqrt(),
                    )),
                    ("JSON", "stringify") => {
                        let val = args.first().unwrap_or(&JsValue::Undefined);
                        Ok(JsValue::Str(self.json_stringify(val)))
                    }
                    ("JSON", "parse") => {
                        let raw = args.first().map(|a| a.as_str()).unwrap_or_default();
                        Ok(self.json_parse(&raw).unwrap_or(JsValue::Null))
                    }
                    ("Object", "keys") => {
                        if let Some(JsValue::Object(target_oid)) = args.first() {
                            let keys: Vec<JsValue> = self
                                .objects
                                .get(*target_oid as usize)
                                .map(|m| {
                                    m.keys()
                                        .filter(|k| !k.starts_with("__"))
                                        .map(|k| JsValue::Str(k.clone()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            return Ok(JsValue::Array(self.alloc_arr(keys)?));
                        }
                        Ok(JsValue::Array(self.alloc_arr(Vec::new())?))
                    }
                    ("Array", "isArray") => Ok(JsValue::Bool(matches!(
                        args.first(),
                        Some(JsValue::Array(_))
                    ))),
                    ("Promise", "resolve") => {
                        let val = args.first().cloned().unwrap_or(JsValue::Undefined);
                        let mut m = HashMap::new();
                        m.insert("__manta_ns".to_string(), JsValue::Str("promise".to_string()));
                        m.insert("__state".to_string(), JsValue::Str("fulfilled".to_string()));
                        m.insert("__val".to_string(), val);
                        Ok(JsValue::Object(self.alloc_obj_map(m)))
                    }
                    ("promise", "then") => {
                        let st = self
                            .objects
                            .get(*oid as usize)
                            .and_then(|m| m.get("__state"))
                            .map(|v| v.as_str())
                            .unwrap_or_default();
                        if st == "fulfilled" {
                            if let Some(cb) = args.first().cloned() {
                                let arg = self
                                    .objects
                                    .get(*oid as usize)
                                    .and_then(|m| m.get("__val"))
                                    .cloned()
                                    .unwrap_or(JsValue::Undefined);
                                if self.microtasks.len() < 1024 {
                                    self.microtasks.push_back(Microtask { cb, arg });
                                }
                            }
                        }
                        Ok(JsValue::Object(*oid))
                    }
                    ("Date", "now") => {
                        let ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as f64)
                            .unwrap_or(0.0);
                        Ok(JsValue::Number(ms))
                    }
                    _ => {
                        // User-defined method stored on JS object
                        let member = self
                            .objects
                            .get(*oid as usize)
                            .and_then(|m| m.get(method))
                            .cloned();
                        if let Some(fn_val) = member {
                            self.invoke_callable_val(
                                program,
                                &fn_val,
                                args,
                                origin_url,
                                dom,
                                clipboard,
                                storage,
                                downloads,
                            )
                        } else {
                            Ok(JsValue::Undefined)
                        }
                    }
                }
            }
            _ => Ok(JsValue::Undefined),
        }
    }

    fn json_stringify(&self, val: &JsValue) -> String {
        match val {
            JsValue::Null | JsValue::Undefined => "null".to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Number(n) => {
                if n.is_finite() {
                    val.as_str()
                } else {
                    "null".to_string()
                }
            }
            JsValue::Str(s) => format!("\"{}\"", escape_json_str(s)),
            JsValue::Array(aid) => {
                let Some(items) = self.arrays.get(*aid as usize) else {
                    return "[]".to_string();
                };
                let parts: Vec<String> = items.iter().map(|v| self.json_stringify(v)).collect();
                format!("[{}]", parts.join(","))
            }
            JsValue::Object(oid) => {
                let Some(map) = self.objects.get(*oid as usize) else {
                    return "{}".to_string();
                };
                let mut pairs = Vec::new();
                for (k, v) in map {
                    if !k.starts_with("__") {
                        pairs.push(format!("\"{}\":{}", escape_json_str(k), self.json_stringify(v)));
                    }
                }
                format!("{{{}}}", pairs.join(","))
            }
            _ => "{}".to_string(),
        }
    }

    fn json_parse(&mut self, input: &str) -> Result<JsValue, &'static str> {
        let mut cursor = 0usize;
        self.parse_json_val(input.as_bytes(), &mut cursor)
    }

    fn parse_json_val(&mut self, b: &[u8], cur: &mut usize) -> Result<JsValue, &'static str> {
        skip_ws(b, cur);
        if *cur >= b.len() {
            return Err("Empty JSON");
        }
        match b[*cur] {
            b'n' if b[*cur..].starts_with(b"null") => {
                *cur += 4;
                Ok(JsValue::Null)
            }
            b't' if b[*cur..].starts_with(b"true") => {
                *cur += 4;
                Ok(JsValue::Bool(true))
            }
            b'f' if b[*cur..].starts_with(b"false") => {
                *cur += 5;
                Ok(JsValue::Bool(false))
            }
            b'"' => Ok(JsValue::Str(parse_json_str(b, cur)?)),
            b'[' => {
                *cur += 1;
                let mut arr = Vec::new();
                skip_ws(b, cur);
                if *cur < b.len() && b[*cur] == b']' {
                    *cur += 1;
                    return Ok(JsValue::Array(self.alloc_arr(arr)?));
                }
                while *cur < b.len() {
                    arr.push(self.parse_json_val(b, cur)?);
                    skip_ws(b, cur);
                    if *cur < b.len() && b[*cur] == b',' {
                        *cur += 1;
                    } else if *cur < b.len() && b[*cur] == b']' {
                        *cur += 1;
                        break;
                    } else {
                        return Err("Malformed JSON array");
                    }
                }
                Ok(JsValue::Array(self.alloc_arr(arr)?))
            }
            b'{' => {
                *cur += 1;
                let oid = self.alloc_obj()?;
                skip_ws(b, cur);
                if *cur < b.len() && b[*cur] == b'}' {
                    *cur += 1;
                    return Ok(JsValue::Object(oid));
                }
                while *cur < b.len() {
                    skip_ws(b, cur);
                    let key = parse_json_str(b, cur)?;
                    skip_ws(b, cur);
                    if *cur >= b.len() || b[*cur] != b':' {
                        return Err("Expected ':' in JSON object");
                    }
                    *cur += 1;
                    let val = self.parse_json_val(b, cur)?;
                    if let Some(m) = self.objects.get_mut(oid as usize) {
                        m.insert(key, val);
                    }
                    skip_ws(b, cur);
                    if *cur < b.len() && b[*cur] == b',' {
                        *cur += 1;
                    } else if *cur < b.len() && b[*cur] == b'}' {
                        *cur += 1;
                        break;
                    } else {
                        return Err("Malformed JSON object");
                    }
                }
                Ok(JsValue::Object(oid))
            }
            _ => {
                let start = *cur;
                while *cur < b.len()
                    && (b[*cur].is_ascii_digit()
                        || matches!(b[*cur], b'-' | b'+' | b'.' | b'e' | b'E'))
                {
                    *cur += 1;
                }
                let s = std::str::from_utf8(&b[start..*cur]).map_err(|_| "Bad UTF8")?;
                let num = s.parse::<f64>().map_err(|_| "Bad JSON number")?;
                Ok(JsValue::Number(num))
            }
        }
    }

    pub fn get_global(&self, name: &str) -> Option<&JsValue> {
        self.scopes.first().and_then(|s| s.vars.get(name))
    }

    pub fn reset(&mut self) {
        self.scopes.clear();
        self.scopes.push(ScopeFrame {
            parent: None,
            vars: HashMap::with_capacity(32),
        });
        self.objects.clear();
        self.arrays.clear();
        self.callables.clear();
        self.microtasks.clear();
        self.console_logs.clear();
        self.fuel_remaining = self.max_fuel;
        self.install_default_globals();
    }
}

fn abstract_eq(a: &JsValue, b: &JsValue) -> bool {
    if std::mem::discriminant(a) == std::mem::discriminant(b) {
        return a == b;
    }
    match (a, b) {
        (JsValue::Null, JsValue::Undefined) | (JsValue::Undefined, JsValue::Null) => true,
        (JsValue::Number(n), JsValue::Str(_)) | (JsValue::Str(_), JsValue::Number(n)) => {
            *n == b.as_number() && *n == a.as_number()
        }
        (JsValue::Bool(_), _) | (_, JsValue::Bool(_)) => a.as_number() == b.as_number(),
        _ => false,
    }
}

fn skip_ws(b: &[u8], cur: &mut usize) {
    while *cur < b.len() && b[*cur].is_ascii_whitespace() {
        *cur += 1;
    }
}

fn parse_json_str(b: &[u8], cur: &mut usize) -> Result<String, &'static str> {
    if *cur >= b.len() || b[*cur] != b'"' {
        return Err("Expected JSON string quote");
    }
    *cur += 1;
    let mut s = String::new();
    while *cur < b.len() {
        let byte = b[*cur];
        if byte == b'"' {
            *cur += 1;
            return Ok(s);
        }
        if byte == b'\\' {
            *cur += 1;
            if *cur >= b.len() {
                return Err("Truncated JSON escape");
            }
            match b[*cur] {
                b'n' => s.push('\n'),
                b'r' => s.push('\r'),
                b't' => s.push('\t'),
                other => s.push(other as char),
            }
            *cur += 1;
        } else {
            let rest = std::str::from_utf8(&b[*cur..]).map_err(|_| "Bad UTF8")?;
            let ch = rest.chars().next().unwrap();
            s.push(ch);
            *cur += ch.len_utf8();
        }
    }
    Err("Unterminated JSON string")
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_js_vm_closures_objects_arrays_promises_json_and_dom_events() {
        let mut dom = ZeroCopyDomArena::new(128 * 1024);
        let mut tokenizer = StreamingHtmlTokenizer::new();
        tokenizer.feed_chunk(
            b"<div id=\"status\">Idle</div><input id=\"email\" type=\"email\" value=\"\" /><button id=\"btn\">Go</button>",
            true,
            |tok| dom.ingest_token(tok),
        );

        let mut vm = MantaJsSandbox::new(50_000);
        let mut clip = ClipboardSandbox::new(1024);
        let mut storage = MantaStorageEngine::new(128 * 1024, 512 * 1024);
        let mut downloads = MantaDownloadManager::new("", 1024 * 1024);

        let script = r#"
            function makeMultiplier(factor) {
                return (x) => x * factor;
            }
            let times6 = makeMultiplier(6);
            let arr = [1, 2, 3, 4];
            let sum = 0;
            for (let v of arr) {
                sum += v;
            }
            let result = times6(7) + sum - 2; // 42 + 10 - 2 = 50

            let obj = JSON.parse("{\"ok\":true,\"items\":[10,20]}");
            let caught = "";
            try {
                throw "boom";
            } catch (e) {
                caught = e;
            }

            let btn = document.getElementById("btn");
            let clicked = 0;
            btn.addEventListener("click", () => { clicked += 1; });
            btn.dispatchEvent("click");

            let asyncVal = 0;
            Promise.resolve(99).then((v) => { asyncVal = v; });

            document.getElementById("status").textContent = "Computed: " + result;
            document.getElementById("email").value = "dev@manta.browser";
            document.cookie = "session_id=manta_999; Path=/";
            MantaDB.putObject("users", "u1", JSON.stringify({ email: "dev@manta.browser" }));
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
        assert_eq!(vm.get_global("caught"), Some(&JsValue::Str("boom".to_string())));
        assert_eq!(vm.get_global("clicked"), Some(&JsValue::Number(1.0)));
        assert_eq!(vm.get_global("asyncVal"), Some(&JsValue::Number(99.0)));

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
        assert!(storage
            .db
            .get_object("https://app.manta.dev", "users", "u1")
            .unwrap()
            .contains("dev@manta.browser"));
        assert!(downloads.get_download(1).is_some());
    }
}
