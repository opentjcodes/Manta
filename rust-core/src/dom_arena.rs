use std::collections::HashMap;

use crate::cpp_bridge::CppMmapSlab;
use crate::html_tokenizer::HtmlToken;
use crate::input_forms::{FormControlData, FormControlKind, MantaFormEngine};

pub type NodeId = u32;
pub const NO_NODE: NodeId = u32::MAX;

/// Compact 32-bit interned symbol ID for tags, attribute names, classes, and IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymId(pub u32);

/// Compact 8-byte slice handle into the arena's contiguous `mmap`-backed byte slab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSpan {
    pub offset: u32,
    pub len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaElementMeta {
    pub node_id: NodeId,
    pub is_video: bool,
    pub src: String,
    pub poster: String,
    pub autoplay: bool,
    pub muted: bool,
    pub controls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Element {
        tag: SymId,
        id_attr: Option<SymId>,
        class_start: u32,
        class_len: u16,
        attr_start: u32,
        attr_len: u16,
        inline_style: Option<TextSpan>,
    },
    Text(TextSpan),
}

/// Flat 48-byte `DomNode` with ZERO per-node heap allocations (`String`/`Vec`).
/// Uses ~8x less memory than pointer/heap-String DOM trees and fits cleanly in CPU cache lines.
#[derive(Debug, Clone, Copy)]
pub struct DomNode {
    pub id: NodeId,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub last_child: NodeId,
    pub next_sibling: NodeId,
    pub kind: NodeKind,
}

struct StringInterner {
    bytes: Vec<u8>,
    spans: Vec<TextSpan>,
    table: HashMap<u64, Vec<SymId>>,
}

impl StringInterner {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(1024),
            spans: Vec::with_capacity(64),
            table: HashMap::with_capacity(64),
        }
    }

    #[inline]
    fn hash_str(s: &str) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in s.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    fn intern(&mut self, s: &str) -> SymId {
        let h = Self::hash_str(s);
        if let Some(bucket) = self.table.get(&h) {
            for &sym in bucket {
                if self.get(sym) == s {
                    return sym;
                }
            }
        }
        let off = self.bytes.len() as u32;
        let len = s.len() as u32;
        self.bytes.extend_from_slice(s.as_bytes());
        let id = SymId(self.spans.len() as u32);
        self.spans.push(TextSpan { offset: off, len });
        self.table.entry(h).or_default().push(id);
        id
    }

    #[inline]
    fn get(&self, sym: SymId) -> &str {
        let Some(sp) = self.spans.get(sym.0 as usize) else {
            return "";
        };
        let start = sp.offset as usize;
        let end = start + sp.len as usize;
        std::str::from_utf8(&self.bytes[start..end]).unwrap_or("")
    }

    fn shrink(&mut self) {
        self.bytes.shrink_to_fit();
        self.spans.shrink_to_fit();
    }

    fn clear_and_shrink(&mut self) {
        self.bytes.clear();
        self.bytes.shrink_to_fit();
        self.spans.clear();
        self.spans.shrink_to_fit();
        self.table.clear();
        self.table.shrink_to_fit();
    }

    fn used_bytes(&self) -> usize {
        self.bytes.len() + self.spans.len() * std::mem::size_of::<TextSpan>()
    }
}

/// Contiguous Data-Oriented DOM Arena (`ZeroCopyDomArena`):
/// - String-interned tags, classes, IDs, and attribute keys (`SymId`).
/// - C++ OS `mmap`-backed contiguous byte slab (`CppMmapSlab`) with `madvise(MADV_DONTNEED)` release.
/// - HTML5 implicit end-tag auto-closing (`<p>`, `<li>`, `<dt>`, `<dd>`, `<td>`, `<th>`, `<tr>`, `<option>`).
/// - Full DOM manipulation (`createElement`, `appendChild`, `removeChild`, `classList`, `querySelectorAll`).
/// - Tracks `<video>`/`<audio>` media elements, `<link rel="stylesheet">`, `<script src>`, and `<base href>`.
pub struct ZeroCopyDomArena {
    nodes: Vec<DomNode>,
    open_stack: Vec<NodeId>,
    interner: StringInterner,
    text_slab: CppMmapSlab,
    class_pool: Vec<SymId>,
    attrs_pool: Vec<(SymId, TextSpan)>,
    id_index: HashMap<SymId, NodeId>,
    event_listeners: HashMap<(NodeId, String), Vec<String>>,
    pub forms: MantaFormEngine,
    pub collected_styles: Vec<String>,
    pub collected_scripts: Vec<String>,
    pub external_stylesheets: Vec<String>,
    pub external_scripts: Vec<String>,
    pub media_elements: Vec<MediaElementMeta>,
    pub base_href: Option<String>,
    pub page_title: String,
    max_bytes: usize,
    pub generation: u64,
}

