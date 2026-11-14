use std::collections::HashMap;

use crate::dom_arena::{NodeId, NodeKind, ZeroCopyDomArena, NO_NODE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Block,
    Inline,
    InlineBlock,
    Flex,
    Grid,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDir {
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    FlexStart,
    Center,
    FlexEnd,
    SpaceBetween,
    SpaceAround,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionMode {
    Static,
    Relative,
    Absolute,
    Fixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy)]
pub struct ComputedStyle {
    pub display: DisplayMode,
    pub position: PositionMode,
    pub flex_direction: FlexDir,
    pub justify_content: JustifyContent,
    pub flex_wrap: bool,
    pub grid_columns: u16,
    pub gap: f32,
    pub flex_grow: f32,
    pub text_color: u32,
    pub background_color: u32,
    pub border_color: u32,
    pub border_width: f32,
    pub font_size_sp: f32,
    pub font_weight_bold: bool,
    pub line_height_mult: f32,
    pub text_align: TextAlign,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    pub padding_top: f32,
    pub padding_bottom: f32,
    pub padding_left: f32,
    pub padding_right: f32,
    pub corner_radius: f32,
    pub explicit_width: Option<f32>,
    pub max_width: Option<f32>,
    pub explicit_height: Option<f32>,
    pub min_height: Option<f32>,
    pub z_index: i32,
    pub overflow_hidden: bool,
    pub opacity: f32,
    pub visibility_hidden: bool,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            display: DisplayMode::Block,
            position: PositionMode::Static,
            flex_direction: FlexDir::Row,
            justify_content: JustifyContent::FlexStart,
            flex_wrap: false,
            grid_columns: 1,
            gap: 8.0,
            flex_grow: 0.0,
            text_color: 0xFFF1F5F9,
            background_color: 0x00000000,
            border_color: 0x00000000,
            border_width: 0.0,
            font_size_sp: 15.0,
            font_weight_bold: false,
            line_height_mult: 1.45,
            text_align: TextAlign::Left,
            margin_top: 4.0,
            margin_bottom: 4.0,
            margin_left: 0.0,
            margin_right: 0.0,
            padding_top: 0.0,
            padding_bottom: 0.0,
            padding_left: 0.0,
            padding_right: 0.0,
            corner_radius: 8.0,
            explicit_width: None,
            max_width: None,
            explicit_height: None,
            min_height: None,
            z_index: 0,
            overflow_hidden: false,
            opacity: 1.0,
            visibility_hidden: false,
        }
    }
}

