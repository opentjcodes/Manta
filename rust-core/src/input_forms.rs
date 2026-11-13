use std::collections::HashMap;

use crate::dom_arena::NodeId;
use crate::media_stream::resolve_relative_url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedFilePayload {
    pub filename: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormControlKind {
    TextInput { input_type: String },
    TextArea,
    Checkbox,
    Radio,
    Select { options: Vec<(String, String)> },
    FileInput { multiple: bool },
    Hidden,
    SubmitButton,
    ResetButton,
}

#[derive(Debug, Clone)]
pub struct FormControlData {
    pub node_id: NodeId,
    pub form_node_id: Option<NodeId>,
    pub name: String,
    pub value: String,
    pub default_value: String,
    pub placeholder: String,
    pub checked: bool,
    pub default_checked: bool,
    pub disabled: bool,
    pub required: bool,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub selection_start: usize,
    pub selection_end: usize,
    pub attached_files: Vec<AttachedFilePayload>,
    pub kind: FormControlKind,
}

#[derive(Debug, Clone)]
pub struct FormDefinition {
    pub form_node_id: NodeId,
    pub action: String,
    pub method: String,
    pub enctype: String,
    pub control_nodes: Vec<NodeId>,
}

#[derive(Debug, Clone)]
pub struct SerializedFormSubmission {
    pub action_url: String,
    pub method: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct MantaFormEngine {
    controls: HashMap<NodeId, FormControlData>,
    forms: HashMap<NodeId, FormDefinition>,
    focused_node: Option<NodeId>,
}

impl MantaFormEngine {
    pub fn new() -> Self {
        Self {
            controls: HashMap::new(),
            forms: HashMap::new(),
            focused_node: None,
        }
    }

    #[inline]
    pub fn focused_node(&self) -> Option<NodeId> {
        self.focused_node
    }

    pub fn focus_control(&mut self, node_id: NodeId) -> bool {
        if let Some(ctrl) = self.controls.get(&node_id) {
            if !ctrl.disabled {
                self.focused_node = Some(node_id);
                return true;
            }
        }
        false
    }

    pub fn blur(&mut self) {
        self.focused_node = None;
    }

    pub fn register_element_from_dom(
        &mut self,
        node_id: NodeId,
        enclosing_form_id: Option<NodeId>,
        tag: &str,
        attributes: &[(String, String)],
    ) {
        if tag == "form" {
            let mut action = String::new();
            let mut method = String::from("GET");
            let mut enctype = String::from("application/x-www-form-urlencoded");
            for (k, v) in attributes {
                match k.as_str() {
                    "action" => action = v.clone(),
                    "method" => method = v.trim().to_ascii_uppercase(),
                    "enctype" => enctype = v.trim().to_ascii_lowercase(),
                    _ => {}
                }
            }
            self.forms.insert(
                node_id,
                FormDefinition {
                    form_node_id: node_id,
                    action,
                    method,
                    enctype,
                    control_nodes: Vec::new(),
                },
            );
            return;
        }

        let mut name = String::new();
        let mut value = String::new();
        let mut placeholder = String::new();
        let mut input_type = String::from("text");
        let mut checked = false;
        let mut disabled = false;
        let mut required = false;
        let mut multiple = false;
        let mut min_length = None;
        let mut max_length = None;

        for (k, v) in attributes {
            match k.as_str() {
                "name" => name = v.clone(),
                "value" => value = v.clone(),
                "placeholder" => placeholder = v.clone(),
                "type" => input_type = v.trim().to_ascii_lowercase(),
                "checked" => checked = true,
                "disabled" => disabled = true,
                "required" => required = true,
                "multiple" => multiple = true,
                "minlength" => min_length = v.parse::<usize>().ok(),
                "maxlength" => max_length = v.parse::<usize>().ok(),
                _ => {}
            }
        }

        let kind = match tag {
            "textarea" => FormControlKind::TextArea,
            "select" => FormControlKind::Select {
                options: Vec::new(),
            },
            "button" => {
                if input_type == "reset" {
                    FormControlKind::ResetButton
                } else {
                    FormControlKind::SubmitButton
                }
            }
            "input" => match input_type.as_str() {
                "checkbox" => FormControlKind::Checkbox,
                "radio" => FormControlKind::Radio,
                "file" => FormControlKind::FileInput { multiple },
                "hidden" => FormControlKind::Hidden,
                "submit" => FormControlKind::SubmitButton,
                "reset" => FormControlKind::ResetButton,
                _ => FormControlKind::TextInput { input_type },
            },
            _ => return,
        };

        let len = value.len();
        self.controls.insert(
            node_id,
            FormControlData {
                node_id,
                form_node_id: enclosing_form_id,
                name,
                default_value: value.clone(),
                value,
                placeholder,
                checked,
                default_checked: checked,
                disabled,
                required,
                min_length,
                max_length,
                selection_start: len,
                selection_end: len,
                attached_files: Vec::new(),
                kind,
            },
        );

        if let Some(fid) = enclosing_form_id {
            if let Some(form_def) = self.forms.get_mut(&fid) {
                form_def.control_nodes.push(node_id);
            }
        }
    }

    pub fn finalize_text_control(&mut self, node_id: NodeId, inner_text: &str) {
        if let Some(ctrl) = self.controls.get_mut(&node_id) {
            if ctrl.kind == FormControlKind::TextArea && ctrl.value.is_empty() {
                ctrl.value = inner_text.to_string();
                ctrl.default_value = inner_text.to_string();
                ctrl.selection_start = ctrl.value.len();
                ctrl.selection_end = ctrl.value.len();
            }
        }
    }

    pub fn get_control(&self, node_id: NodeId) -> Option<&FormControlData> {
        self.controls.get(&node_id)
    }

    pub fn set_value(&mut self, node_id: NodeId, new_value: &str) -> Result<(), &'static str> {
        let ctrl = self
            .controls
            .get_mut(&node_id)
            .ok_or("Node is not an interactive form control")?;
        if ctrl.disabled {
            return Err("Control is disabled");
        }
        let clipped = if let Some(max_len) = ctrl.max_length {
            new_value.chars().take(max_len).collect::<String>()
        } else {
            new_value.to_string()
        };
        ctrl.selection_start = clipped.len();
        ctrl.selection_end = clipped.len();
        ctrl.value = clipped;
        Ok(())
    }

    pub fn toggle_checkable(&mut self, node_id: NodeId) -> Result<bool, &'static str> {
        let (kind, form_id, group_name, disabled) = {
            let ctrl = self
                .controls
                .get(&node_id)
                .ok_or("Node is not a form control")?;
            (
                ctrl.kind.clone(),
                ctrl.form_node_id,
                ctrl.name.clone(),
                ctrl.disabled,
            )
        };
        if disabled {
            return Err("Control is disabled");
        }
        match kind {
            FormControlKind::Checkbox => {
                let ctrl = self.controls.get_mut(&node_id).unwrap();
                ctrl.checked = !ctrl.checked;
                Ok(ctrl.checked)
            }
            FormControlKind::Radio => {
                for other in self.controls.values_mut() {
                    if other.kind == FormControlKind::Radio
                        && other.form_node_id == form_id
                        && other.name == group_name
                    {
                        other.checked = other.node_id == node_id;
                    }
                }
                Ok(true)
            }
            _ => Err("Control is not a checkbox or radio button"),
        }
    }

    pub fn attach_file(
        &mut self,
        node_id: NodeId,
        filename: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<(), &'static str> {
        let ctrl = self
            .controls
            .get_mut(&node_id)
            .ok_or("Node is not a file input")?;
        if ctrl.disabled {
            return Err("File input is disabled");
        }
        let clean_name = sanitize_multipart_header_field(filename);
        let clean_mime = sanitize_multipart_header_field(mime_type);
        match ctrl.kind {
            FormControlKind::FileInput { multiple } => {
                if !multiple {
                    ctrl.attached_files.clear();
                }
                ctrl.value = format!("C:\\fakepath\\{}", clean_name);
                ctrl.attached_files.push(AttachedFilePayload {
                    filename: clean_name,
                    mime_type: if clean_mime.is_empty() {
                        "application/octet-stream".to_string()
                    } else {
                        clean_mime
                    },
                    bytes,
                });
                Ok(())
            }
            _ => Err("Target control is not an <input type=\"file\">"),
        }
    }

    pub fn reset_form(&mut self, form_node_id: NodeId) -> bool {
        let Some(form) = self.forms.get(&form_node_id) else {
            return false;
        };
        let nodes = form.control_nodes.clone();
        for cid in nodes {
            if let Some(ctrl) = self.controls.get_mut(&cid) {
                ctrl.value = ctrl.default_value.clone();
                ctrl.checked = ctrl.default_checked;
                ctrl.attached_files.clear();
            }
        }
        true
    }

    pub fn serialize_submission(
        &self,
        form_or_control_id: NodeId,
        current_page_url: &str,
    ) -> Result<SerializedFormSubmission, &'static str> {
        let form_id = if self.forms.contains_key(&form_or_control_id) {
            form_or_control_id
        } else if let Some(ctrl) = self.controls.get(&form_or_control_id) {
            ctrl.form_node_id.ok_or("Control is not inside a <form>")?
        } else {
            return Err("Unknown form or control NodeId");
        };

        let form = self.forms.get(&form_id).ok_or("Form definition not found")?;
        let action_url = if form.action.is_empty() {
            current_page_url.to_string()
        } else {
            resolve_relative_url(current_page_url, &form.action)
        };

        let has_files = form.control_nodes.iter().any(|cid| {
            self.controls
                .get(cid)
                .map(|c| !c.attached_files.is_empty())
                .unwrap_or(false)
        });

        for &cid in &form.control_nodes {
            let Some(ctrl) = self.controls.get(&cid) else {
                continue;
            };
            if ctrl.disabled {
                continue;
            }
            if ctrl.required {
                match ctrl.kind {
                    FormControlKind::Checkbox | FormControlKind::Radio => {
                        if !ctrl.checked {
                            return Err("Required checkbox/radio control is not checked");
                        }
                    }
                    FormControlKind::FileInput { .. } => {
                        if ctrl.attached_files.is_empty() {
                            return Err("Required file input has no attached file");
                        }
                    }
                    _ => {
                        if ctrl.value.trim().is_empty() {
                            return Err("Required input field is empty");
                        }
                    }
                }
            }
            if let Some(min_len) = ctrl.min_length {
                if ctrl.value.chars().count() < min_len {
                    return Err("Input value is shorter than minlength constraint");
                }
            }
        }

        if form.method == "POST" && (form.enctype == "multipart/form-data" || has_files) {
            let boundary = "----MantaFormBoundary7MA4YWxkTrZu0gW";
            let mut body = Vec::with_capacity(512);

            for &cid in &form.control_nodes {
                let Some(ctrl) = self.controls.get(&cid) else {
                    continue;
                };
                if ctrl.disabled || ctrl.name.is_empty() {
                    continue;
                }
                let safe_name = sanitize_multipart_header_field(&ctrl.name);
                match ctrl.kind {
                    FormControlKind::FileInput { .. } => {
                        for file in &ctrl.attached_files {
                            let safe_fname = sanitize_multipart_header_field(&file.filename);
                            let safe_mime = sanitize_multipart_header_field(&file.mime_type);
                            body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
                            body.extend_from_slice(
                                format!(
                                    "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                                    safe_name, safe_fname
                                )
                                .as_bytes(),
                            );
                            body.extend_from_slice(
                                format!("Content-Type: {}\r\n\r\n", safe_mime).as_bytes(),
                            );
                            body.extend_from_slice(&file.bytes);
                            body.extend_from_slice(b"\r\n");
                        }
                    }
                    FormControlKind::Checkbox | FormControlKind::Radio => {
                        if ctrl.checked {
                            let val = if ctrl.value.is_empty() {
                                "on"
                            } else {
                                &ctrl.value
                            };
                            append_multipart_text(&mut body, boundary, &safe_name, val);
                        }
                    }
                    FormControlKind::SubmitButton | FormControlKind::ResetButton => {}
                    _ => {
                        append_multipart_text(&mut body, boundary, &safe_name, &ctrl.value);
                    }
                }
            }
            body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());
            return Ok(SerializedFormSubmission {
                action_url,
                method: "POST".to_string(),
                content_type: format!("multipart/form-data; boundary={}", boundary),
                body,
            });
        }

        let mut pairs = Vec::new();
        for &cid in &form.control_nodes {
            let Some(ctrl) = self.controls.get(&cid) else {
                continue;
            };
            if ctrl.disabled || ctrl.name.is_empty() {
                continue;
            }
            match ctrl.kind {
                FormControlKind::Checkbox | FormControlKind::Radio => {
                    if ctrl.checked {
                        let val = if ctrl.value.is_empty() {
                            "on"
                        } else {
                            &ctrl.value
                        };
                        pairs.push(format!(
                            "{}={}",
                            url_encode(&ctrl.name),
                            url_encode(val)
                        ));
                    }
                }
                FormControlKind::SubmitButton | FormControlKind::ResetButton => {}
                _ => {
                    pairs.push(format!(
                        "{}={}",
                        url_encode(&ctrl.name),
                        url_encode(&ctrl.value)
                    ));
                }
            }
        }
        let encoded_query = pairs.join("&");

        if form.method == "GET" {
            let sep = if action_url.contains('?') { '&' } else { '?' };
            let final_url = if encoded_query.is_empty() {
                action_url
            } else {
                format!("{}{}{}", action_url, sep, encoded_query)
            };
            Ok(SerializedFormSubmission {
                action_url: final_url,
                method: "GET".to_string(),
                content_type: String::new(),
                body: Vec::new(),
            })
        } else {
            Ok(SerializedFormSubmission {
                action_url,
                method: "POST".to_string(),
                content_type: "application/x-www-form-urlencoded".to_string(),
                body: encoded_query.into_bytes(),
            })
        }
    }

    pub fn clear(&mut self) {
        self.controls.clear();
        self.forms.clear();
        self.focused_node = None;
    }
}