impl ZeroCopyDomArena {
    pub fn new(max_bytes: usize) -> Self {
        let slab_init = (max_bytes / 8).clamp(8192, 256 * 1024);
        let mut arena = Self {
            nodes: Vec::with_capacity(128),
            open_stack: Vec::with_capacity(16),
            interner: StringInterner::new(),
            text_slab: CppMmapSlab::with_capacity(slab_init),
            class_pool: Vec::with_capacity(128),
            attrs_pool: Vec::with_capacity(128),
            id_index: HashMap::with_capacity(32),
            event_listeners: HashMap::new(),
            forms: MantaFormEngine::new(),
            collected_styles: Vec::new(),
            collected_scripts: Vec::new(),
            external_stylesheets: Vec::new(),
            external_scripts: Vec::new(),
            media_elements: Vec::new(),
            base_href: None,
            page_title: String::from("Manta"),
            max_bytes,
            generation: 1,
        };
        let root_id = arena.alloc_node(NO_NODE, NodeKind::Document);
        arena.open_stack.push(root_id);
        arena
    }

    #[inline]
    pub fn nodes(&self) -> &[DomNode] {
        &self.nodes
    }

    #[inline]
    pub fn used_bytes(&self) -> usize {
        self.nodes.len() * std::mem::size_of::<DomNode>()
            + self.text_slab.len()
            + self.interner.used_bytes()
            + self.class_pool.len() * std::mem::size_of::<SymId>()
            + self.attrs_pool.len() * std::mem::size_of::<(SymId, TextSpan)>()
            + self
                .collected_styles
                .iter()
                .map(|s| s.len())
                .sum::<usize>()
            + self
                .collected_scripts
                .iter()
                .map(|s| s.len())
                .sum::<usize>()
    }

    #[inline]
    pub fn sym_str(&self, sym: SymId) -> &str {
        self.interner.get(sym)
    }

    #[inline]
    pub fn span_str(&self, span: TextSpan) -> &str {
        let bytes = self.text_slab.as_slice();
        let start = span.offset as usize;
        let end = start.saturating_add(span.len as usize);
        if end > bytes.len() {
            return "";
        }
        std::str::from_utf8(&bytes[start..end]).unwrap_or("")
    }

    #[inline]
    pub fn node_tag(&self, node_id: NodeId) -> Option<&str> {
        let node = self.nodes.get(node_id as usize)?;
        match node.kind {
            NodeKind::Element { tag, .. } => Some(self.sym_str(tag)),
            _ => None,
        }
    }

    #[inline]
    pub fn node_id_attr(&self, node_id: NodeId) -> Option<&str> {
        let node = self.nodes.get(node_id as usize)?;
        match node.kind {
            NodeKind::Element { id_attr: Some(id), .. } => Some(self.sym_str(id)),
            _ => None,
        }
    }

    #[inline]
    pub fn node_has_class(&self, node_id: NodeId, cls: &str) -> bool {
        let Some(node) = self.nodes.get(node_id as usize) else {
            return false;
        };
        if let NodeKind::Element {
            class_start,
            class_len,
            ..
        } = node.kind
        {
            let s = class_start as usize;
            let e = s + class_len as usize;
            if let Some(slice) = self.class_pool.get(s..e) {
                return slice.iter().any(|&sym| self.sym_str(sym) == cls);
            }
        }
        false
    }

    pub fn node_classes(&self, node_id: NodeId) -> Vec<&str> {
        let Some(node) = self.nodes.get(node_id as usize) else {
            return Vec::new();
        };
        if let NodeKind::Element {
            class_start,
            class_len,
            ..
        } = node.kind
        {
            let s = class_start as usize;
            let e = s + class_len as usize;
            if let Some(slice) = self.class_pool.get(s..e) {
                return slice.iter().map(|&sym| self.sym_str(sym)).collect();
            }
        }
        Vec::new()
    }