impl ComputedStyle {
    pub fn inherit_from(parent: &ComputedStyle) -> Self {
        Self {
            text_color: parent.text_color,
            font_size_sp: parent.font_size_sp,
            font_weight_bold: parent.font_weight_bold,
            line_height_mult: parent.line_height_mult,
            text_align: parent.text_align,
            visibility_hidden: parent.visibility_hidden,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    AdjacentSibling,
    GeneralSibling,
}

#[derive(Debug, Clone)]
pub enum AttrOp {
    Present,
    Equals(String),
    StartsWith(String),
    EndsWith(String),
    Contains(String),
}

#[derive(Debug, Clone)]
pub struct AttrCond {
    pub name: String,
    pub op: AttrOp,
}

#[derive(Debug, Clone)]
pub enum PseudoClass {
    Root,
    FirstChild,
    LastChild,
    Empty,
    Checked,
    Disabled,
    Enabled,
    Focus,
    NthEven,
    NthOdd,
    NthExact(usize),
    Not(Box<CompoundSelector>),
}

#[derive(Debug, Clone, Default)]
pub struct CompoundSelector {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrCond>,
    pub pseudos: Vec<PseudoClass>,
}

#[derive(Debug, Clone)]
pub struct ComplexSelector {
    pub parts: Vec<(Option<Combinator>, CompoundSelector)>,
}

#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector: ComplexSelector,
    pub specificity: (u16, u16, u16),
    pub source_order: usize,
    pub declarations: Vec<(String, String)>,
}

pub struct MicroCssEngine {
    rules: Vec<CssRule>,
    id_buckets: HashMap<String, Vec<usize>>,
    class_buckets: HashMap<String, Vec<usize>>,
    tag_buckets: HashMap<String, Vec<usize>>,
    universal_bucket: Vec<usize>,
    variables: HashMap<String, String>,
    next_order: usize,
    pub viewport_width: f32,
    pub dark_mode: bool,
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
            id_buckets: HashMap::new(),
            class_buckets: HashMap::new(),
            tag_buckets: HashMap::new(),
            universal_bucket: Vec::new(),
            variables: HashMap::new(),
            next_order: 0,
            viewport_width: 390.0,
            dark_mode: true,
        }
    }

    pub fn parse_stylesheet(&mut self, css_text: &str) {
        let stripped = strip_css_comments(css_text);
        self.parse_block_list(&stripped);
    }

    fn parse_block_list(&mut self, input: &str) {
        let bytes = input.as_bytes();
        let mut i = 0usize;

        while i < bytes.len() {
            while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b';') {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }

            let Some(open_rel) = find_char_outside_quotes(&input[i..], '{') else {
                break;
            };
            let header = input[i..i + open_rel].trim();
            let body_start = i + open_rel + 1;
            let Some(body_end) = find_matching_brace(input, body_start) else {
                break;
            };
            let body = &input[body_start..body_end];
            i = body_end + 1;

            if header.starts_with('@') {
                let header_lower = header.to_ascii_lowercase();
                if header_lower.starts_with("@media") || header_lower.starts_with("@supports") {
                    if self.eval_at_rule_condition(&header_lower) {
                        self.parse_block_list(body);
                    }
                }
                continue;
            }

            let declarations = self.parse_declarations(body);
            if declarations.is_empty() {
                continue;
            }

            for sel_str in split_top_level_commas(header) {
                if let Some(complex) = parse_complex_selector(sel_str) {
                    let spec = compute_complex_specificity(&complex);
                    let order = self.next_order;
                    self.next_order += 1;

                    let rule_idx = self.rules.len();
                    let subject = complex.parts.last().map(|(_, c)| c);
                    if let Some(subj) = subject {
                        if let Some(ref id) = subj.id {
                            self.id_buckets.entry(id.clone()).or_default().push(rule_idx);
                        } else if let Some(first_cls) = subj.classes.first() {
                            self.class_buckets
                                .entry(first_cls.clone())
                                .or_default()
                                .push(rule_idx);
                        } else if let Some(ref tag) = subj.tag {
                            self.tag_buckets
                                .entry(tag.clone())
                                .or_default()
                                .push(rule_idx);
                        } else {
                            self.universal_bucket.push(rule_idx);
                        }
                    }

                    self.rules.push(CssRule {
                        selector: complex,
                        specificity: spec,
                        source_order: order,
                        declarations: declarations.clone(),
                    });
                }
            }
        }
    }

    fn eval_at_rule_condition(&self, cond: &str) -> bool {
        if cond.contains("prefers-color-scheme: dark") && !self.dark_mode {
            return false;
        }
        if cond.contains("prefers-color-scheme: light") && self.dark_mode {
            return false;
        }
        if let Some(idx) = cond.find("max-width:") {
            if let Some(px) = parse_numeric_px(cond[idx + 10..].split(')').next().unwrap_or("")) {
                if self.viewport_width > px {
                    return false;
                }
            }
        }
        if let Some(idx) = cond.find("min-width:") {
            if let Some(px) = parse_numeric_px(cond[idx + 10..].split(')').next().unwrap_or("")) {
                if self.viewport_width < px {
                    return false;
                }
            }
        }
        true
    }

    fn parse_declarations(&mut self, decl_block: &str) -> Vec<(String, String)> {
        let mut decls = Vec::new();
        for pair in split_top_level_semicolons(decl_block) {
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
                    decls.push((prop.to_ascii_lowercase(), val));
                }
            }
        }
        decls
    }

    pub fn compute_style_with_parent(
        &self,
        dom: &ZeroCopyDomArena,
        node_id: NodeId,
        parent_style: Option<&ComputedStyle>,
    ) -> ComputedStyle {
        let mut style = match parent_style {
            Some(p) => ComputedStyle::inherit_from(p),
            None => ComputedStyle::default(),
        };

        let Some(tag) = dom.node_tag(node_id) else {
            return style;
        };

        match tag {
            "head" | "script" | "style" | "meta" | "link" | "title" | "noscript" | "template" => {
                style.display = DisplayMode::None;
                return style;
            }
            "span" | "a" | "strong" | "b" | "em" | "i" | "small" | "code" | "label" => {
                style.display = DisplayMode::Inline;
            }
            "img" | "video" | "audio" | "svg" => {
                style.display = DisplayMode::InlineBlock;
            }
            _ => {
                style.display = DisplayMode::Block;
            }
        }

        match tag {
            "h1" => {
                style.font_size_sp = 26.0;
                style.font_weight_bold = true;
                style.margin_top = 14.0;
                style.margin_bottom = 10.0;
            }
            "h2" => {
                style.font_size_sp = 21.0;
                style.font_weight_bold = true;
                style.margin_top = 12.0;
                style.margin_bottom = 8.0;
            }
            "h3" => {
                style.font_size_sp = 18.0;
                style.font_weight_bold = true;
                style.margin_top = 10.0;
                style.margin_bottom = 6.0;
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
                style.padding_top = 4.0;
                style.padding_bottom = 4.0;
                style.padding_left = 6.0;
                style.padding_right = 6.0;
                style.corner_radius = 8.0;
            }
            "blockquote" => {
                style.background_color = 0xFF161922;
                style.text_color = 0xFF94A3B8;
                style.padding_top = 10.0;
                style.padding_bottom = 10.0;
                style.padding_left = 12.0;
                style.padding_right = 12.0;
            }
            "input" | "textarea" | "select" => {
                style.background_color = 0xFF161922;
                style.border_color = 0xFF334155;
                style.border_width = 1.0;
                style.text_color = 0xFFF8FAFC;
                style.padding_top = 8.0;
                style.padding_bottom = 8.0;
                style.padding_left = 10.0;
                style.padding_right = 10.0;
                style.corner_radius = 10.0;
                style.margin_top = 6.0;
                style.margin_bottom = 6.0;
            }
            "button" => {
                style.background_color = 0xFF1E3252;
                style.text_color = 0xFFD3E3FD;
                style.font_weight_bold = true;
                style.padding_top = 8.0;
                style.padding_bottom = 8.0;
                style.padding_left = 12.0;
                style.padding_right = 12.0;
                style.corner_radius = 12.0;
            }
            _ => {}
        }

        let mut candidates: Vec<usize> = Vec::with_capacity(16);
        if let Some(id) = dom.node_id_attr(node_id) {
            if let Some(list) = self.id_buckets.get(id) {
                candidates.extend_from_slice(list);
            }
        }
        for cls in dom.node_classes(node_id) {
            if let Some(list) = self.class_buckets.get(cls) {
                candidates.extend_from_slice(list);
            }
        }
        if let Some(list) = self.tag_buckets.get(tag) {
            candidates.extend_from_slice(list);
        }
        candidates.extend_from_slice(&self.universal_bucket);

        let mut matched: Vec<&CssRule> = Vec::new();
        candidates.sort_unstable();
        candidates.dedup();
        for idx in candidates {
            let rule = &self.rules[idx];
            if complex_selector_matches(&rule.selector, dom, node_id) {
                matched.push(rule);
            }
        }
        matched.sort_by_key(|r| (r.specificity, r.source_order));

        for rule in matched {
            self.apply_declarations(&mut style, &rule.declarations);
        }

        if let Some(inline) = dom.node_inline_style(node_id) {
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

    pub fn compute_style(&self, dom: &ZeroCopyDomArena, node_id: NodeId) -> ComputedStyle {
        self.compute_style_with_parent(dom, node_id, None)
    }

    fn apply_declarations(&self, style: &mut ComputedStyle, decls: &[(String, String)]) {
        for (prop, raw_val) in decls {
            let val = self.resolve_var(raw_val);
            match prop.as_str() {
                "display" => match val.to_ascii_lowercase().as_str() {
                    "none" => style.display = DisplayMode::None,
                    "flex" | "inline-flex" => style.display = DisplayMode::Flex,
                    "grid" | "inline-grid" => style.display = DisplayMode::Grid,
                    "inline" => style.display = DisplayMode::Inline,
                    "inline-block" => style.display = DisplayMode::InlineBlock,
                    _ => style.display = DisplayMode::Block,
                },
                "position" => match val.to_ascii_lowercase().as_str() {
                    "relative" => style.position = PositionMode::Relative,
                    "absolute" => style.position = PositionMode::Absolute,
                    "fixed" | "sticky" => style.position = PositionMode::Fixed,
                    _ => style.position = PositionMode::Static,
                },
                "flex-direction" => {
                    if val.eq_ignore_ascii_case("column") {
                        style.flex_direction = FlexDir::Column;
                    } else {
                        style.flex_direction = FlexDir::Row;
                    }
                }
                "flex-wrap" => {
                    style.flex_wrap = val.eq_ignore_ascii_case("wrap");
                }
                "justify-content" => match val.to_ascii_lowercase().as_str() {
                    "center" => style.justify_content = JustifyContent::Center,
                    "flex-end" | "end" | "right" => style.justify_content = JustifyContent::FlexEnd,
                    "space-between" => style.justify_content = JustifyContent::SpaceBetween,
                    "space-around" | "space-evenly" => style.justify_content = JustifyContent::SpaceAround,
                    _ => style.justify_content = JustifyContent::FlexStart,
                },
                "grid-template-columns" => {
                    style.grid_columns = parse_grid_column_count(&val);
                }
                "gap" | "grid-gap" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.gap = px.clamp(0.0, 64.0);
                    }
                }
                "flex-grow" | "flex" => {
                    if let Some(first) = val.split_whitespace().next() {
                        if let Ok(fg) = first.parse::<f32>() {
                            style.flex_grow = fg.clamp(0.0, 16.0);
                        }
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
                "border-width" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.border_width = px.clamp(0.0, 16.0);
                    }
                }
                "border" => {
                    for part in val.split_whitespace() {
                        if let Some(px) = parse_numeric_px(part) {
                            style.border_width = px.clamp(0.0, 16.0);
                        } else if let Some(c) = parse_css_color(part) {
                            style.border_color = c;
                        }
                    }
                }
                "font-size" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.font_size_sp = px.clamp(10.0, 64.0);
                    }
                }
                "font-weight" => {
                    if val.contains("bold") || matches!(val.as_str(), "600" | "700" | "800" | "900")
                    {
                        style.font_weight_bold = true;
                    } else if val == "normal" || val == "400" {
                        style.font_weight_bold = false;
                    }
                }
                "text-align" => match val.to_ascii_lowercase().as_str() {
                    "center" => style.text_align = TextAlign::Center,
                    "right" | "end" => style.text_align = TextAlign::Right,
                    _ => style.text_align = TextAlign::Left,
                },
                "line-height" => {
                    if let Ok(mult) = val.parse::<f32>() {
                        style.line_height_mult = mult.clamp(1.0, 3.0);
                    }
                }
                "padding" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        let p = px.clamp(0.0, 64.0);
                        style.padding_top = p;
                        style.padding_bottom = p;
                        style.padding_left = p;
                        style.padding_right = p;
                    }
                }
                "padding-top" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.padding_top = px.clamp(0.0, 64.0);
                    }
                }
                "padding-bottom" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.padding_bottom = px.clamp(0.0, 64.0);
                    }
                }
                "padding-left" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.padding_left = px.clamp(0.0, 64.0);
                    }
                }
                "padding-right" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.padding_right = px.clamp(0.0, 64.0);
                    }
                }
                "margin" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        let m = px.clamp(0.0, 64.0);
                        style.margin_top = m;
                        style.margin_bottom = m;
                        style.margin_left = m;
                        style.margin_right = m;
                    }
                }
                "margin-top" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.margin_top = px.clamp(0.0, 64.0);
                    }
                }
                "margin-bottom" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.margin_bottom = px.clamp(0.0, 64.0);
                    }
                }
                "width" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.explicit_width = Some(px.clamp(16.0, 2000.0));
                    }
                }
                "max-width" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.max_width = Some(px.clamp(16.0, 2000.0));
                    }
                }
                "height" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.explicit_height = Some(px.clamp(16.0, 1200.0));
                    }
                }
                "min-height" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.min_height = Some(px.clamp(16.0, 1200.0));
                    }
                }
                "z-index" => {
                    if let Ok(z) = val.parse::<i32>() {
                        style.z_index = z;
                    }
                }
                "overflow" => {
                    style.overflow_hidden = val.eq_ignore_ascii_case("hidden");
                }
                "border-radius" => {
                    if let Some(px) = parse_numeric_px(&val) {
                        style.corner_radius = px.clamp(0.0, 64.0);
                    }
                }
                "opacity" => {
                    if let Ok(op) = val.parse::<f32>() {
                        style.opacity = op.clamp(0.0, 1.0);
                    }
                }
                "visibility" => {
                    style.visibility_hidden = val.eq_ignore_ascii_case("hidden")
                        || val.eq_ignore_ascii_case("collapse");
                }
                _ => {}
            }
        }
    }

    fn resolve_var(&self, val: &str) -> String {
        if let Some(inner) = val.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
            let mut parts = inner.splitn(2, ',');
            let var_name = parts.next().unwrap_or("").trim();
            if let Some(resolved) = self.variables.get(var_name) {
                return resolved.clone();
            }
            if let Some(fallback) = parts.next() {
                return fallback.trim().to_string();
            }
        }
        val.to_string()
    }

    pub fn clear(&mut self) {
        self.rules.clear();
        self.rules.shrink_to_fit();
        self.id_buckets.clear();
        self.class_buckets.clear();
        self.tag_buckets.clear();
        self.universal_bucket.clear();
        self.variables.clear();
        self.next_order = 0;
    }
}

