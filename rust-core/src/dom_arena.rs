use std::collections::HashMap;

use crate::html_tokenizer::HtmlToken;
use crate::input_forms::{FormControlData, FormControlKind, MantaFormEngine};

/// Compact 32-bit node index into the flat `ZeroCopyDomArena`.
/// Eliminates 64-bit pointer overhead and `Rc<RefCell<...>>` cycles completely.
pub type NodeId = u32;

pub const NO_NODE: NodeId = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Element {
        tag: String,
        id_attr: Option<String>,
        classes: Vec<String>,
        attributes: Vec<(String, String)>,
        inline_style: Option<String>,
    },
    Text(String),
}

#[derive(Debug, Clone)]
pub struct DomNode {
    pub id: NodeId,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub last_child: NodeId,
    pub next_sibling: NodeId,
    pub kind: NodeKind,
}

/// Contiguous Data-Oriented DOM Arena with **$O(1)$ ID Hash Index**, **Interactive Form Engine**,
/// and **DOM Event Listener Registry**.
pub struct ZeroCopyDomArena {
    nodes: Vec<DomNode>,
    open_stack: Vec<NodeId>,
    id_index: HashMap<String, NodeId>,
    event_listeners: HashMap<(NodeId, String), Vec<String>>, // (node_id, event_type) -> Vec<js_handler_code>
    pub forms: MantaFormEngine,
    pub collected_styles: Vec<String>,
    pub collected_scripts: Vec<String>,
    pub page_title: String,
    approx_bytes: usize,
    max_bytes: usize,
}

