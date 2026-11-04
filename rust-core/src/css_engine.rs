use std::collections::HashMap;

use crate::dom_arena::{DomNode, NodeKind};

#[derive(Debug, Clone, Copy)]
pub struct ComputedStyle {
    pub text_color: u32,       // ARGB 0xAARRGGBB
    pub background_color: u32, // ARGB 0xAARRGGBB
    pub border_color: u32,     // ARGB 0xAARRGGBB
    pub font_size_sp: f32,
    pub font_weight_bold: bool,
    pub margin_vertical: f32,
    pub padding: f32,
    pub corner_radius: f32,
    pub explicit_height: Option<f32>,
    pub display_none: bool,
    pub is_flex_row: bool,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            text_color: 0xFFF1F5F9,
            background_color: 0x00000000,
            border_color: 0x00000000,
            font_size_sp: 15.0,
            font_weight_bold: false,
            margin_vertical: 6.0,
            padding: 0.0,
            corner_radius: 8.0,
            explicit_height: None,
            display_none: false,
            is_flex_row: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector: String,
    pub specificity: (u16, u16, u16), // (id_count, class_count, tag_count)
    pub source_order: usize,
    pub declarations: Vec<(String, String)>,
}

/// High-speed streaming CSS parser and W3C Specificity Cascade Matcher.
pub struct MicroCssEngine {
    rules: Vec<CssRule>,
    variables: HashMap<String, String>,
    next_order: usize,
}

