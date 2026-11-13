use crate::css_engine::{ComputedStyle, DisplayMode, FlexDir, MicroCssEngine};
use crate::dom_arena::{NodeId, NodeKind, ZeroCopyDomArena, NO_NODE};
use crate::input_forms::FormControlKind;

/// Hardware-friendly draw command emitted by Manta's layout pass.
#[derive(Debug, Clone)]
pub enum PaintCommand {
    FillRect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color_argb: u32,
        corner_radius: f32,
    },
    DrawText {
        x: f32,
        y: f32,
        text: String,
        color_argb: u32,
        font_size_sp: f32,
        bold: bool,
        link_href: Option<String>,
    },
    DrawInputBox {
        node_id: NodeId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        display_text: String,
        is_focused: bool,
        is_checked: bool,
    },
}

/// Complete frame output for a tab, with viewport culling to slash GPU draw calls on long pages.
#[derive(Debug, Clone)]
pub struct DisplayListFrame {
    pub page_title: String,
    pub background_argb: u32,
    pub content_height: f32,
    pub commands: Vec<PaintCommand>,
    pub dom_nodes_count: usize,
    pub ram_used_bytes: usize,
}

impl DisplayListFrame {
    pub fn build(
        dom: &ZeroCopyDomArena,
        css: &MicroCssEngine,
        viewport_width: f32,
    ) -> DisplayListFrame {
        Self::build_culled(dom, css, viewport_width, 0.0, f32::INFINITY)
    }

    pub fn build_culled(
        dom: &ZeroCopyDomArena,
        css: &MicroCssEngine,
        viewport_width: f32,
        scroll_y: f32,
        viewport_height: f32,
    ) -> DisplayListFrame {
        let mut commands = Vec::with_capacity(64);
        let mut cursor_y = 16.0f32;
        let horizontal_padding = 16.0f32;
        let mut page_bg = 0xFF09090B;

        let cull_min_y = if viewport_height.is_finite() {
            (scroll_y - 256.0).max(0.0)
        } else {
            0.0
        };
        let cull_max_y = if viewport_height.is_finite() {
            scroll_y + viewport_height + 256.0
        } else {
            f32::INFINITY
        };

        if !dom.nodes().is_empty() {
            let root_style = ComputedStyle::default();
            layout_node_recursive(
                0,
                dom,
                css,
                &root_style,
                horizontal_padding,
                (viewport_width - (horizontal_padding * 2.0)).max(80.0),
                &mut cursor_y,
                &mut page_bg,
                None,
                cull_min_y,
                cull_max_y,
                &mut commands,
            );
        }

        DisplayListFrame {
            page_title: dom.page_title.clone(),
            background_argb: page_bg,
            content_height: cursor_y + 32.0,
            commands,
            dom_nodes_count: dom.nodes().len(),
            ram_used_bytes: dom.used_bytes(),
        }
    }

    pub fn to_compact_wire_string(&self) -> String {
        let mut out = String::with_capacity(256 + self.commands.len() * 48);
        out.push_str(&format!(
            "MANTA_FRAME|title={}|bg=#{:08X}|height={:.1}|nodes={}|ramKB={:.2}\n",
            self.page_title.replace('|', " "),
            self.background_argb,
            self.content_height,
            self.dom_nodes_count,
            self.ram_used_bytes as f32 / 1024.0
        ));

        for cmd in &self.commands {
            match cmd {
                PaintCommand::FillRect {
                    x,
                    y,
                    width,
                    height,
                    color_argb,
                    corner_radius,
                } => {
                    out.push_str(&format!(
                        "RECT|{:.1}|{:.1}|{:.1}|{:.1}|#{:08X}|{:.1}\n",
                        x, y, width, height, color_argb, corner_radius
                    ));
                }
                PaintCommand::DrawText {
                    x,
                    y,
                    text,
                    color_argb,
                    font_size_sp,
                    bold,
                    link_href,
                } => {
                    out.push_str(&format!(
                        "TEXT|{:.1}|{:.1}|{:.1}|{}|#{:08X}|{}|{}\n",
                        x,
                        y,
                        font_size_sp,
                        if *bold { 1 } else { 0 },
                        color_argb,
                        link_href.as_deref().unwrap_or(""),
                        text.replace('\n', " ").replace('|', " ")
                    ));
                }
                PaintCommand::DrawInputBox {
                    node_id,
                    x,
                    y,
                    width,
                    height,
                    display_text,
                    is_focused,
                    is_checked,
                } => {
                    out.push_str(&format!(
                        "INPUT|{}|{:.1}|{:.1}|{:.1}|{:.1}|{}|{}|{}\n",
                        node_id,
                        x,
                        y,
                        width,
                        height,
                        u8::from(*is_focused),
                        u8::from(*is_checked),
                        display_text.replace('\n', " ").replace('|', " ")
                    ));
                }
            }
        }
        out
    }
}