fn parse_grid_column_count(raw: &str) -> u16 {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(inner) = lower
        .strip_prefix("repeat(")
        .and_then(|s| s.strip_suffix(')'))
    {
        if let Some(cnt_str) = inner.split(',').next() {
            if let Ok(n) = cnt_str.trim().parse::<u16>() {
                return n.clamp(1, 12);
            }
        }
    }
    let count = lower.split_whitespace().count() as u16;
    count.clamp(1, 12)
}

fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0usize;
    let mut quote = 0u8;
    while i < bytes.len() {
        let b = bytes[i];
        if quote != 0 {
            out.push(b as char);
            if b == quote && (i == 0 || bytes[i - 1] != b'\\') {
                quote = 0;
            }
            i += 1;
        } else if b == b'"' || b == b'\'' {
            quote = b;
            out.push(b as char);
            i += 1;
        } else if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            let ch = css[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn find_char_outside_quotes(s: &str, target: char) -> Option<usize> {
    let mut quote = '\0';
    for (idx, c) in s.char_indices() {
        if quote != '\0' {
            if c == quote {
                quote = '\0';
            }
        } else if c == '"' || c == '\'' {
            quote = c;
        } else if c == target {
            return Some(idx);
        }
    }
    None
}

fn find_matching_brace(s: &str, start_after_open: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut quote = '\0';
    for (rel_idx, c) in s[start_after_open..].char_indices() {
        if quote != '\0' {
            if c == quote {
                quote = '\0';
            }
        } else if c == '"' || c == '\'' {
            quote = c;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(start_after_open + rel_idx);
            }
        }
    }
    None
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut paren = 0usize;
    let mut bracket = 0usize;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            ',' if paren == 0 && bracket == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        out.push(s[start..].trim());
    }
    out
}