impl Default for MicroCssEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MicroCssEngine {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            variables: HashMap::new(),
            next_order: 0,
        }
    }

    pub fn parse_stylesheet(&mut self, css_text: &str) {
        for block in css_text.split('}') {
            let Some(open_brace) = block.find('{') else {
                continue;
            };
            let selector_group = block[..open_brace].trim();
            let decl_block = block[open_brace + 1..].trim();
            if selector_group.is_empty() || selector_group.starts_with("@keyframes") {
                continue;
            }

            let mut declarations = Vec::new();
            for pair in decl_block.split(';') {
                if let Some(colon) = pair.find(':') {
                    let prop = pair[..colon].trim().to_string();
                    let val = pair[colon + 1..]
                        .trim()
                        .trim_end_matches("!important")
                        .trim()
                        .to_string();
                    if prop.starts_with("--") {
                        self.variables.insert(prop, val);
                    } else if !prop.is_empty() && !val.is_empty() {
                        declarations.push((prop.to_ascii_lowercase(), val));
                    }
                }
            }

            if !declarations.is_empty() {
                for sel in selector_group.split(',') {
                    let trimmed = sel.trim().to_string();
                    if !trimmed.is_empty() {
                        let spec = compute_specificity(&trimmed);
                        let order = self.next_order;
                        self.next_order += 1;
                        self.rules.push(CssRule {
                            selector: trimmed,
                            specificity: spec,
                            source_order: order,
                            declarations: declarations.clone(),
                        });
                    }
                }
            }
        }
        // Keep rules sorted by W3C specificity `(id, class, tag)` then `source_order`
        self.rules
            .sort_by_key(|r| (r.specificity, r.source_order));
    }

    pub fn compute_style(&self, node: &DomNode) -> ComputedStyle {
        let mut style = ComputedStyle::default();

        let NodeKind::Element {
            ref tag,
            ref id_attr,
            ref classes,
            ref inline_style,
            ..
        } = node.kind
        else {
            return style;
        };

        // 1. User-Agent semantic defaults (including interactive form controls)
        match tag.as_str() {
            "head" | "script" | "style" | "meta" | "link" | "title" | "noscript" | "template" => {
                style.display_none = true;
                return style;
            }
            "h1" => {
                style.font_size_sp = 26.0;
                style.font_weight_bold = true;
                style.margin_vertical = 14.0;
            }
            "h2" => {
                style.font_size_sp = 21.0;
                style.font_weight_bold = true;
                style.margin_vertical = 12.0;
            }
            "h3" => {
                style.font_size_sp = 18.0;
                style.font_weight_bold = true;
                style.margin_vertical = 10.0;
            }
            "h4" | "strong" | "b" | "label" => {
                style.font_weight_bold = true;
            }
            "a" => {
                style.text_color = 0xFFA8C7FA;
            }
            "pre" | "code" => {
                style.background_color = 0xFF12141C;
                style.text_color = 0xFF6DD58C;
                style.padding = 8.0;
                style.corner_radius = 12.0;
            }
            "blockquote" => {
                style.background_color = 0xFF161922;
                style.text_color = 0xFF94A3B8;
                style.padding = 10.0;
            }
            "input" | "textarea" | "select" => {
                style.background_color = 0xFF161922;
                style.border_color = 0xFF334155;
                style.text_color = 0xFFF8FAFC;
                style.padding = 10.0;
                style.corner_radius = 12.0;
                style.margin_vertical = 8.0;
            }
            "button" => {
                style.background_color = 0xFF1E3252;
                style.text_color = 0xFFD3E3FD;
                style.font_weight_bold = true;
                style.padding = 10.0;
                style.corner_radius = 16.0;
            }
            _ => {}
        }

        // 2. Match stylesheet rules sorted by W3C specificity
        for rule in &self.rules {
            if selector_matches(&rule.selector, tag, id_attr.as_deref(), classes) {
                self.apply_declarations(&mut style, &rule.declarations);
            }
        }

        // 3. Apply inline `style="..."` attribute (highest cascade priority)
        if let Some(inline) = inline_style {
            let mut inline_decls = Vec::new();
            for pair in inline.split(';') {
                if let Some(colon) = pair.find(':') {
                    inline_decls.push((
                        pair[..colon].trim().to_ascii_lowercase(),
                        pair[colon + 1..]
                            .trim()
                            .trim_end_matches("!important")
                            .trim()
                            .to_string(),
                    ));
                }
            }
            self.apply_declarations(&mut style, &inline_decls);
        }

        style
    }

    fn apply_declarations(&self, style: &mut ComputedStyle, decls: &[(String, String)]) {
        for (prop, raw_val) in decls {
            let val = self.resolve_var(raw_val);
            match prop.as_str() {
                "display" => {
                    if val.eq_ignore_ascii_case("none") {
                        style.display_none = true;
                    } else if val.eq_ignore_ascii_case("flex")
                        || val.eq_ignore_ascii_case("inline-flex")
                    {
                        style.display_none = false;
                        style.is_flex_row = true;
                    } else {
                        style.display_none = false;
                    }
                }
                "color" => {
                    if let Some(c) = parse_css_color(&val) {
                        style.text_color = c;
                    }
                }
                "background" | "background-color" => {
                    if let Some(c) = parse_css_color(&val) {
                        style.background_color = c;
                    }
                }
                "border-color" => {
                    if let Some(c) = parse_css_color(&val) {
                        style.border_color = c;
                    }
                }
                "font-size" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.font_size_sp = px.clamp(10.0, 64.0);
                    }
                }
                "font-weight" => {
                    if val.contains("bold")
                        || matches!(val.as_str(), "600" | "700" | "800" | "900")
                    {
                        style.font_weight_bold = true;
                    } else if val == "normal" || val == "400" {
                        style.font_weight_bold = false;
                    }
                }
                "padding" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.padding = px.clamp(0.0, 64.0);
                    }
                }
                "margin" | "margin-top" | "margin-bottom" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.margin_vertical = px.clamp(0.0, 64.0);
                    }
                }
                "height" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.explicit_height = Some(px.clamp(16.0, 800.0));
                    }
                }
                "border-radius" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.corner_radius = px.clamp(0.0, 64.0);
                    }
                }
                _ => {}
            }
        }
    }

    fn resolve_var(&self, val: &str) -> String {
        if let Some(inner) = val.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
            let var_name = inner.split(',').next().unwrap_or("").trim();
            if let Some(resolved) = self.variables.get(var_name) {
                return resolved.clone();
            }
        }
        val.to_string()
    }

    pub fn clear(&mut self) {
        self.rules.clear();
        self.rules.shrink_to_fit();
        self.variables.clear();
        self.next_order = 0;
    }
}

fn compute_specificity(selector: &str) -> (u16, u16, u16) {
    let mut ids = 0u16;
    let mut classes = 0u16;
    let mut tags = 0u16;
    for part in selector.split_whitespace() {
        let p = part.trim_matches_combinator();
        if p.is_empty() {
            continue;
        }
        ids += p.matches('#').count() as u16;
        classes += p.matches('.').count() as u16;
        if !p.starts_with('#') && !p.starts_with('.') && !p.starts_with('*') {
            tags += 1;
        }
    }
    (ids, classes, tags)
}