impl ZeroCopyDomArena {
    pub fn new(max_bytes: usize) -> Self {
        let mut arena = Self {
            nodes: Vec::with_capacity(128),
            open_stack: Vec::with_capacity(16),
            id_index: HashMap::with_capacity(32),
            event_listeners: HashMap::new(),
            forms: MantaFormEngine::new(),
            collected_styles: Vec::new(),
            collected_scripts: Vec::new(),
            page_title: String::from("Manta"),
            approx_bytes: 0,
            max_bytes,
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
        self.approx_bytes
    }

    pub fn ingest_token(&mut self, token: HtmlToken) {
        if self.approx_bytes >= self.max_bytes {
            return;
        }
        let current_parent = *self.open_stack.last().unwrap_or(&0);

        match token {
            HtmlToken::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                let mut id_attr = None;
                let mut classes = Vec::new();
                let mut inline_style = None;
                let mut onclick_handler = None;
                let mut oninput_handler = None;
                let mut onsubmit_handler = None;

                for (k, v) in &attributes {
                    match k.as_str() {
                        "id" => id_attr = Some(v.clone()),
                        "class" => {
                            classes.extend(v.split_whitespace().map(|s| s.to_string()));
                        }
                        "style" => inline_style = Some(v.clone()),
                        "onclick" => onclick_handler = Some(v.clone()),
                        "oninput" | "onchange" => oninput_handler = Some(v.clone()),
                        "onsubmit" => onsubmit_handler = Some(v.clone()),
                        _ => {}
                    }
                }

                let is_form_control =
                    matches!(name.as_str(), "input" | "textarea" | "select" | "button" | "form");
                let enclosing_form = self.find_enclosing_form_in_stack();

                let node_id = self.alloc_node(
                    current_parent,
                    NodeKind::Element {
                        tag: name.clone(),
                        id_attr: id_attr.clone(),
                        classes,
                        attributes: attributes.clone(),
                        inline_style,
                    },
                );

                if let Some(id_str) = id_attr {
                    if !id_str.is_empty() {
                        self.id_index.insert(id_str, node_id);
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
                    self.forms.register_element_from_dom(
                        node_id,
                        enclosing_form,
                        &name,
                        &attributes,
                    );
                }

                if !self_closing {
                    self.open_stack.push(node_id);
                }
            }
            HtmlToken::EndTag { name } => {
                if let Some(pos) = self.open_stack.iter().rposition(|&nid| {
                    matches!(
                        &self.nodes[nid as usize].kind,
                        NodeKind::Element { tag, .. } if tag == &name
                    )
                }) {
                    let closed_node_id = self.open_stack[pos];
                    self.open_stack.truncate(pos);
                    if name == "textarea" || name == "option" {
                        let initial_txt = self.text_content(closed_node_id);
                        self.forms.finalize_text_control(closed_node_id, &initial_txt);
                    }
                }
            }
            HtmlToken::Text(txt) => {
                if let Some(parent_node) = self.nodes.get(current_parent as usize) {
                    if let NodeKind::Element { tag, .. } = &parent_node.kind {
                        match tag.as_str() {
                            "style" => {
                                self.approx_bytes += txt.len();
                                self.collected_styles.push(txt);
                                return;
                            }
                            "script" => {
                                self.approx_bytes += txt.len();
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
                }
                self.alloc_node(current_parent, NodeKind::Text(txt));
            }
        }
    }

    fn find_enclosing_form_in_stack(&self) -> Option<NodeId> {
        for &nid in self.open_stack.iter().rev() {
            if let Some(node) = self.nodes.get(nid as usize) {
                if let NodeKind::Element { ref tag, .. } = node.kind {
                    if tag == "form" {
                        return Some(nid);
                    }
                }
            }
        }
        None
    }

    /// $O(1)$ hash-indexed lookup by element `id`.
    #[inline]
    pub fn find_by_id(&self, target_id: &str) -> Option<NodeId> {
        self.id_index.get(target_id).copied()
    }

    pub fn query_selector(&self, selector: &str) -> Option<NodeId> {
        let sel = selector.trim();
        if let Some(id) = sel.strip_prefix('#') {
            return self.find_by_id(id);
        }
        if let Some(cls) = sel.strip_prefix('.') {
            for node in &self.nodes {
                if let NodeKind::Element { ref classes, .. } = node.kind {
                    if classes.iter().any(|c| c == cls) {
                        return Some(node.id);
                    }
                }
            }
            return None;
        }
        let tag_target = sel.to_ascii_lowercase();
        for node in &self.nodes {
            if let NodeKind::Element { ref tag, .. } = node.kind {
                if tag == &tag_target {
                    return Some(node.id);
                }
            }
        }
        None
    }

    /// Dynamically creates an element node (for `document.createElement`).
    pub fn create_element(&mut self, tag_name: &str) -> NodeId {
        let clean_tag = tag_name.trim().to_ascii_lowercase();
        self.alloc_node(
            0,
            NodeKind::Element {
                tag: clean_tag,
                id_attr: None,
                classes: Vec::new(),
                attributes: Vec::new(),
                inline_style: None,
            },
        )
    }

    /// Sets an attribute on an element and keeps the $O(1)$ `id_index` and `MantaFormEngine` in sync.
    pub fn set_attribute(&mut self, node_id: NodeId, name: &str, value: &str) -> bool {
        let Some(node) = self.nodes.get_mut(node_id as usize) else {
            return false;
        };
        let attr_key = name.trim().to_ascii_lowercase();
        if let NodeKind::Element {
            ref mut id_attr,
            ref mut classes,
            ref mut attributes,
            ref mut inline_style,
            ..
        } = node.kind
        {
            if attr_key == "id" {
                if let Some(old_id) = id_attr.take() {
                    self.id_index.remove(&old_id);
                }
                if !value.is_empty() {
                    *id_attr = Some(value.to_string());
                    self.id_index.insert(value.to_string(), node_id);
                }
            } else if attr_key == "class" {
                *classes = value.split_whitespace().map(|s| s.to_string()).collect();
            } else if attr_key == "style" {
                *inline_style = Some(value.to_string());
            } else if attr_key == "value" {
                let _ = self.forms.set_value(node_id, value);
            }

            if let Some(existing) = attributes.iter_mut().find(|(k, _)| k == &attr_key) {
                existing.1 = value.to_string();
            } else {
                attributes.push((attr_key, value.to_string()));
            }
            return true;
        }
        false
    }

    pub fn get_attribute(&self, node_id: NodeId, name: &str) -> Option<String> {
        let node = self.nodes.get(node_id as usize)?;
        let attr_key = name.trim().to_ascii_lowercase();
        if attr_key == "value" {
            if let Some(ctrl) = self.forms.get_control(node_id) {
                return Some(ctrl.value.clone());
            }
        }
        if let NodeKind::Element { ref attributes, .. } = node.kind {
            return attributes
                .iter()
                .find(|(k, _)| k == &attr_key)
                .map(|(_, v)| v.clone());
        }
        None
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

    pub fn set_text_content(&mut self, node_id: NodeId, new_text: String) -> bool {
        let idx = node_id as usize;
        if idx >= self.nodes.len() {
            return false;
        }
        if self.forms.get_control(node_id).is_some() {
            let _ = self.forms.set_value(node_id, &new_text);
        }
        let first_child = self.nodes[idx].first_child;
        if first_child != NO_NODE {
            if let NodeKind::Text(ref mut existing) = self.nodes[first_child as usize].kind {
                *existing = new_text;
                return true;
            }
        }
        self.alloc_node(node_id, NodeKind::Text(new_text));
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

    fn collect_text_recursive(&self, node_id: NodeId, out: &mut String) {
        let Some(node) = self.nodes.get(node_id as usize) else {
            return;
        };
        match &node.kind {
            NodeKind::Text(t) => {
                if !out.is_empty() {
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

    /// Fast O(1) arena reset and memory release when a tab is explicitly closed.
    pub fn clear_and_shrink(&mut self) {
        self.nodes.clear();
        self.nodes.shrink_to_fit();
        self.open_stack.clear();
        self.id_index.clear();
        self.id_index.shrink_to_fit();
        self.event_listeners.clear();
        self.forms.clear();
        self.collected_styles.clear();
        self.collected_styles.shrink_to_fit();
        self.collected_scripts.clear();
        self.collected_scripts.shrink_to_fit();
        self.page_title.clear();
        self.approx_bytes = 0;
    }

    fn alloc_node(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = self.nodes.len() as NodeId;
        let node_cost = std::mem::size_of::<DomNode>()
            + match &kind {
                NodeKind::Document => 0,
                NodeKind::Text(t) => t.len(),
                NodeKind::Element {
                    tag, attributes, ..
                } => tag.len() + attributes.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>(),
            };
        self.approx_bytes += node_cost;

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