fn split_top_level_semicolons(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut paren = 0usize;
    let mut start = 0usize;
    let mut quote = '\0';
    for (i, c) in s.char_indices() {
        if quote != '\0' {
            if c == quote {
                quote = '\0';
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = c,
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            ';' if paren == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        out.push(s[start..].trim());
    }
    out
}

fn parse_complex_selector(raw: &str) -> Option<ComplexSelector> {
    let mut parts = Vec::new();
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut paren = 0usize;
    let mut bracket = 0usize;

    for c in raw.trim().chars() {
        if c == '(' {
            paren += 1;
            buf.push(c);
        } else if c == ')' {
            paren = paren.saturating_sub(1);
            buf.push(c);
        } else if c == '[' {
            bracket += 1;
            buf.push(c);
        } else if c == ']' {
            bracket = bracket.saturating_sub(1);
            buf.push(c);
        } else if paren == 0 && bracket == 0 && (c == '>' || c == '+' || c == '~' || c.is_whitespace()) {
            if !buf.is_empty() {
                tokens.push(std::mem::take(&mut buf));
            }
            if c == '>' || c == '+' || c == '~' {
                tokens.push(c.to_string());
            } else if tokens.last().map(|s| s.as_str()) != Some(" ") && !tokens.is_empty() {
                tokens.push(" ".to_string());
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        tokens.push(buf);
    }

    let mut pending_comb: Option<Combinator> = None;
    for tok in tokens {
        match tok.as_str() {
            " " => {
                if pending_comb.is_none() {
                    pending_comb = Some(Combinator::Descendant);
                }
            }
            ">" => pending_comb = Some(Combinator::Child),
            "+" => pending_comb = Some(Combinator::AdjacentSibling),
            "~" => pending_comb = Some(Combinator::GeneralSibling),
            compound_str => {
                let comp = parse_compound_selector(compound_str)?;
                let comb = if parts.is_empty() {
                    None
                } else {
                    pending_comb.or(Some(Combinator::Descendant))
                };
                parts.push((comb, comp));
                pending_comb = None;
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(ComplexSelector { parts })
    }
}

fn parse_compound_selector(s: &str) -> Option<CompoundSelector> {
    if s.is_empty() {
        return None;
    }
    let mut comp = CompoundSelector::default();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        match chars[i] {
            '*' => {
                i += 1;
            }
            '#' => {
                i += 1;
                let start = i;
                while i < chars.len() && !matches!(chars[i], '.' | '#' | '[' | ':') {
                    i += 1;
                }
                let id: String = chars[start..i].iter().collect();
                if !id.is_empty() {
                    comp.id = Some(id);
                }
            }
            '.' => {
                i += 1;
                let start = i;
                while i < chars.len() && !matches!(chars[i], '.' | '#' | '[' | ':') {
                    i += 1;
                }
                let cls: String = chars[start..i].iter().collect();
                if !cls.is_empty() {
                    comp.classes.push(cls);
                }
            }
            '[' => {
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                let inner: String = chars[start..i].iter().collect();
                if i < chars.len() && chars[i] == ']' {
                    i += 1;
                }
                if let Some(attr) = parse_attr_cond(&inner) {
                    comp.attrs.push(attr);
                }
            }
            ':' => {
                i += 1;
                if i < chars.len() && chars[i] == ':' {
                    while i < chars.len() && !matches!(chars[i], '.' | '#' | '[') {
                        i += 1;
                    }
                    continue;
                }
                let start = i;
                let mut p_depth = 0usize;
                while i < chars.len() {
                    if chars[i] == '(' {
                        p_depth += 1;
                    } else if chars[i] == ')' {
                        if p_depth == 0 {
                            break;
                        }
                        p_depth -= 1;
                        if p_depth == 0 {
                            i += 1;
                            break;
                        }
                    } else if p_depth == 0 && matches!(chars[i], '.' | '#' | '[' | ':') {
                        break;
                    }
                    i += 1;
                }
                let ps: String = chars[start..i].iter().collect();
                if let Some(pseudo) = parse_pseudo_class(&ps) {
                    comp.pseudos.push(pseudo);
                }
            }
            _ => {
                let start = i;
                while i < chars.len() && !matches!(chars[i], '.' | '#' | '[' | ':') {
                    i += 1;
                }
                let tag: String = chars[start..i].iter().collect();
                if !tag.is_empty() {
                    comp.tag = Some(tag.to_ascii_lowercase());
                }
            }
        }
    }
    Some(comp)
}

fn parse_attr_cond(inner: &str) -> Option<AttrCond> {
    let trimmed = inner.trim();
    for (sep, make_op) in [
        ("^=", (|v: String| AttrOp::StartsWith(v)) as fn(String) -> AttrOp),
        ("$=", |v| AttrOp::EndsWith(v)),
        ("*=", |v| AttrOp::Contains(v)),
        ("=", |v| AttrOp::Equals(v)),
    ] {
        if let Some((k, v)) = trimmed.split_once(sep) {
            let clean_v = v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            return Some(AttrCond {
                name: k.trim().to_ascii_lowercase(),
                op: make_op(clean_v),
            });
        }
    }
    if !trimmed.is_empty() {
        Some(AttrCond {
            name: trimmed.to_ascii_lowercase(),
            op: AttrOp::Present,
        })
    } else {
        None
    }
}

fn parse_pseudo_class(raw: &str) -> Option<PseudoClass> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(inner) = lower.strip_prefix("not(").and_then(|s| s.strip_suffix(')')) {
        let sub = parse_compound_selector(inner.trim())?;
        return Some(PseudoClass::Not(Box::new(sub)));
    }
    if let Some(inner) = lower
        .strip_prefix("nth-child(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return match inner.trim() {
            "even" => Some(PseudoClass::NthEven),
            "odd" => Some(PseudoClass::NthOdd),
            n => n.parse::<usize>().ok().map(PseudoClass::NthExact),
        };
    }
    match lower.as_str() {
        "root" => Some(PseudoClass::Root),
        "first-child" => Some(PseudoClass::FirstChild),
        "last-child" => Some(PseudoClass::LastChild),
        "empty" => Some(PseudoClass::Empty),
        "checked" => Some(PseudoClass::Checked),
        "disabled" => Some(PseudoClass::Disabled),
        "enabled" => Some(PseudoClass::Enabled),
        "focus" => Some(PseudoClass::Focus),
        _ => None,
    }
}

fn compute_complex_specificity(sel: &ComplexSelector) -> (u16, u16, u16) {
    let mut ids = 0u16;
    let mut classes = 0u16;
    let mut tags = 0u16;
    for (_, comp) in &sel.parts {
        if comp.id.is_some() {
            ids += 1;
        }
        classes += (comp.classes.len() + comp.attrs.len() + comp.pseudos.len()) as u16;
        if comp.tag.is_some() {
            tags += 1;
        }
    }
    (ids, classes, tags)
}

fn complex_selector_matches(
    sel: &ComplexSelector,
    dom: &ZeroCopyDomArena,
    node_id: NodeId,
) -> bool {
    if sel.parts.is_empty() {
        return false;
    }
    let last_idx = sel.parts.len() - 1;
    if !compound_matches(&sel.parts[last_idx].1, dom, node_id) {
        return false;
    }
    match_chain_backward(&sel.parts, last_idx, dom, node_id)
}

fn match_chain_backward(
    parts: &[(Option<Combinator>, CompoundSelector)],
    idx: usize,
    dom: &ZeroCopyDomArena,
    current_id: NodeId,
) -> bool {
    if idx == 0 {
        return true;
    }
    let comb = parts[idx].0.unwrap_or(Combinator::Descendant);
    let target_comp = &parts[idx - 1].1;

    match comb {
        Combinator::Child => {
            let parent = dom.nodes()[current_id as usize].parent;
            if parent == NO_NODE {
                return false;
            }
            compound_matches(target_comp, dom, parent)
                && match_chain_backward(parts, idx - 1, dom, parent)
        }
        Combinator::Descendant => {
            let mut anc = dom.nodes()[current_id as usize].parent;
            while anc != NO_NODE {
                if compound_matches(target_comp, dom, anc)
                    && match_chain_backward(parts, idx - 1, dom, anc)
                {
                    return true;
                }
                anc = dom.nodes()[anc as usize].parent;
            }
            false
        }
        Combinator::AdjacentSibling => {
            if let Some(prev) = prev_element_sibling(dom, current_id) {
                compound_matches(target_comp, dom, prev)
                    && match_chain_backward(parts, idx - 1, dom, prev)
            } else {
                false
            }
        }
        Combinator::GeneralSibling => {
            let mut cur = current_id;
            while let Some(prev) = prev_element_sibling(dom, cur) {
                if compound_matches(target_comp, dom, prev)
                    && match_chain_backward(parts, idx - 1, dom, prev)
                {
                    return true;
                }
                cur = prev;
            }
            false
        }
    }
}

fn compound_matches(comp: &CompoundSelector, dom: &ZeroCopyDomArena, node_id: NodeId) -> bool {
    let Some(tag) = dom.node_tag(node_id) else {
        return false;
    };
    if let Some(ref req_tag) = comp.tag {
        if tag != req_tag {
            return false;
        }
    }
    if let Some(ref req_id) = comp.id {
        if dom.node_id_attr(node_id) != Some(req_id.as_str()) {
            return false;
        }
    }
    for cls in &comp.classes {
        if !dom.node_has_class(node_id, cls) {
            return false;
        }
    }
    for attr in &comp.attrs {
        let Some(val) = dom.get_attribute(node_id, &attr.name) else {
            return false;
        };
        match &attr.op {
            AttrOp::Present => {}
            AttrOp::Equals(v) => {
                if &val != v {
                    return false;
                }
            }
            AttrOp::StartsWith(v) => {
                if !val.starts_with(v.as_str()) {
                    return false;
                }
            }
            AttrOp::EndsWith(v) => {
                if !val.ends_with(v.as_str()) {
                    return false;
                }
            }
            AttrOp::Contains(v) => {
                if !val.contains(v.as_str()) {
                    return false;
                }
            }
        }
    }
    for pseudo in &comp.pseudos {
        if !pseudo_matches(pseudo, dom, node_id) {
            return false;
        }
    }
    true
}

fn pseudo_matches(pseudo: &PseudoClass, dom: &ZeroCopyDomArena, node_id: NodeId) -> bool {
    match pseudo {
        PseudoClass::Root => dom.node_tag(node_id) == Some("html"),
        PseudoClass::FirstChild => prev_element_sibling(dom, node_id).is_none(),
        PseudoClass::LastChild => next_element_sibling(dom, node_id).is_none(),
        PseudoClass::Empty => dom.nodes()[node_id as usize].first_child == NO_NODE,
        PseudoClass::Checked => dom.forms.get_control(node_id).map(|c| c.checked).unwrap_or(false),
        PseudoClass::Disabled => dom.forms.get_control(node_id).map(|c| c.disabled).unwrap_or(false),
        PseudoClass::Enabled => dom.forms.get_control(node_id).map(|c| !c.disabled).unwrap_or(false),
        PseudoClass::Focus => dom.forms.focused_node() == Some(node_id),
        PseudoClass::NthEven => element_child_ordinal(dom, node_id) % 2 == 0,
        PseudoClass::NthOdd => element_child_ordinal(dom, node_id) % 2 == 1,
        PseudoClass::NthExact(n) => element_child_ordinal(dom, node_id) == *n,
        PseudoClass::Not(sub) => !compound_matches(sub, dom, node_id),
    }
}

fn prev_element_sibling(dom: &ZeroCopyDomArena, node_id: NodeId) -> Option<NodeId> {
    let parent = dom.nodes()[node_id as usize].parent;
    if parent == NO_NODE {
        return None;
    }
    let mut cur = dom.nodes()[parent as usize].first_child;
    let mut last_elem = None;
    while cur != NO_NODE && cur != node_id {
        if matches!(dom.nodes()[cur as usize].kind, NodeKind::Element { .. }) {
            last_elem = Some(cur);
        }
        cur = dom.nodes()[cur as usize].next_sibling;
    }
    last_elem
}

fn next_element_sibling(dom: &ZeroCopyDomArena, node_id: NodeId) -> Option<NodeId> {
    let mut cur = dom.nodes()[node_id as usize].next_sibling;
    while cur != NO_NODE {
        if matches!(dom.nodes()[cur as usize].kind, NodeKind::Element { .. }) {
            return Some(cur);
        }
        cur = dom.nodes()[cur as usize].next_sibling;
    }
    None
}

fn element_child_ordinal(dom: &ZeroCopyDomArena, node_id: NodeId) -> usize {
    let parent = dom.nodes()[node_id as usize].parent;
    if parent == NO_NODE {
        return 1;
    }
    let mut ord = 1usize;
    let mut cur = dom.nodes()[parent as usize].first_child;
    while cur != NO_NODE && cur != node_id {
        if matches!(dom.nodes()[cur as usize].kind, NodeKind::Element { .. }) {
            ord += 1;
        }
        cur = dom.nodes()[cur as usize].next_sibling;
    }
    ord
}

pub fn parse_css_color(raw: &str) -> Option<u32> {
    let v = raw.trim();
    if let Some(hex) = v.strip_prefix('#') {
        if hex.len() == 6 {
            let rgb = u32::from_str_radix(hex, 16).ok()?;
            return Some(0xFF00_0000 | rgb);
        } else if hex.len() == 8 {
            let rgba = u32::from_str_radix(hex, 16).ok()?;
            let a = rgba & 0xFF;
            let rgb = rgba >> 8;
            return Some((a << 24) | rgb);
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
        let parts: Vec<&str> = rgb_inner.split(',').collect();
        if parts.len() >= 3 {
            let r = parts[0].trim().parse::<u32>().ok()?.min(255);
            let g = parts[1].trim().parse::<u32>().ok()?.min(255);
            let b = parts[2].trim().parse::<u32>().ok()?.min(255);
            let a = if parts.len() >= 4 {
                ((parts[3].trim().parse::<f32>().unwrap_or(1.0).clamp(0.0, 1.0)) * 255.0) as u32
            } else {
                255
            };
            return Some((a << 24) | (r << 16) | (g << 8) | b);
        }
    }
    match v.to_ascii_lowercase().as_str() {
        "black" => Some(0xFF000000),
        "white" => Some(0xFFFFFFFF),
        "red" => Some(0xFFEF4444),
        "green" => Some(0xFF10B981),
        "blue" => Some(0xFF3B82F6),
        "gray" | "grey" => Some(0xFF64748B),
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
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_css_combinators_grid_and_media_queries() {
        let mut dom = ZeroCopyDomArena::new(64 * 1024);
        let mut tok = StreamingHtmlTokenizer::new();
        tok.feed_chunk(
            b"<div id=\"hero\" class=\"card active\" style=\"display: grid; grid-template-columns: repeat(3, 1fr);\"><span><em id=\"em1\">Nested</em></span></div>",
            true,
            |t| dom.ingest_token(t),
        );

        let mut css = MicroCssEngine::new();
        css.parse_stylesheet(
            r#"
            @media (prefers-color-scheme: dark) {
                #hero.card.active { color: #00FF00; gap: 12px; }
            }
            div#hero span > em:first-child { color: #3B82F6; }
            "#,
        );

        let hero_id = dom.find_by_id("hero").unwrap();
        let hero_style = css.compute_style(&dom, hero_id);
        assert_eq!(hero_style.text_color, 0xFF00FF00);
        assert_eq!(hero_style.display, DisplayMode::Grid);
        assert_eq!(hero_style.grid_columns, 3);

        let em_id = dom.find_by_id("em1").unwrap();
        let em_style = css.compute_style(&dom, em_id);
        assert_eq!(em_style.text_color, 0xFF3B82F6);
    }
}