trait TrimCombinator {
    fn trim_matches_combinator(&self) -> &str;
}

impl TrimCombinator for str {
    fn trim_matches_combinator(&self) -> &str {
        self.trim_matches(|c| c == '>' || c == '+' || c == '~')
    }
}

fn selector_matches(
    selector: &str,
    tag: &str,
    id_attr: Option<&str>,
    classes: &[String],
) -> bool {
    let target_sel = selector.split_whitespace().last().unwrap_or(selector);
    if target_sel == "*" {
        return true;
    }
    if let Some(id) = target_sel.strip_prefix('#') {
        return id_attr == Some(id);
    }
    if let Some(cls) = target_sel.strip_prefix('.') {
        return classes.iter().any(|c| c == cls);
    }
    if let Some(dot_idx) = target_sel.find('.') {
        let tag_part = &target_sel[..dot_idx];
        let cls_part = &target_sel[dot_idx + 1..];
        return tag_part.eq_ignore_ascii_case(tag) && classes.iter().any(|c| c == cls_part);
    }
    if let Some(hash_idx) = target_sel.find('#') {
        let tag_part = &target_sel[..hash_idx];
        let id_part = &target_sel[hash_idx + 1..];
        return tag_part.eq_ignore_ascii_case(tag) && id_attr == Some(id_part);
    }
    target_sel.eq_ignore_ascii_case(tag)
}

pub fn parse_css_color(raw: &str) -> Option<u32> {
    let v = raw.trim();
    if let Some(hex) = v.strip_prefix('#') {
        if hex.len() == 6 {
            let rgb = u32::from_str_radix(hex, 16).ok()?;
            return Some(0xFF00_0000 | rgb);
        } else if hex.len() == 3 {
            let r = u32::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u32::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u32::from_str_radix(&hex[2..3], 16).ok()?;
            return Some(0xFF00_0000 | ((r * 17) << 16) | ((g * 17) << 8) | (b * 17));
        }
    }
    if let Some(rgb_inner) = v
        .strip_prefix("rgb(")
        .or_else(|| v.strip_prefix("rgba("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let nums: Vec<u32> = rgb_inner
            .split(',')
            .take(3)
            .filter_map(|p| p.trim().parse::<u32>().ok())
            .collect();
        if nums.len() == 3 {
            let r = nums[0].min(255);
            let g = nums[1].min(255);
            let b = nums[2].min(255);
            return Some(0xFF00_0000 | (r << 16) | (g << 8) | b);
        }
    }
    match v.to_ascii_lowercase().as_str() {
        "black" => Some(0xFF000000),
        "white" => Some(0xFFFFFFFF),
        "red" => Some(0xFFEF4444),
        "green" => Some(0xFF10B981),
        "blue" => Some(0xFF3B82F6),
        "transparent" => Some(0x00000000),
        _ => None,
    }
}

fn parse_numeric_px(raw: &str) -> Option<f32> {
    let trimmed = raw.trim();
    if let Some(rem) = trimmed
        .strip_suffix("rem")
        .or_else(|| trimmed.strip_suffix("em"))
    {
        return rem.trim().parse::<f32>().ok().map(|v| v * 16.0);
    }
    let clean = trimmed.trim_end_matches("px").trim();
    clean.parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom_arena::ZeroCopyDomArena;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_css_specificity_id_beats_later_tag_rule() {
        let mut dom = ZeroCopyDomArena::new(32 * 1024);
        let mut tok = StreamingHtmlTokenizer::new();
        tok.feed_chunk(b"<div id=\"hero\" class=\"card\">Hello</div>", true, |t| {
            dom.ingest_token(t);
        });

        let mut css = MicroCssEngine::new();
        css.parse_stylesheet("#hero { color: #00FF00; } div { color: #FF0000; }");

        let hero_id = dom.find_by_id("hero").unwrap();
        let style = css.compute_style(&dom.nodes()[hero_id as usize]);
        assert_eq!(style.text_color, 0xFF00FF00);
    }
}