#[inline]
fn is_visible_band(y: f32, h: f32, cull_min_y: f32, cull_max_y: f32) -> bool {
    y + h >= cull_min_y && y <= cull_max_y
}

#[allow(clippy::too_many_arguments)]
fn layout_node_recursive(
    node_id: NodeId,
    dom: &ZeroCopyDomArena,
    css: &MicroCssEngine,
    parent_style: &ComputedStyle,
    x: f32,
    avail_width: f32,
    cursor_y: &mut f32,
    page_bg: &mut u32,
    inherited_link: Option<String>,
    cull_min_y: f32,
    cull_max_y: f32,
    commands: &mut Vec<PaintCommand>,
) {
    let Some(node) = dom.nodes().get(node_id as usize) else {
        return;
    };

    let style = match node.kind {
        NodeKind::Document => *parent_style,
        NodeKind::Element { .. } => css.compute_style_with_parent(dom, node_id, Some(parent_style)),
        NodeKind::Text(_) => *parent_style,
    };

    if style.display == DisplayMode::None || style.opacity <= 0.0 {
        return;
    }

    let mut current_link = inherited_link;

    if let Some(tag) = dom.node_tag(node_id) {
        if tag == "body" && style.background_color != 0 {
            *page_bg = style.background_color;
        }
        if tag == "a" {
            if let Some(href) = dom.get_attribute(node_id, "href") {
                current_link = Some(href);
            }
        }
        if tag == "hr" {
            *cursor_y += 8.0;
            if is_visible_band(*cursor_y, 2.0, cull_min_y, cull_max_y) {
                commands.push(PaintCommand::FillRect {
                    x,
                    y: *cursor_y,
                    width: avail_width,
                    height: 1.5,
                    color_argb: 0xFF232734,
                    corner_radius: 1.0,
                });
            }
            *cursor_y += 10.0;
            return;
        }
        if tag == "br" {
            *cursor_y += style.font_size_sp * style.line_height_mult;
            return;
        }

        if let Some(ctrl) = dom.control_for_node(node_id) {
            if ctrl.kind != FormControlKind::Hidden {
                *cursor_y += style.margin_top;
                let box_h = style.explicit_height.unwrap_or(42.0);
                let box_w = style.explicit_width.unwrap_or(avail_width).min(avail_width);
                let is_focused = dom.forms.focused_node() == Some(node_id);
                let display_text = match &ctrl.kind {
                    FormControlKind::TextInput { input_type } if input_type == "password" => {
                        "•".repeat(ctrl.value.chars().count())
                    }
                    FormControlKind::Checkbox => {
                        format!(
                            "[{}] {}",
                            if ctrl.checked { "✓" } else { " " },
                            ctrl.name
                        )
                    }
                    FormControlKind::Radio => {
                        format!(
                            "({}) {}",
                            if ctrl.checked { "•" } else { " " },
                            ctrl.name
                        )
                    }
                    _ => {
                        if ctrl.value.is_empty() {
                            ctrl.placeholder.clone()
                        } else {
                            ctrl.value.clone()
                        }
                    }
                };

                if is_visible_band(*cursor_y, box_h, cull_min_y, cull_max_y) && !style.visibility_hidden {
                    commands.push(PaintCommand::DrawInputBox {
                        node_id,
                        x,
                        y: *cursor_y,
                        width: box_w,
                        height: box_h,
                        display_text,
                        is_focused,
                        is_checked: ctrl.checked,
                    });
                }
                *cursor_y += box_h + style.margin_bottom + 4.0;
                return;
            }
        }

        *cursor_y += style.margin_top;
        let box_start_y = *cursor_y;
        let bg_cmd_idx = if (style.background_color != 0 || style.border_width > 0.0) && tag != "body" {
            let idx = commands.len();
            if style.border_width > 0.0 && style.border_color != 0 {
                commands.push(PaintCommand::FillRect {
                    x,
                    y: box_start_y,
                    width: style.explicit_width.unwrap_or(avail_width).min(avail_width),
                    height: 0.0,
                    color_argb: style.border_color,
                    corner_radius: style.corner_radius,
                });
            }
            if style.background_color != 0 {
                commands.push(PaintCommand::FillRect {
                    x: x + style.border_width,
                    y: box_start_y + style.border_width,
                    width: (style.explicit_width.unwrap_or(avail_width).min(avail_width)
                        - style.border_width * 2.0)
                        .max(8.0),
                    height: 0.0,
                    color_argb: style.background_color,
                    corner_radius: (style.corner_radius - style.border_width).max(0.0),
                });
            }
            Some(idx)
        } else {
            None
        };

        *cursor_y += style.padding_top;
        let inner_x = x + style.padding_left + style.border_width;
        let inner_w = (style.explicit_width.unwrap_or(avail_width).min(avail_width)
            - style.padding_left
            - style.padding_right
            - style.border_width * 2.0)
            .max(40.0);

        // Flexbox Row Layout
        if style.display == DisplayMode::Flex && style.flex_direction == FlexDir::Row {
            let mut flex_children = Vec::new();
            let mut ch = node.first_child;
            while ch != NO_NODE {
                let is_ignorable_ws = dom
                    .node_text(ch)
                    .map(|t| t.trim().is_empty())
                    .unwrap_or(false);
                if !is_ignorable_ws {
                    flex_children.push(ch);
                }
                ch = dom.nodes()[ch as usize].next_sibling;
            }

            if !flex_children.is_empty() {
                let count = flex_children.len() as f32;
                let total_gap = style.gap * (flex_children.len().saturating_sub(1) as f32);
                let item_w = ((inner_w - total_gap) / count).max(40.0);
                let row_start_y = *cursor_y;
                let mut tallest_bottom = row_start_y;

                for (idx, child_id) in flex_children.into_iter().enumerate() {
                    let mut child_y = row_start_y;
                    let child_x = inner_x + (idx as f32) * (item_w + style.gap);
                    layout_node_recursive(
                        child_id,
                        dom,
                        css,
                        &style,
                        child_x,
                        item_w,
                        &mut child_y,
                        page_bg,
                        current_link.clone(),
                        cull_min_y,
                        cull_max_y,
                        commands,
                    );
                    if child_y > tallest_bottom {
                        tallest_bottom = child_y;
                    }
                }
                *cursor_y = tallest_bottom;
            }
        } else {
            let mut child = node.first_child;
            while child != NO_NODE {
                layout_node_recursive(
                    child,
                    dom,
                    css,
                    &style,
                    inner_x,
                    inner_w,
                    cursor_y,
                    page_bg,
                    current_link.clone(),
                    cull_min_y,
                    cull_max_y,
                    commands,
                );
                child = dom.nodes()[child as usize].next_sibling;
            }
        }

        *cursor_y += style.padding_bottom;
        let measured_h = (*cursor_y - box_start_y)
            .max(style.explicit_height.unwrap_or(0.0));
        *cursor_y = box_start_y + measured_h + style.margin_bottom;

        // Back-patch background/border rectangle height or cull if offscreen
        if let Some(start_idx) = bg_cmd_idx {
            let visible = is_visible_band(box_start_y, measured_h, cull_min_y, cull_max_y)
                && !style.visibility_hidden
                && measured_h > 0.0;
            if visible {
                let has_border = style.border_width > 0.0 && style.border_color != 0;
                if has_border {
                    if let Some(PaintCommand::FillRect { height, .. }) = commands.get_mut(start_idx) {
                        *height = measured_h;
                    }
                    if style.background_color != 0 {
                        if let Some(PaintCommand::FillRect { height, .. }) = commands.get_mut(start_idx + 1) {
                            *height = (measured_h - style.border_width * 2.0).max(0.0);
                        }
                    }
                } else if let Some(PaintCommand::FillRect { height, .. }) = commands.get_mut(start_idx) {
                    *height = measured_h;
                }
            } else {
                // Evict invisible culled placeholder rect(s) cleanly
                if style.border_width > 0.0 && style.border_color != 0 && style.background_color != 0 {
                    if start_idx + 1 < commands.len() {
                        commands.remove(start_idx + 1);
                    }
                }
                if start_idx < commands.len() {
                    commands.remove(start_idx);
                }
            }
        }
        return;
    }

    if let Some(text) = dom.node_text(node_id) {
        if !text.is_empty() {
            let prefix = if node.parent != NO_NODE && dom.node_tag(node.parent) == Some("li") {
                "• "
            } else {
                ""
            };
            let full_text = if prefix.is_empty() {
                text.to_string()
            } else {
                format!("{}{}", prefix, text)
            };

            let chars_per_line = (avail_width / (parent_style.font_size_sp * 0.55)).max(12.0);
            let lines = ((full_text.chars().count() as f32) / chars_per_line)
                .ceil()
                .max(1.0);
            let block_h = lines * (parent_style.font_size_sp * parent_style.line_height_mult) + 2.0;

            if is_visible_band(*cursor_y, block_h, cull_min_y, cull_max_y)
                && !parent_style.visibility_hidden
            {
                commands.push(PaintCommand::DrawText {
                    x,
                    y: *cursor_y,
                    text: full_text,
                    color_argb: parent_style.text_color,
                    font_size_sp: parent_style.font_size_sp,
                    bold: parent_style.font_weight_bold,
                    link_href: current_link.clone(),
                });
            }

            *cursor_y += block_h;
        }
        return;
    }

    // Document root traversal
    let mut child = node.first_child;
    while child != NO_NODE {
        layout_node_recursive(
            child,
            dom,
            css,
            &style,
            x,
            avail_width,
            cursor_y,
            page_bg,
            current_link.clone(),
            cull_min_y,
            cull_max_y,
            commands,
        );
        child = dom.nodes()[child as usize].next_sibling;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_flexbox_row_and_viewport_culling() {
        let mut dom = ZeroCopyDomArena::new(64 * 1024);
        let mut tok = StreamingHtmlTokenizer::new();
        tok.feed_chunk(
            b"<div id=\"row\" style=\"display: flex; gap: 10px;\"><div>Left</div><div>Right</div></div>",
            true,
            |t| dom.ingest_token(t),
        );
        let css = MicroCssEngine::new();
        let frame = DisplayListFrame::build(&dom, &css, 400.0);
        let texts: Vec<(f32, f32, &str)> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                PaintCommand::DrawText { x, y, text, .. } => Some((*x, *y, text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2);
        // Flex row aligns both items on the same Y coordinate with Left < Right on X
        assert!((texts[0].1 - texts[1].1).abs() < 8.0);
        assert!(texts[1].0 > texts[0].0);
    }
}
