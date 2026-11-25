use std::collections::HashSet;

use super::lexer::{JsLexer, Token};
use super::value::{CompiledJsProgram, JsFunctionDef, JsParam, JsValue, OpCode};

#[derive(Debug, Clone)]
enum LValue {
    Var(u16),
    Prop { prop_id: u16 },
    Index,
}

struct LoopPatchContext {
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
}

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
            let before = compiler.pos;
            compiler.compile_statement()?;
            if compiler.pos == before {
                compiler.advance();
            }
        }
        compiler.program.instructions.push(OpCode::Halt);
        compiler.optimize_program();
        Ok(compiler.program)
    }

    fn optimize_program(&mut self) {
        let mut jump_targets = HashSet::new();
        for op in &self.program.instructions {
            match *op {
                OpCode::Jump(t)
                | OpCode::JumpIfFalse(t)
                | OpCode::JumpIfTrue(t)
                | OpCode::PushTry(t) => {
                    jump_targets.insert(t);
                }
                _ => {}
            }
        }
        for f in &self.program.functions {
            jump_targets.insert(f.entry_pc);
            for p in &f.params {
                if let Some(d) = p.default_pc {
                    jump_targets.insert(d);
                }
            }
        }

        let mut changed = true;
        let mut passes = 0;
        while changed && passes < 6 {
            changed = false;
            passes += 1;
            let len = self.program.instructions.len();
            let mut i = 0usize;
            while i + 2 < len {
                if !jump_targets.contains(&(i + 1)) && !jump_targets.contains(&(i + 2)) {
                    let op0 = self.program.instructions[i].clone();
                    let op1 = self.program.instructions[i + 1].clone();
                    let op2 = self.program.instructions[i + 2].clone();
                    if let (OpCode::LoadConst(a_id), OpCode::LoadConst(b_id)) = (op0, op1) {
                        let a = self.program.constants[a_id as usize].clone();
                        let b = self.program.constants[b_id as usize].clone();
                        let folded = match (a, b, op2) {
                            (JsValue::Number(x), JsValue::Number(y), OpCode::Add) => {
                                Some(JsValue::Number(x + y))
                            }
                            (JsValue::Number(x), JsValue::Number(y), OpCode::Sub) => {
                                Some(JsValue::Number(x - y))
                            }
                            (JsValue::Number(x), JsValue::Number(y), OpCode::Mul) => {
                                Some(JsValue::Number(x * y))
                            }
                            (JsValue::Number(x), JsValue::Number(y), OpCode::Div) if y != 0.0 => {
                                Some(JsValue::Number(x / y))
                            }
                            (JsValue::Number(x), JsValue::Number(y), OpCode::Pow) => {
                                Some(JsValue::Number(x.powf(y)))
                            }
                            (JsValue::Str(x), JsValue::Str(y), OpCode::Add) => {
                                Some(JsValue::Str(format!("{}{}", x, y)))
                            }
                            _ => None,
                        };
                        if let Some(val) = folded {
                            let c_id = self.program.add_const(val);
                            self.program.instructions[i] = OpCode::LoadConst(c_id);
                            self.program.instructions[i + 1] = OpCode::Jump(i + 3);
                            self.program.instructions[i + 2] = OpCode::Pop;
                            jump_targets.insert(i + 3);
                            changed = true;
                            i += 3;
                            continue;
                        }
                    }
                }
                i += 1;
            }
        }
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
            Token::KeywordAsync if self.peek_at(1) == &Token::KeywordFunction => {
                self.advance();
                self.advance();
                let fn_name = if let Token::Ident(n) = self.peek().clone() {
                    self.advance();
                    Some(n)
                } else {
                    None
                };
                let fn_idx = self.compile_function_body(fn_name.clone(), true)?;
                self.program.instructions.push(OpCode::MakeClosure(fn_idx));
                if let Some(name) = fn_name {
                    let id = self.program.add_ident(&name);
                    self.program.instructions.push(OpCode::DeclVar(id));
                }
                self.program.instructions.push(OpCode::Pop);
                Ok(())
            }
            Token::KeywordFunction => {
                self.advance();
                let fn_name = if let Token::Ident(n) = self.peek().clone() {
                    self.advance();
                    Some(n)
                } else {
                    None
                };
                let fn_idx = self.compile_function_body(fn_name.clone(), false)?;
                self.program.instructions.push(OpCode::MakeClosure(fn_idx));
                if let Some(name) = fn_name {
                    let id = self.program.add_ident(&name);
                    self.program.instructions.push(OpCode::DeclVar(id));
                }
                self.program.instructions.push(OpCode::Pop);
                Ok(())
            }
            Token::KeywordClass => self.compile_class_declaration(),
            Token::LBrace => {
                self.advance();
                while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
                    let before = self.pos;
                    self.compile_statement()?;
                    if self.pos == before {
                        self.advance();
                    }
                }
                if self.peek() == &Token::RBrace {
                    self.advance();
                }
                Ok(())
            }
            Token::KeywordLet => {
                self.advance();
                loop {
                    if self.peek() == &Token::LBrace {
                        self.compile_object_destructure_decl()?;
                    } else if self.peek() == &Token::LBracket {
                        self.compile_array_destructure_decl()?;
                    } else {
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
                    }
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
            Token::KeywordDo => {
                self.advance();
                let body_start = self.program.instructions.len();
                self.loop_stack.push(LoopPatchContext {
                    break_jumps: Vec::new(),
                    continue_jumps: Vec::new(),
                });
                self.compile_statement()?;
                let ctx = self.loop_stack.pop().unwrap();
                let cond_pc = self.program.instructions.len();
                for cj in ctx.continue_jumps {
                    self.program.instructions[cj] = OpCode::Jump(cond_pc);
                }
                if self.peek() == &Token::KeywordWhile {
                    self.advance();
                }
                if self.peek() == &Token::LParen {
                    self.advance();
                }
                self.compile_expr()?;
                if self.peek() == &Token::RParen {
                    self.advance();
                }
                self.program.instructions.push(OpCode::JumpIfTrue(body_start));
                let loop_end = self.program.instructions.len();
                for bj in ctx.break_jumps {
                    self.program.instructions[bj] = OpCode::Jump(loop_end);
                }
                self.consume_optional_semicolons();
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
                if self.peek() == &Token::KeywordFinally {
                    self.advance();
                    self.compile_statement()?;
                }
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

    fn compile_object_destructure_decl(&mut self) -> Result<(), &'static str> {
        self.advance();
        let mut pairs = Vec::new();
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            let Token::Ident(prop_name) = self.advance() else {
                break;
            };
            let prop_id = self.program.add_ident(&prop_name);
            let target_id = if self.peek() == &Token::Colon {
                self.advance();
                if let Token::Ident(alias) = self.advance() {
                    self.program.add_ident(&alias)
                } else {
                    prop_id
                }
            } else {
                prop_id
            };
            let has_default = if self.peek() == &Token::Assign {
                self.advance();
                self.compile_expr()?;
                true
            } else {
                false
            };
            pairs.push((prop_id, target_id, has_default));
            if self.peek() == &Token::Comma {
                self.advance();
            }
        }
        if self.peek() == &Token::RBrace {
            self.advance();
        }
        if self.peek() == &Token::Assign {
            self.advance();
        }
        self.compile_expr()?;
        self.program
            .instructions
            .push(OpCode::DestructureObj { pairs });
        self.program.instructions.push(OpCode::Pop);
        Ok(())
    }

    fn compile_array_destructure_decl(&mut self) -> Result<(), &'static str> {
        self.advance();
        let mut targets = Vec::new();
        while self.peek() != &Token::RBracket && self.peek() != &Token::Eof {
            let is_rest = if self.peek() == &Token::Spread {
                self.advance();
                true
            } else {
                false
            };
            if let Token::Ident(vname) = self.advance() {
                targets.push((self.program.add_ident(&vname), is_rest));
            }
            if self.peek() == &Token::Comma {
                self.advance();
            }
        }
        if self.peek() == &Token::RBracket {
            self.advance();
        }
        if self.peek() == &Token::Assign {
            self.advance();
        }
        self.compile_expr()?;
        self.program
            .instructions
            .push(OpCode::DestructureArr { targets });
        self.program.instructions.push(OpCode::Pop);
        Ok(())
    }

    fn compile_class_declaration(&mut self) -> Result<(), &'static str> {
        self.advance();
        let Token::Ident(class_name) = self.advance() else {
            return Err("Expected class name");
        };
        let class_id = self.program.add_ident(&class_name);
        let has_parent = if self.peek() == &Token::KeywordExtends {
            self.advance();
            self.compile_primary()?;
            true
        } else {
            false
        };

        if self.peek() == &Token::LBrace {
            self.advance();
        }

        let mut method_count = 0u16;
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            self.consume_optional_semicolons();
            if self.peek() == &Token::RBrace {
                break;
            }
            let mut is_static = false;
            let mut is_async = false;
            if matches!(self.peek(), Token::Ident(ref s) if s == "static")
                && !matches!(self.peek_at(1), Token::LParen)
            {
                is_static = true;
                self.advance();
            }
            if self.peek() == &Token::KeywordAsync {
                is_async = true;
                self.advance();
            }
            let Token::Ident(m_name) = self.advance() else {
                self.advance();
                continue;
            };
            let key_label = if is_static {
                format!("__static::{}", m_name)
            } else {
                m_name.clone()
            };
            let key_c = self.program.add_const(JsValue::Str(key_label));
            self.program.instructions.push(OpCode::LoadConst(key_c));

            let fn_idx = self.compile_function_body(Some(m_name), is_async)?;
            self.program.instructions.push(OpCode::MakeClosure(fn_idx));
            method_count += 1;
        }
        if self.peek() == &Token::RBrace {
            self.advance();
        }

        self.program.instructions.push(OpCode::BuildClass {
            has_parent,
            method_count,
        });
        self.program.instructions.push(OpCode::DeclVar(class_id));
        self.program.instructions.push(OpCode::Pop);
        Ok(())
    }

    fn compile_for_statement(&mut self) -> Result<(), &'static str> {
        self.advance();
        if self.peek() == &Token::LParen {
            self.advance();
        }

        let is_for_of_or_in = (self.peek() == &Token::KeywordLet
            && matches!(self.peek_at(1), Token::Ident(_))
            && matches!(self.peek_at(2), Token::KeywordOf | Token::KeywordIn))
            || (matches!(self.peek(), Token::Ident(_))
                && matches!(self.peek_at(1), Token::KeywordOf | Token::KeywordIn));

        if is_for_of_or_in {
            let is_decl = self.peek() == &Token::KeywordLet;
            if is_decl {
                self.advance();
            }
            let Token::Ident(item_name) = self.advance() else {
                return Err("Expected loop variable");
            };
            let is_for_in = self.advance() == Token::KeywordIn;
            self.compile_expr()?;
            if self.peek() == &Token::RParen {
                self.advance();
            }
            if is_for_in {
                self.program.instructions.push(OpCode::KeysArray);
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
        self.advance();
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
                    let body_skip = self.program.instructions.len();
                    self.program.instructions.push(OpCode::Jump(0));
                    let test_pc = self.program.instructions.len();
                    if let Some(nc) = next_case_patch.take() {
                        self.program.instructions[nc] = OpCode::JumpIfFalse(test_pc);
                    }
                    self.program.instructions[ft] = OpCode::Jump(test_pc);
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

    fn compile_function_body(&mut self, name: Option<String>, is_async: bool) -> Result<u16, &'static str> {
        if self.peek() == &Token::LParen {
            self.advance();
        }
        let skip_jump_idx = self.program.instructions.len();
        self.program.instructions.push(OpCode::Jump(0));

        let mut params = Vec::new();
        while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
            let is_rest = if self.peek() == &Token::Spread {
                self.advance();
                true
            } else {
                false
            };
            if let Token::Ident(p) = self.advance() {
                let name_id = self.program.add_ident(&p);
                let default_pc = if self.peek() == &Token::Assign {
                    self.advance();
                    let dp = self.program.instructions.len();
                    self.compile_expr()?;
                    self.program.instructions.push(OpCode::Return);
                    Some(dp)
                } else {
                    None
                };
                params.push(JsParam {
                    name_id,
                    default_pc,
                    is_rest,
                });
            }
            if self.peek() == &Token::Comma {
                self.advance();
            }
        }
        if self.peek() == &Token::RParen {
            self.advance();
        }

        let entry_pc = self.program.instructions.len();
        let fn_idx = self.program.functions.len() as u16;
        self.program.functions.push(JsFunctionDef {
            name,
            params,
            entry_pc,
            is_async,
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
        if self.is_arrow_function_ahead() {
            return self.compile_arrow_function();
        }

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
                    LValue::Prop { prop_id } => {
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

        self.pos = saved_pos;
        self.program.instructions.truncate(saved_instr_len);
        self.compile_ternary()
    }

    fn is_arrow_function_ahead(&self) -> bool {
        let mut off = 0usize;
        if self.peek() == &Token::KeywordAsync {
            off = 1;
        }
        if matches!(self.peek_at(off), Token::Ident(_)) && self.peek_at(off + 1) == &Token::Arrow {
            return true;
        }
        if self.peek_at(off) == &Token::LParen {
            let mut idx = self.pos + off + 1;
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
        let is_async = if self.peek() == &Token::KeywordAsync {
            self.advance();
            true
        } else {
            false
        };
        let skip_jump = self.program.instructions.len();
        self.program.instructions.push(OpCode::Jump(0));

        let mut params = Vec::new();
        if let Token::Ident(p) = self.peek().clone() {
            self.advance();
            params.push(JsParam {
                name_id: self.program.add_ident(&p),
                default_pc: None,
                is_rest: false,
            });
        } else if self.peek() == &Token::LParen {
            self.advance();
            while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                let is_rest = if self.peek() == &Token::Spread {
                    self.advance();
                    true
                } else {
                    false
                };
                if let Token::Ident(p) = self.advance() {
                    let name_id = self.program.add_ident(&p);
                    let default_pc = if self.peek() == &Token::Assign {
                        self.advance();
                        let dp = self.program.instructions.len();
                        self.compile_expr()?;
                        self.program.instructions.push(OpCode::Return);
                        Some(dp)
                    } else {
                        None
                    };
                    params.push(JsParam {
                        name_id,
                        default_pc,
                        is_rest,
                    });
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

        let entry_pc = self.program.instructions.len();
        let fn_idx = self.program.functions.len() as u16;
        self.program.functions.push(JsFunctionDef {
            name: None,
            params,
            entry_pc,
            is_async,
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

        if !matches!(self.peek_at(1), Token::Dot | Token::LBracket | Token::LParen) {
            return Ok(None);
        }

        self.advance();
        let root_id = self.program.add_ident(&name);
        self.program.instructions.push(OpCode::LoadIdent(root_id));
        let mut current_lval: Option<LValue> = None;

        loop {
            if self.peek() == &Token::Dot {
                if let Some(LValue::Prop { prop_id }) = current_lval.take() {
                    self.program.instructions.push(OpCode::GetProp(prop_id));
                } else if let Some(LValue::Index) = current_lval.take() {
                    self.program.instructions.push(OpCode::GetIndex);
                }
                self.advance();
                let Token::Ident(prop) = self.advance() else {
                    return Ok(None);
                };
                let pid = self.program.add_ident(&prop);
                current_lval = Some(LValue::Prop { prop_id: pid });
            } else if self.peek() == &Token::LBracket {
                if let Some(LValue::Prop { prop_id }) = current_lval.take() {
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
                let (is_spread, argc) = self.compile_call_args_or_spread()?;
                match current_lval.take() {
                    Some(LValue::Prop { prop_id }) => {
                        self.program.instructions.push(if is_spread {
                            OpCode::CallMethodArray { prop_id }
                        } else {
                            OpCode::CallMethod { prop_id, argc }
                        });
                    }
                    _ => {
                        self.program.instructions.push(if is_spread {
                            OpCode::CallValueArray
                        } else {
                            OpCode::CallValue { argc }
                        });
                    }
                }
            } else {
                break;
            }
        }
        Ok(current_lval)
    }

    fn compile_call_args_or_spread(&mut self) -> Result<(bool, u8), &'static str> {
        let mut has_spread = false;
        let mut scan = self.pos;
        let mut depth = 1usize;
        while scan < self.tokens.len() && depth > 0 {
            match self.tokens[scan] {
                Token::LParen | Token::LBracket | Token::LBrace => depth += 1,
                Token::RParen | Token::RBracket | Token::RBrace => depth -= 1,
                Token::Spread if depth == 1 => {
                    has_spread = true;
                    break;
                }
                Token::Eof => break,
                _ => {}
            }
            scan += 1;
        }

        if has_spread {
            self.program.instructions.push(OpCode::MakeArray(0));
            while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                if self.peek() == &Token::Spread {
                    self.advance();
                    self.compile_expr()?;
                    self.program.instructions.push(OpCode::ArraySpread);
                } else {
                    self.compile_expr()?;
                    self.program.instructions.push(OpCode::ArrayPush);
                }
                if self.peek() == &Token::Comma {
                    self.advance();
                }
            }
            if self.peek() == &Token::RParen {
                self.advance();
            }
            Ok((true, 0))
        } else {
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
            Ok((false, argc))
        }
    }

    fn compile_ternary(&mut self) -> Result<(), &'static str> {
        self.compile_nullish()?;
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

    fn compile_nullish(&mut self) -> Result<(), &'static str> {
        self.compile_logical_or()?;
        while self.peek() == &Token::Nullish {
            self.advance();
            self.compile_logical_or()?;
            self.program.instructions.push(OpCode::Nullish);
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
        self.compile_bit_or()?;
        while self.peek() == &Token::AndAnd {
            self.advance();
            self.compile_bit_or()?;
            self.program.instructions.push(OpCode::And);
        }
        Ok(())
    }

    fn compile_bit_or(&mut self) -> Result<(), &'static str> {
        self.compile_bit_xor()?;
        while self.peek() == &Token::BitOr {
            self.advance();
            self.compile_bit_xor()?;
            self.program.instructions.push(OpCode::BitOr);
        }
        Ok(())
    }

    fn compile_bit_xor(&mut self) -> Result<(), &'static str> {
        self.compile_bit_and()?;
        while self.peek() == &Token::BitXor {
            self.advance();
            self.compile_bit_and()?;
            self.program.instructions.push(OpCode::BitXor);
        }
        Ok(())
    }

    fn compile_bit_and(&mut self) -> Result<(), &'static str> {
        self.compile_equality()?;
        while self.peek() == &Token::BitAnd {
            self.advance();
            self.compile_equality()?;
            self.program.instructions.push(OpCode::BitAnd);
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
        self.compile_shift()?;
        while matches!(
            self.peek(),
            Token::Lt
                | Token::Lte
                | Token::Gt
                | Token::Gte
                | Token::KeywordIn
                | Token::KeywordInstanceof
        ) {
            let op = self.advance();
            self.compile_shift()?;
            match op {
                Token::Lt => self.program.instructions.push(OpCode::Lt),
                Token::Lte => self.program.instructions.push(OpCode::Lte),
                Token::Gt => self.program.instructions.push(OpCode::Gt),
                Token::Gte => self.program.instructions.push(OpCode::Gte),
                Token::KeywordIn => self.program.instructions.push(OpCode::InOp),
                Token::KeywordInstanceof => self.program.instructions.push(OpCode::InstanceOf),
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_shift(&mut self) -> Result<(), &'static str> {
        self.compile_additive()?;
        while matches!(self.peek(), Token::Shl | Token::Shr | Token::UShr) {
            let op = self.advance();
            self.compile_additive()?;
            match op {
                Token::Shl => self.program.instructions.push(OpCode::Shl),
                Token::Shr => self.program.instructions.push(OpCode::Shr),
                Token::UShr => self.program.instructions.push(OpCode::UShr),
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
        self.compile_exponent()?;
        while matches!(self.peek(), Token::Star | Token::Slash | Token::Percent) {
            let op = self.advance();
            self.compile_exponent()?;
            match op {
                Token::Star => self.program.instructions.push(OpCode::Mul),
                Token::Slash => self.program.instructions.push(OpCode::Div),
                Token::Percent => self.program.instructions.push(OpCode::Mod),
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_exponent(&mut self) -> Result<(), &'static str> {
        self.compile_unary()?;
        if self.peek() == &Token::Pow {
            self.advance();
            self.compile_exponent()?;
            self.program.instructions.push(OpCode::Pow);
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
            Token::Plus => {
                self.advance();
                self.compile_unary()
            }
            Token::Bang => {
                self.advance();
                self.compile_unary()?;
                self.program.instructions.push(OpCode::Not);
                Ok(())
            }
            Token::BitNot => {
                self.advance();
                self.compile_unary()?;
                self.program.instructions.push(OpCode::BitNot);
                Ok(())
            }
            Token::KeywordTypeof => {
                self.advance();
                self.compile_unary()?;
                self.program.instructions.push(OpCode::TypeOf);
                Ok(())
            }
            Token::KeywordAwait => {
                self.advance();
                self.compile_unary()?;
                self.program.instructions.push(OpCode::Await);
                Ok(())
            }
            Token::KeywordDelete => {
                self.advance();
                if let Token::Ident(root) = self.advance() {
                    let rid = self.program.add_ident(&root);
                    self.program.instructions.push(OpCode::LoadIdent(rid));
                    if self.peek() == &Token::Dot {
                        self.advance();
                        if let Token::Ident(prop) = self.advance() {
                            let pid = self.program.add_ident(&prop);
                            self.program.instructions.push(OpCode::DeleteProp(pid));
                            return Ok(());
                        }
                    }
                }
                let t = self.program.add_const(JsValue::Bool(true));
                self.program.instructions.push(OpCode::LoadConst(t));
                Ok(())
            }
            Token::KeywordNew => {
                self.advance();
                self.compile_primary()?;
                while self.peek() == &Token::Dot {
                    self.advance();
                    if let Token::Ident(p) = self.advance() {
                        let pid = self.program.add_ident(&p);
                        self.program.instructions.push(OpCode::GetProp(pid));
                    }
                }
                let argc = if self.peek() == &Token::LParen {
                    self.advance();
                    let (_, count) = self.compile_call_args_or_spread()?;
                    count
                } else {
                    0
                };
                self.program.instructions.push(OpCode::Construct { argc });
                Ok(())
            }
            Token::PlusPlus | Token::MinusMinus => {
                let is_inc = self.advance() == Token::PlusPlus;
                if let Token::Ident(name) = self.advance() {
                    let id = self.program.add_ident(&name);
                    let one = self.program.add_const(JsValue::Number(1.0));
                    self.program.instructions.push(OpCode::LoadIdent(id));
                    self.program.instructions.push(OpCode::LoadConst(one));
                    self.program
                        .instructions
                        .push(if is_inc { OpCode::Add } else { OpCode::Sub });
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
                self.program
                    .instructions
                    .push(if is_inc { OpCode::Add } else { OpCode::Sub });
                self.program.instructions.push(OpCode::StoreVar(id));
                self.program.instructions.push(OpCode::Pop);
                return Ok(());
            }
        }

        self.compile_primary()?;

        loop {
            if self.peek() == &Token::Dot || self.peek() == &Token::OptChain {
                let is_opt = self.advance() == Token::OptChain;
                if self.peek() == &Token::LBracket {
                    self.advance();
                    self.compile_expr()?;
                    if self.peek() == &Token::RBracket {
                        self.advance();
                    }
                    self.program.instructions.push(OpCode::GetIndex);
                    continue;
                }
                let Token::Ident(prop) = self.advance() else {
                    return Err("Expected property name after `.`");
                };
                let prop_id = self.program.add_ident(&prop);
                if self.peek() == &Token::LParen {
                    self.advance();
                    let (is_spread, argc) = self.compile_call_args_or_spread()?;
                    self.program.instructions.push(if is_spread {
                        OpCode::CallMethodArray { prop_id }
                    } else {
                        OpCode::CallMethod { prop_id, argc }
                    });
                } else {
                    self.program.instructions.push(if is_opt {
                        OpCode::GetPropOpt(prop_id)
                    } else {
                        OpCode::GetProp(prop_id)
                    });
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
                let (is_spread, argc) = self.compile_call_args_or_spread()?;
                self.program.instructions.push(if is_spread {
                    OpCode::CallValueArray
                } else {
                    OpCode::CallValue { argc }
                });
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
            Token::Regex(pat, flags) => {
                let pat_id = self.program.add_ident(&pat);
                let flags_id = self.program.add_ident(&flags);
                self.program
                    .instructions
                    .push(OpCode::MakeRegex { pat_id, flags_id });
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
                self.program.instructions.push(OpCode::MakeArray(0));
                while self.peek() != &Token::RBracket && self.peek() != &Token::Eof {
                    if self.peek() == &Token::Spread {
                        self.advance();
                        self.compile_expr()?;
                        self.program.instructions.push(OpCode::ArraySpread);
                    } else {
                        self.compile_expr()?;
                        self.program.instructions.push(OpCode::ArrayPush);
                    }
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                }
                if self.peek() == &Token::RBracket {
                    self.advance();
                }
                Ok(())
            }
            Token::LBrace => {
                self.program.instructions.push(OpCode::MakeObject(0));
                while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
                    if self.peek() == &Token::Spread {
                        self.advance();
                        self.compile_expr()?;
                        self.program.instructions.push(OpCode::ObjectSpread);
                    } else {
                        let key_str = match self.advance() {
                            Token::Ident(k) | Token::Str(k) => k,
                            Token::Number(n) => JsValue::Number(n).as_str(),
                            _ => break,
                        };
                        let key_id = self.program.add_ident(&key_str);
                        self.program.instructions.push(OpCode::Dup);
                        if self.peek() == &Token::Colon {
                            self.advance();
                            self.compile_expr()?;
                        } else {
                            self.program.instructions.push(OpCode::LoadIdent(key_id));
                        }
                        self.program.instructions.push(OpCode::SetProp(key_id));
                        self.program.instructions.push(OpCode::Pop);
                    }
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                }
                if self.peek() == &Token::RBrace {
                    self.advance();
                }
                Ok(())
            }
            Token::KeywordAsync if self.peek() == &Token::KeywordFunction => {
                self.advance();
                let name = if let Token::Ident(n) = self.peek().clone() {
                    self.advance();
                    Some(n)
                } else {
                    None
                };
                let fn_idx = self.compile_function_body(name, true)?;
                self.program.instructions.push(OpCode::MakeClosure(fn_idx));
                Ok(())
            }
            Token::KeywordFunction => {
                let name = if let Token::Ident(n) = self.peek().clone() {
                    self.advance();
                    Some(n)
                } else {
                    None
                };
                let fn_idx = self.compile_function_body(name, false)?;
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
