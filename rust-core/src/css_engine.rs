use crate::dom_arena::{DomNode, NodeKind};

#[derive(Debug, Clone, Copy)]
pub struct ComputedStyle {
    pub text_color: u32,       // ARGB 0xAARRGGBB
    pub background_color: u32, // ARGB 0xAARRGGBB
    pub font_size_sp: f32,
    pub font_weight_bold: bool,
    pub margin_vertical: f32,
    pub padding: f32,
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
            display_none: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector: String,
    pub declarations: Vec<(String, String)>,
}

/// Lightweight streaming CSS parser and cascade matcher.
pub struct MicroCssEngine {
    rules: Vec<CssRule>,
}

impl Default for MicroCssEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MicroCssEngine {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn parse_stylesheet(&mut self, css_text: &str) {
        for block in css_text.split('}') {
            let Some(open_brace) = block.find('{') else {
                continue;
            };
            let selector_group = block[..open_brace].trim();
            let decl_block = block[open_brace + 1..].trim();
            if selector_group.is_empty() || selector_group.starts_with('@') {
                continue;
            }

            let mut declarations = Vec::new();
            for pair in decl_block.split(';') {
                if let Some(colon) = pair.find(':') {
                    let prop = pair[..colon].trim().to_ascii_lowercase();
                    let val = pair[colon + 1..].trim().to_string();
                    if !prop.is_empty() && !val.is_empty() {
                        declarations.push((prop, val));
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

        // 1. User-Agent minimal semantic defaults
        match tag.as_str() {
            "head" | "script" | "style" | "meta" | "link" | "title" | "noscript" => {
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
            "a" => {
                style.text_color = 0xFF38BDF8;
            }
            "button" => {
                style.background_color = 0xFF1E293B;
                style.text_color = 0xFFFFFFFF;
                style.padding = 10.0;
            }
            _ => {}
        }

        // 2. Match stylesheet rules in cascade order
        for rule in &self.rules {
            if selector_matches(&rule.selector, tag, id_attr.as_deref(), classes) {
                apply_declarations(&mut style, &rule.declarations);
            }
        }

        // 3. Apply inline `style="..."` attribute
        if let Some(inline) = inline_style {
            let mut inline_decls = Vec::new();
            for pair in inline.split(';') {
                if let Some(colon) = pair.find(':') {
                    inline_decls.push((
                        pair[..colon].trim().to_ascii_lowercase(),
                        pair[colon + 1..].trim().to_string(),
                    ));
                }
            }
            apply_declarations(&mut style, &inline_decls);
        }

        style
    }

    pub fn clear(&mut self) {
        self.rules.clear();
        self.rules.shrink_to_fit();
    }
}

fn selector_matches(
    selector: &str,
    tag: &str,
    id_attr: Option<&str>,
    classes: &[String],
) -> bool {
    if let Some(id) = selector.strip_prefix('#') {
        return id_attr == Some(id);
    }
    if let Some(cls) = selector.strip_prefix('.') {
        return classes.iter().any(|c| c == cls);
    }
    selector.eq_ignore_ascii_case(tag)
}

fn apply_declarations(style: &mut ComputedStyle, decls: &[(String, String)]) {
    for (prop, val) in decls {
        match prop.as_str() {
            "display" if val.eq_ignore_ascii_case("none") => {
                style.display_none = true;
            }
            "color" => {
                if let Some(c) = parse_css_color(val) {
                    style.text_color = c;
                }
            }
            "background" | "background-color" => {
                if let Some(c) = parse_css_color(val) {
                    style.background_color = c;
                }
            }
            "font-size" => {
                if let Some(px) = parse_numeric_px(val) {
                    style.font_size_sp = px.clamp(10.0, 48.0);
                }
            }
            "font-weight" => {
                if val.contains("bold") || val == "700" || val == "800" || val == "900" {
                    style.font_weight_bold = true;
                }
            }
            "padding" => {
                if let Some(px) = parse_numeric_px(val) {
                    style.padding = px.clamp(0.0, 64.0);
                }
            }
            _ => {}
        }
    }
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
    let trimmed = raw
        .trim()
        .trim_end_matches("px")
        .trim_end_matches("rem")
        .trim_end_matches("em")
        .trim();
    trimmed.parse::<f32>().ok()
}
