use std::collections::HashMap;

use crate::clipboard::ClipboardSandbox;
use crate::dom_arena::ZeroCopyDomArena;

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
                if n.fract() == 0.0 {
                    format!("{:.0}", n)
                } else {
                    n.to_string()
                }
            }
            JsValue::Str(s) => s.clone(),
        }
    }
}

/// High-Efficiency Sandboxed Micro-JS Engine (`MantaJsSandbox`).
/// - Fuel-metered execution (prevents infinite loops and CPU/battery drain).
/// - Anti-bot/Anti-JS-detection built-in (`navigator.webdriver === false`, `navigator.cookieEnabled === false`).
/// - Direct safe bindings to `ZeroCopyDomArena` and `ClipboardSandbox`.
pub struct MantaJsSandbox {
    globals: HashMap<String, JsValue>,
    fuel_remaining: u32,
    max_fuel: u32,
    pub console_logs: Vec<String>,
}

impl MantaJsSandbox {
    pub fn new(max_fuel: u32) -> Self {
        let mut globals = HashMap::with_capacity(16);
        // Built-in anti-detection & privacy environment constants
        globals.insert("navigator.webdriver".to_string(), JsValue::Bool(false));
        globals.insert("navigator.cookieEnabled".to_string(), JsValue::Bool(false));
        globals.insert(
            "navigator.userAgent".to_string(),
            JsValue::Str("Mozilla/5.0 (Linux; Android 14) Manta/1.0".to_string()),
        );
        globals.insert("window.isSecureContext".to_string(), JsValue::Bool(true));

        Self {
            globals,
            fuel_remaining: max_fuel,
            max_fuel,
            console_logs: Vec::new(),
        }
    }

    /// Executes a script inside the fuel-limited sandbox with direct access to the flat DOM arena.
    pub fn execute_script(
        &mut self,
        script: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
    ) -> Result<JsValue, &'static str> {
        self.fuel_remaining = self.max_fuel;
        let mut last_val = JsValue::Undefined;

        for raw_stmt in script.split([';', '\n']) {
            let stmt = raw_stmt.trim();
            if stmt.is_empty() || stmt.starts_with("//") {
                continue;
            }
            if self.fuel_remaining == 0 {
                return Err("JS execution terminated: instruction fuel limit exceeded");
            }
            self.fuel_remaining -= 1;
            last_val = self.eval_statement(stmt, dom, clipboard)?;
        }