fn sanitize_multipart_header_field(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '"' && c != '\r' && c != '\n' && c != '\0')
        .collect()
}

fn append_multipart_text(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes(),
    );
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::dom_arena::ZeroCopyDomArena;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_form_inputs_file_upload_and_relative_action() {
        let mut dom = ZeroCopyDomArena::new(64 * 1024);
        let mut tok = StreamingHtmlTokenizer::new();
        let html = br#"
            <form id="upload-form" action="/api/upload" method="POST" enctype="multipart/form-data">
                <input id="title" name="title" type="text" required />
                <input id="doc" name="document" type="file" required />
            </form>
        "#;
        tok.feed_chunk(html, true, |t| dom.ingest_token(t));

        let title_id = dom.find_by_id("title").unwrap();
        let doc_id = dom.find_by_id("doc").unwrap();

        dom.forms.set_value(title_id, "Q4 Report").unwrap();
        dom.forms
            .attach_file(doc_id, "evil\"\r\nreport.txt", "text/plain", b"Hello Manta".to_vec())
            .unwrap();

        let sub = dom
            .forms
            .serialize_submission(title_id, "https://example.com/form")
            .unwrap();
        assert_eq!(sub.action_url, "https://example.com/api/upload");
        assert_eq!(sub.method, "POST");
        assert!(sub.content_type.starts_with("multipart/form-data"));
        let body_str = String::from_utf8_lossy(&sub.body);
        assert!(body_str.contains("Q4 Report"));
        assert!(body_str.contains("filename=\"evilreport.txt\""));
        assert!(body_str.contains("Hello Manta"));
    }
}
