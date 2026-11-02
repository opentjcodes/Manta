use std::collections::HashMap;

use crate::dom_arena::{DomNode, NodeKind};

#[derive(Debug, Clone, Copy)]
pub struct ComputedStyle {
    pub text_color: u32,       // ARGB 0xAARRGGBB
    pub background_color: u32, // ARGB 0xAARRGGBB
    pub font_size_sp: f32,
    pub font_weight_bold: bool,
    pub margin_vertical: f32,
    pub padding: f32,
    pub corner_radius: f32,
    pub display_none: bool,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            text_color: 0xFFF1F5F9,       // Clean crisp off-white for Manta dark mode
            background_color: 0x00000000, // Transparent by default
            font_size_sp: 15.0,
            font_weight_bold: false,
            margin_vertical: 6.0,
            padding: 0.0,
            corner_radius: 8.0,
            display_none: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector: String,
    pub declarations: Vec<(String, String)>,
}

/// Lightweight streaming CSS parser and cascade matcher supporting CSS variables (`--var`),
/// compound selectors (`tag.class`, `tag#id`), `rgb()`/`rgba()`, hex, and named colors.
pub struct MicroCssEngine {
    rules: Vec<CssRule>,
    variables: HashMap<String, String>,
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
                        self.rules.push(CssRule {
                            selector: trimmed,
                            declarations: declarations.clone(),
                        });
                    }
                }
            }
        }
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

        // 1. User-Agent semantic defaults
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
            "h4" | "strong" | "b" => {
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
            "button" => {
                style.background_color = 0xFF1E3252;
                style.text_color = 0xFFD3E3FD;
                style.font_weight_bold = true;
                style.padding = 10.0;
                style.corner_radius = 16.0;
            }
            _ => {}
        }

        // 2. Match stylesheet rules in cascade order
        for rule in &self.rules {
            if selector_matches(&rule.selector, tag, id_attr.as_deref(), classes) {
                self.apply_declarations(&mut style, &rule.declarations);
            }
        }

        // 3. Apply inline `style="..."` attribute
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
                "display" if val.eq_ignore_ascii_case("none") => {
                    style.display_none = true;
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
                "font-size" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.font_size_sp = px.clamp(10.0, 48.0);
                    }
                }
                "font-weight" => {
                    if val.contains("bold") || val == "600" || val == "700" || val == "800" || val == "900" {
                        style.font_weight_bold = true;
                    }
                }
                "padding" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.padding = px.clamp(0.0, 64.0);
                    }
                }
                "border-radius" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.corner_radius = px.clamp(0.0, 48.0);
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
    }
}

fn selector_matches(
    selector: &str,
    tag: &str,
    id_attr: Option<&str>,
    classes: &[String],
) -> bool {
    let target_sel = selector.split_whitespace().last().unwrap_or(selector);
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
            return Some(0xFF00_0000 | (r * 17) << 16 | (g * 17) << 8 | (b * 17));
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
    if let Some(rem) = trimmed.strip_suffix("rem").or_else(|| trimmed.strip_suffix("em")) {
        return rem.trim().parse::<f32>().ok().map(|v| v * 16.0);
    }
    let clean = trimmed.trim_end_matches("px").trim();
    clean.parse::<f32>().ok()
}