        Ok(last_val)
    }

    pub fn get_global(&self, name: &str) -> Option<&JsValue> {
        self.globals.get(name)
    }

    pub fn reset_and_zeroize(&mut self) {
        self.globals.clear();
        self.console_logs.clear();
        self.fuel_remaining = self.max_fuel;
    }

    fn eval_statement(
        &mut self,
        stmt: &str,
        dom: &mut ZeroCopyDomArena,
        clipboard: &mut ClipboardSandbox,
    ) -> Result<JsValue, &'static str> {
        // 1. Variable declaration: `let x = ...` / `const x = ...` / `var x = ...`
        if let Some(rest) = stmt
            .strip_prefix("let ")
            .or_else(|| stmt.strip_prefix("const "))
            .or_else(|| stmt.strip_prefix("var "))
        {
            if let Some(eq_idx) = rest.find('=') {
                let var_name = rest[..eq_idx].trim().to_string();
                let rhs = rest[eq_idx + 1..].trim();
                let val = self.eval_expr(rhs, dom)?;
                self.globals.insert(var_name, val.clone());
                return Ok(val);
            }
        }

        // 2. DOM mutation: `document.getElementById("...").textContent = "..."` or `querySelector`
        if (stmt.starts_with("document.getElementById(")
            || stmt.starts_with("document.querySelector("))
            && stmt.contains(".textContent")
        {
            if let (Some(q1), Some(eq_pos)) = (stmt.find('('), stmt.find('=')) {
                if let Some(q2) = stmt[q1 + 1..].find(')') {
                    let arg_raw = stmt[q1 + 1..q1 + 1 + q2].trim();
                    let selector = unquote(arg_raw);
                    let rhs_val = self.eval_expr(stmt[eq_pos + 1..].trim(), dom)?;

                    let target_node = if stmt.starts_with("document.getElementById(") {
                        dom.find_by_id(selector)
                    } else {
                        dom.query_selector(selector)
                    };

                    if let Some(nid) = target_node {
                        dom.set_text_content(nid, rhs_val.as_str());
                    }
                    return Ok(rhs_val);
                }
            }
        }

        // 3. `document.title = "..."`
        if let Some(rhs) = stmt.strip_prefix("document.title") {
            let trimmed = rhs.trim();
            if let Some(val_expr) = trimmed.strip_prefix('=') {
                let v = self.eval_expr(val_expr.trim(), dom)?;
                dom.page_title = v.as_str();
                return Ok(v);
            }
        }

        // 4. `navigator.clipboard.writeText("...")`
        if let Some(inner) = stmt
            .strip_prefix("navigator.clipboard.writeText(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let val = self.eval_expr(inner.trim(), dom)?;
            clipboard.copy_text(&val.as_str());
            return Ok(JsValue::Undefined);
        }

        // 5. `console.log(...)`
        if let Some(inner) = stmt
            .strip_prefix("console.log(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let val = self.eval_expr(inner.trim(), dom)?;
            if self.console_logs.len() < 32 {
                self.console_logs.push(val.as_str());
            }
            return Ok(JsValue::Undefined);
        }

        self.eval_expr(stmt, dom)
    }

    fn eval_expr(&self, expr: &str, dom: &ZeroCopyDomArena) -> Result<JsValue, &'static str> {
        let e = expr.trim();
        if e == "true" {
            return Ok(JsValue::Bool(true));
        }
        if e == "false" {
            return Ok(JsValue::Bool(false));
        }
        if e == "null" {
            return Ok(JsValue::Null);
        }
        if e == "undefined" {
            return Ok(JsValue::Undefined);
        }

        if (e.starts_with('"') && e.ends_with('"')) || (e.starts_with('\'') && e.ends_with('\'')) {
            return Ok(JsValue::Str(unquote(e).to_string()));
        }

        if let Ok(num) = e.parse::<f64>() {
            return Ok(JsValue::Number(num));
        }

        // Binary addition (numeric or string concatenation)
        if let Some(plus_idx) = e.find('+') {
            let left = self.eval_expr(&e[..plus_idx], dom)?;
            let right = self.eval_expr(&e[plus_idx + 1..], dom)?;
            return match (left, right) {
                (JsValue::Number(a), JsValue::Number(b)) => Ok(JsValue::Number(a + b)),
                (a, b) => Ok(JsValue::Str(format!("{}{}", a.as_str(), b.as_str()))),
            };
        }

        // DOM text query: `document.getElementById("...").textContent`
        if let Some(rest) = e.strip_prefix("document.getElementById(") {
            if let Some(end_paren) = rest.find(')') {
                let id = unquote(rest[..end_paren].trim());
                let prop = rest[end_paren + 1..].trim();
                if prop == ".textContent" {
                    if let Some(nid) = dom.find_by_id(id) {
                        return Ok(JsValue::Str(dom.text_content(nid)));
                    }
                    return Ok(JsValue::Null);
                }
            }
        }

        if let Some(val) = self.globals.get(e) {
            return Ok(val.clone());
        }

        Ok(JsValue::Undefined)
    }
}

fn unquote(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_js_dom_mutation_and_anti_webdriver() {
        let mut dom = ZeroCopyDomArena::new(64 * 1024);
        let mut tokenizer = StreamingHtmlTokenizer::new();
        tokenizer.feed_chunk(b"<div id=\"status\">Idle</div>", true, |tok| {
            dom.ingest_token(tok);
        });

        let mut vm = MantaJsSandbox::new(1000);
        let mut clip = ClipboardSandbox::new(1024);

        let script = r#"
            let bot = navigator.webdriver;
            document.getElementById("status").textContent = "Manta Fast Execution";
            navigator.clipboard.writeText("Copied from Manta");
        "#;
        vm.execute_script(script, &mut dom, &mut clip).unwrap();

        assert_eq!(vm.get_global("bot"), Some(&JsValue::Bool(false)));
        let nid = dom.find_by_id("status").unwrap();
        assert_eq!(dom.text_content(nid), "Manta Fast Execution");
        assert_eq!(clip.paste_text(), "Copied from Manta");
    }
}