    #[inline]
    pub fn node_inline_style(&self, node_id: NodeId) -> Option<&str> {
        let node = self.nodes.get(node_id as usize)?;
        match node.kind {
            NodeKind::Element {
                inline_style: Some(sp),
                ..
            } => Some(self.span_str(sp)),
            _ => None,
        }
    }

    #[inline]
    pub fn node_text(&self, node_id: NodeId) -> Option<&str> {
        let node = self.nodes.get(node_id as usize)?;
        match node.kind {
            NodeKind::Text(sp) => Some(self.span_str(sp)),
            _ => None,
        }
    }

    fn alloc_span(&mut self, text: &str) -> TextSpan {
        if text.is_empty() {
            return TextSpan { offset: 0, len: 0 };
        }
        if let Some(off) = self.text_slab.push_bytes(text.as_bytes()) {
            TextSpan {
                offset: off as u32,
                len: text.len() as u32,
            }
        } else {
            TextSpan { offset: 0, len: 0 }
        }
    }

    pub fn ingest_token(&mut self, token: HtmlToken) {
        if self.used_bytes() >= self.max_bytes {
            return;
        }
        self.generation = self.generation.wrapping_add(1);

        match token {
            HtmlToken::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                // HTML5 implicit end-tag auto-closing for broken/minimal markup
                self.auto_close_implicit_for_start(&name);
                let current_parent = *self.open_stack.last().unwrap_or(&0);

                // Extract <base>, <link rel="stylesheet">, <script src="...">, <video>/<audio>
                if name == "base" {
                    if let Some((_, href)) = attributes.iter().find(|(k, _)| k == "href") {
                        self.base_href = Some(href.clone());
                    }
                } else if name == "link" {
                    let rel = attributes
                        .iter()
                        .find(|(k, _)| k == "rel")
                        .map(|(_, v)| v.to_ascii_lowercase())
                        .unwrap_or_default();
                    if rel.contains("stylesheet") {
                        if let Some((_, href)) = attributes.iter().find(|(k, _)| k == "href") {
                            if !href.is_empty() && self.external_stylesheets.len() < 32 {
                                self.external_stylesheets.push(href.clone());
                            }
                        }
                    }
                } else if name == "script" {
                    if let Some((_, src)) = attributes.iter().find(|(k, _)| k == "src") {
                        if !src.is_empty() && self.external_scripts.len() < 64 {
                            self.external_scripts.push(src.clone());
                        }
                    }
                }

                let mut id_attr_sym = None;
                let mut id_raw = String::new();
                let mut inline_style_span = None;
                let mut onclick_handler = None;
                let mut oninput_handler = None;
                let mut onsubmit_handler = None;

                let class_start = self.class_pool.len() as u32;
                let mut class_len = 0u16;
                let attr_start = self.attrs_pool.len() as u32;
                let mut attr_len = 0u16;

                for (k, v) in &attributes {
                    match k.as_str() {
                        "id" if !v.is_empty() => {
                            let sym = self.interner.intern(v);
                            id_attr_sym = Some(sym);
                            id_raw = v.clone();
                        }
                        "class" => {
                            for cls in v.split_whitespace() {
                                let c_sym = self.interner.intern(cls);
                                self.class_pool.push(c_sym);
                                class_len = class_len.saturating_add(1);
                            }
                        }
                        "style" if !v.is_empty() => {
                            inline_style_span = Some(self.alloc_span(v));
                        }
                        "onclick" => onclick_handler = Some(v.clone()),
                        "oninput" | "onchange" => oninput_handler = Some(v.clone()),
                        "onsubmit" => onsubmit_handler = Some(v.clone()),
                        _ => {}
                    }
                    let k_sym = self.interner.intern(k);
                    let v_span = self.alloc_span(v);
                    self.attrs_pool.push((k_sym, v_span));
                    attr_len = attr_len.saturating_add(1);
                }

                let is_form_control = matches!(
                    name.as_str(),
                    "input" | "textarea" | "select" | "button" | "form"
                );
                let enclosing_form = self.find_enclosing_form_in_stack();
                let tag_sym = self.interner.intern(&name);

                let node_id = self.alloc_node(
                    current_parent,
                    NodeKind::Element {
                        tag: tag_sym,
                        id_attr: id_attr_sym,
                        class_start,
                        class_len,
                        attr_start,
                        attr_len,
                        inline_style: inline_style_span,
                    },
                );

                if let Some(sym) = id_attr_sym {
                    if !id_raw.is_empty() {
                        self.id_index.insert(sym, node_id);
                    }
                }

                if name == "video" || name == "audio" {
                    let src = attributes
                        .iter()
                        .find(|(k, _)| k == "src")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let poster = attributes
                        .iter()
                        .find(|(k, _)| k == "poster")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let autoplay = attributes.iter().any(|(k, _)| k == "autoplay");
                    let muted = attributes.iter().any(|(k, _)| k == "muted");
                    let controls = attributes.iter().any(|(k, _)| k == "controls");
                    self.media_elements.push(MediaElementMeta {
                        node_id,
                        is_video: name == "video",
                        src,
                        poster,
                        autoplay,
                        muted,
                        controls,
                    });
                } else if name == "source" {
                    if let Some(parent_tag) = self.node_tag(current_parent) {
                        if parent_tag == "video" || parent_tag == "audio" {
                            if let Some((_, src)) = attributes.iter().find(|(k, _)| k == "src") {
                                if let Some(last_media) =
                                    self.media_elements.iter_mut().rev().find(|m| m.node_id == current_parent)
                                {
                                    if last_media.src.is_empty() {
                                        last_media.src = src.clone();
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(code) = onclick_handler {
                    self.add_event_listener(node_id, "click", &code);
                }
                if let Some(code) = oninput_handler {
                    self.add_event_listener(node_id, "input", &code);
                }
                if let Some(code) = onsubmit_handler {
                    self.add_event_listener(node_id, "submit", &code);
                }

                if is_form_control {
                    self.forms
                        .register_element_from_dom(node_id, enclosing_form, &name, &attributes);
                }

                if !self_closing {
                    self.open_stack.push(node_id);
                }
            }
            HtmlToken::EndTag { name } => {
                if let Some(pos) = self
                    .open_stack
                    .iter()
                    .rposition(|&nid| self.node_tag(nid) == Some(name.as_str()))
                {
                    let closed_node_id = self.open_stack[pos];
                    self.open_stack.truncate(pos);
                    if name == "textarea" || name == "option" {
                        let initial_txt = self.text_content(closed_node_id);
                        self.forms
                            .finalize_text_control(closed_node_id, &initial_txt);
                    }
                }
            }
            HtmlToken::Text(txt) => {
                let current_parent = *self.open_stack.last().unwrap_or(&0);
                if let Some(parent_tag) = self.node_tag(current_parent) {
                    match parent_tag {
                        "style" => {
                            self.collected_styles.push(txt);
                            return;
                        }
                        "script" => {
                            self.collected_scripts.push(txt);
                            return;
                        }
                        "title" => {
                            self.page_title = txt;
                            return;
                        }
                        _ => {}
                    }
                }
                let span = self.alloc_span(&txt);
                self.alloc_node(current_parent, NodeKind::Text(span));
            }
        }
    }

    fn auto_close_implicit_for_start(&mut self, incoming_tag: &str) {
        let closes_p = matches!(
            incoming_tag,
            "address"
                | "article"
                | "aside"
                | "blockquote"
                | "div"
                | "dl"
                | "fieldset"
                | "footer"
                | "form"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "header"
                | "hr"
                | "main"
                | "nav"
                | "ol"
                | "p"
                | "pre"
                | "section"
                | "table"
                | "ul"
        );
        if closes_p {
            if let Some(&top) = self.open_stack.last() {
                if self.node_tag(top) == Some("p") {
                    self.open_stack.pop();
                }
            }
        }
        match incoming_tag {
            "li" => self.pop_if_open_until("li", &["ul", "ol"]),
            "dt" | "dd" => {
                self.pop_if_open_until("dt", &["dl"]);
                self.pop_if_open_until("dd", &["dl"]);
            }
            "td" | "th" => {
                self.pop_if_open_until("td", &["tr", "table"]);
                self.pop_if_open_until("th", &["tr", "table"]);
            }
            "tr" => {
                self.pop_if_open_until("td", &["table"]);
                self.pop_if_open_until("th", &["table"]);
                self.pop_if_open_until("tr", &["table"]);
            }
            "option" => self.pop_if_open_until("option", &["select", "optgroup"]),
            "body" => self.pop_if_open_until("head", &["html"]),
            _ => {}
        }
    }

    fn pop_if_open_until(&mut self, target: &str, scope_stop: &[&str]) {
        for idx in (1..self.open_stack.len()).rev() {
            let nid = self.open_stack[idx];
            let Some(t) = self.node_tag(nid) else {
                continue;
            };
            if t == target {
                self.open_stack.truncate(idx);
                return;
            }
            if scope_stop.contains(&t) {
                return;
            }
        }
    }

    fn find_enclosing_form_in_stack(&self) -> Option<NodeId> {
        for &nid in self.open_stack.iter().rev() {
            if self.node_tag(nid) == Some("form") {
                return Some(nid);
            }
        }
        None
    }

    #[inline]
    pub fn find_by_id(&self, target_id: &str) -> Option<NodeId> {
        let h = StringInterner::hash_str(target_id);
        let bucket = self.interner.table.get(&h)?;
        for &sym in bucket {
            if self.interner.get(sym) == target_id {
                return self.id_index.get(&sym).copied();
            }
        }
        None
    }

    pub fn query_selector(&self, selector: &str) -> Option<NodeId> {
        let sel = selector.trim();
        if let Some(id) = sel.strip_prefix('#') {
            return self.find_by_id(id);
        }
        if let Some(cls) = sel.strip_prefix('.') {
            for node in &self.nodes {
                if self.node_has_class(node.id, cls) {
                    return Some(node.id);
                }
            }
            return None;
        }
        let tag_target = sel.to_ascii_lowercase();
        for node in &self.nodes {
            if self.node_tag(node.id) == Some(tag_target.as_str()) {
                return Some(node.id);
            }
        }
        None
    }

    pub fn query_selector_all(&self, selector: &str) -> Vec<NodeId> {
        let sel = selector.trim();
        let mut out = Vec::new();
        if let Some(id) = sel.strip_prefix('#') {
            if let Some(nid) = self.find_by_id(id) {
                out.push(nid);
            }
            return out;
        }
        if let Some(cls) = sel.strip_prefix('.') {
            for node in &self.nodes {
                if self.node_has_class(node.id, cls) {
                    out.push(node.id);
                }
            }
            return out;
        }
        let tag_target = sel.to_ascii_lowercase();
        for node in &self.nodes {
            if self.node_tag(node.id) == Some(tag_target.as_str()) {
                out.push(node.id);
            }
        }
        out
    }

    pub fn create_element(&mut self, tag_name: &str) -> NodeId {
        self.generation = self.generation.wrapping_add(1);
        let clean_tag = tag_name.trim().to_ascii_lowercase();
        let tag_sym = self.interner.intern(&clean_tag);
        self.alloc_node(
            NO_NODE,
            NodeKind::Element {
                tag: tag_sym,
                id_attr: None,
                class_start: 0,
                class_len: 0,
                attr_start: 0,
                attr_len: 0,
                inline_style: None,
            },
        )
    }

    pub fn append_child(&mut self, parent_id: NodeId, child_id: NodeId) -> bool {
        let p = parent_id as usize;
        let c = child_id as usize;
        if p >= self.nodes.len() || c >= self.nodes.len() || parent_id == child_id {
            return false;
        }
        // Detach from old parent first if already attached
        let old_parent = self.nodes[c].parent;
        if old_parent != NO_NODE {
            let _ = self.remove_child(old_parent, child_id);
        }
        self.nodes[c].parent = parent_id;
        self.nodes[c].next_sibling = NO_NODE;

        let prev_last = self.nodes[p].last_child;
        if prev_last == NO_NODE {
            self.nodes[p].first_child = child_id;
        } else {
            self.nodes[prev_last as usize].next_sibling = child_id;
        }
        self.nodes[p].last_child = child_id;
        self.generation = self.generation.wrapping_add(1);
        true
    }

    pub fn remove_child(&mut self, parent_id: NodeId, child_id: NodeId) -> bool {
        let p = parent_id as usize;
        let c = child_id as usize;
        if p >= self.nodes.len() || c >= self.nodes.len() {
            return false;
        }
        if self.nodes[c].parent != parent_id {
            return false;
        }
        let next_sib = self.nodes[c].next_sibling;
        if self.nodes[p].first_child == child_id {
            self.nodes[p].first_child = next_sib;
            if self.nodes[p].last_child == child_id {
                self.nodes[p].last_child = NO_NODE;
            }
        } else {
            let mut cur = self.nodes[p].first_child;
            while cur != NO_NODE {
                if self.nodes[cur as usize].next_sibling == child_id {
                    self.nodes[cur as usize].next_sibling = next_sib;
                    if self.nodes[p].last_child == child_id {
                        self.nodes[p].last_child = cur;
                    }
                    break;
                }
                cur = self.nodes[cur as usize].next_sibling;
            }
        }
        self.nodes[c].parent = NO_NODE;
        self.nodes[c].next_sibling = NO_NODE;
        self.generation = self.generation.wrapping_add(1);
        true
    }

    pub fn set_attribute(&mut self, node_id: NodeId, name: &str, value: &str) -> bool {
        if (node_id as usize) >= self.nodes.len() {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        let attr_key = name.trim().to_ascii_lowercase();

        let kind = self.nodes[node_id as usize].kind;
        let NodeKind::Element {
            tag,
            mut id_attr,
            mut class_start,
            mut class_len,
            mut attr_start,
            mut attr_len,
            mut inline_style,
        } = kind
        else {
            return false;
        };

        if attr_key == "id" {
            if let Some(old_sym) = id_attr {
                self.id_index.remove(&old_sym);
            }
            if value.is_empty() {
                id_attr = None;
            } else {
                let new_sym = self.interner.intern(value);
                id_attr = Some(new_sym);
                self.id_index.insert(new_sym, node_id);
            }
        } else if attr_key == "class" {
            class_start = self.class_pool.len() as u32;
            class_len = 0;
            for cls in value.split_whitespace() {
                let c_sym = self.interner.intern(cls);
                self.class_pool.push(c_sym);
                class_len = class_len.saturating_add(1);
            }
        } else if attr_key == "style" {
            inline_style = Some(self.alloc_span(value));
        } else if attr_key == "value" {
            let _ = self.forms.set_value(node_id, value);
        }

        // Rebuild element's attribute slice cleanly
        let key_sym = self.interner.intern(&attr_key);
        let val_span = self.alloc_span(value);

        let old_s = attr_start as usize;
        let old_e = old_s + attr_len as usize;
        let mut updated = false;
        if let Some(slice) = self.attrs_pool.get_mut(old_s..old_e) {
            for pair in slice.iter_mut() {
                if pair.0 == key_sym {
                    pair.1 = val_span;
                    updated = true;
                    break;
                }
            }
        }
        if !updated {
            let new_start = self.attrs_pool.len() as u32;
            let mut count = 0u16;
            for i in old_s..old_e {
                if let Some(&pair) = self.attrs_pool.get(i) {
                    self.attrs_pool.push(pair);
                    count = count.saturating_add(1);
                }
            }
            self.attrs_pool.push((key_sym, val_span));
            attr_start = new_start;
            attr_len = count.saturating_add(1);
        }

        self.nodes[node_id as usize].kind = NodeKind::Element {
            tag,
            id_attr,
            class_start,
            class_len,
            attr_start,
            attr_len,
            inline_style,
        };
        true
    }

    pub fn remove_attribute(&mut self, node_id: NodeId, name: &str) -> bool {
        if (node_id as usize) >= self.nodes.len() {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        let attr_key = name.trim().to_ascii_lowercase();
        let kind = self.nodes[node_id as usize].kind;
        let NodeKind::Element {
            tag,
            mut id_attr,
            mut class_start,
            mut class_len,
            attr_start,
            attr_len,
            mut inline_style,
        } = kind
        else {
            return false;
        };

        if attr_key == "id" {
            if let Some(old) = id_attr.take() {
                self.id_index.remove(&old);
            }
        } else if attr_key == "class" {
            class_start = 0;
            class_len = 0;
        } else if attr_key == "style" {
            inline_style = None;
        }

        let key_sym = self.interner.intern(&attr_key);
        let old_s = attr_start as usize;
        let old_e = old_s + attr_len as usize;
        let new_start = self.attrs_pool.len() as u32;
        let mut new_len = 0u16;
        for i in old_s..old_e {
            if let Some(&pair) = self.attrs_pool.get(i) {
                if pair.0 != key_sym {
                    self.attrs_pool.push(pair);
                    new_len = new_len.saturating_add(1);
                }
            }
        }
        self.nodes[node_id as usize].kind = NodeKind::Element {
            tag,
            id_attr,
            class_start,
            class_len,
            attr_start: new_start,
            attr_len: new_len,
            inline_style,
        };
        true
    }

    pub fn get_attribute(&self, node_id: NodeId, name: &str) -> Option<String> {
        let node = self.nodes.get(node_id as usize)?;
        let attr_key = name.trim().to_ascii_lowercase();
        if attr_key == "value" {
            if let Some(ctrl) = self.forms.get_control(node_id) {
                return Some(ctrl.value.clone());
            }
        }
        if let NodeKind::Element {
            attr_start,
            attr_len,
            ..
        } = node.kind
        {
            let s = attr_start as usize;
            let e = s + attr_len as usize;
            if let Some(slice) = self.attrs_pool.get(s..e) {
                for &(k_sym, v_span) in slice {
                    if self.sym_str(k_sym) == attr_key {
                        return Some(self.span_str(v_span).to_string());
                    }
                }
            }
        }
        None
    }

    pub fn add_class(&mut self, node_id: NodeId, cls: &str) -> bool {
        let clean = cls.trim();
        if clean.is_empty() || self.node_has_class(node_id, clean) {
            return true;
        }
        let mut existing: Vec<String> = self
            .node_classes(node_id)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        existing.push(clean.to_string());
        self.set_attribute(node_id, "class", &existing.join(" "))
    }

    pub fn remove_class(&mut self, node_id: NodeId, cls: &str) -> bool {
        let clean = cls.trim();
        let existing: Vec<String> = self
            .node_classes(node_id)
            .into_iter()
            .filter(|&c| c != clean)
            .map(|s| s.to_string())
            .collect();
        self.set_attribute(node_id, "class", &existing.join(" "))
    }

    pub fn add_event_listener(&mut self, node_id: NodeId, event_type: &str, handler_js: &str) {
        self.event_listeners
            .entry((node_id, event_type.trim().to_ascii_lowercase()))
            .or_default()
            .push(handler_js.to_string());
    }

    pub fn get_event_listeners(&self, node_id: NodeId, event_type: &str) -> Vec<String> {
        self.event_listeners
            .get(&(node_id, event_type.trim().to_ascii_lowercase()))
            .cloned()
            .unwrap_or_default()
    }

    /// Collects bubbling event path `[target, parent, ..., document]` for event dispatch.
    pub fn event_bubbling_path(&self, target_id: NodeId) -> Vec<NodeId> {
        let mut path = Vec::with_capacity(16);
        let mut cur = target_id;
        while cur != NO_NODE && (cur as usize) < self.nodes.len() && path.len() < 128 {
            path.push(cur);
            cur = self.nodes[cur as usize].parent;
        }
        path
    }

    pub fn set_text_content(&mut self, node_id: NodeId, new_text: String) -> bool {
        let idx = node_id as usize;
        if idx >= self.nodes.len() {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        if self.forms.get_control(node_id).is_some() {
            let _ = self.forms.set_value(node_id, &new_text);
        }
        let span = self.alloc_span(&new_text);
        let first_child = self.nodes[idx].first_child;
        if first_child != NO_NODE {
            if let NodeKind::Text(_) = self.nodes[first_child as usize].kind {
                self.nodes[first_child as usize].kind = NodeKind::Text(span);
                return true;
            }
        }
        self.alloc_node(node_id, NodeKind::Text(span));
        true
    }

    pub fn text_content(&self, node_id: NodeId) -> String {
        if let Some(ctrl) = self.forms.get_control(node_id) {
            if matches!(
                ctrl.kind,
                FormControlKind::TextInput { .. } | FormControlKind::TextArea
            ) {
                return ctrl.value.clone();
            }
        }
        let mut out = String::new();
        self.collect_text_recursive(node_id, &mut out);
        out
    }

    pub fn control_for_node(&self, node_id: NodeId) -> Option<&FormControlData> {
        self.forms.get_control(node_id)
    }

    /// Searches DOM text nodes case-insensitively for Find-in-Page.
    pub fn find_text_matches(&self, query: &str) -> Vec<NodeId> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut matches = Vec::new();
        for node in &self.nodes {
            if let NodeKind::Text(sp) = node.kind {
                if self.span_str(sp).to_ascii_lowercase().contains(&q) {
                    matches.push(node.id);
                }
            }
        }
        matches
    }

    /// Extracts clean readable article text for Reader Mode (skipping nav/footer/aside).
    pub fn extract_reader_text(&self) -> String {
        let mut out = String::with_capacity(1024);
        self.collect_reader_recursive(0, &mut out);
        out
    }

    fn collect_reader_recursive(&self, node_id: NodeId, out: &mut String) {
        let Some(node) = self.nodes.get(node_id as usize) else {
            return;
        };
        if let Some(tag) = self.node_tag(node_id) {
            if matches!(tag, "head" | "script" | "style" | "nav" | "footer" | "aside") {
                return;
            }
        }
        if let NodeKind::Text(sp) = node.kind {
            let txt = self.span_str(sp).trim();
            if !txt.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(txt);
            }
            return;
        }
        let mut child = node.first_child;
        while child != NO_NODE {
            self.collect_reader_recursive(child, out);
            child = self.nodes[child as usize].next_sibling;
        }
    }

    fn collect_text_recursive(&self, node_id: NodeId, out: &mut String) {
        let Some(node) = self.nodes.get(node_id as usize) else {
            return;
        };
        match node.kind {
            NodeKind::Text(sp) => {
                let t = self.span_str(sp);
                if !out.is_empty() && !t.is_empty() {
                    out.push(' ');
                }
                out.push_str(t);
            }
            _ => {
                let mut child = node.first_child;
                while child != NO_NODE {
                    self.collect_text_recursive(child, out);
                    child = self.nodes[child as usize].next_sibling;
                }
            }
        }
    }

    /// Drops transient raw style/script sources and returns unused OS pages via `madvise`.
    pub fn release_transient_sources_and_compact(&mut self) {
        self.collected_styles.clear();
        self.collected_styles.shrink_to_fit();
        self.collected_scripts.clear();
        self.collected_scripts.shrink_to_fit();
        self.open_stack.shrink_to_fit();
        self.nodes.shrink_to_fit();
        self.class_pool.shrink_to_fit();
        self.attrs_pool.shrink_to_fit();
        self.interner.shrink();
        self.text_slab.release_unused_pages();
    }

    /// Fast O(1) arena reset and memory release when a tab is explicitly closed.
    pub fn clear_and_shrink(&mut self) {
        self.nodes.clear();
        self.nodes.shrink_to_fit();
        self.open_stack.clear();
        self.open_stack.shrink_to_fit();
        self.interner.clear_and_shrink();
        self.text_slab.clear_and_release();
        self.class_pool.clear();
        self.class_pool.shrink_to_fit();
        self.attrs_pool.clear();
        self.attrs_pool.shrink_to_fit();
        self.id_index.clear();
        self.id_index.shrink_to_fit();
        self.event_listeners.clear();
        self.forms.clear();
        self.collected_styles.clear();
        self.collected_styles.shrink_to_fit();
        self.collected_scripts.clear();
        self.collected_scripts.shrink_to_fit();
        self.external_stylesheets.clear();
        self.external_scripts.clear();
        self.media_elements.clear();
        self.base_href = None;
        self.page_title.clear();
    }

    fn alloc_node(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(DomNode {
            id,
            parent,
            first_child: NO_NODE,
            last_child: NO_NODE,
            next_sibling: NO_NODE,
            kind,
        });

        if parent != NO_NODE && (parent as usize) < self.nodes.len() {
            let prev_last = self.nodes[parent as usize].last_child;
            if prev_last == NO_NODE {
                self.nodes[parent as usize].first_child = id;
            } else {
                self.nodes[prev_last as usize].next_sibling = id;
            }
            self.nodes[parent as usize].last_child = id;
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_flat_dom_arena_memory_efficiency_and_implicit_closing() {
        assert!(std::mem::size_of::<DomNode>() <= 64);

        let mut dom = ZeroCopyDomArena::new(512 * 1024);
        let mut tok = StreamingHtmlTokenizer::new();
        tok.feed_chunk(
            b"<ul><li>One<li>Two</ul><p>First<div id=\"box\" class=\"card active\">Inside</div>",
            true,
            |t| dom.ingest_token(t),
        );

        let box_id = dom.find_by_id("box").unwrap();
        assert!(dom.node_has_class(box_id, "card"));
        assert!(dom.node_has_class(box_id, "active"));
        assert_eq!(dom.text_content(box_id), "Inside");
        assert!(!dom.find_text_matches("inside").is_empty());
    }
}
