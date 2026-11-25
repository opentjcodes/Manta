use crate::cpp_bridge::{now_us, wrap_text_lines};
use crate::css_engine::{
    ComputedStyle, DisplayMode, FlexDir, JustifyContent, MicroCssEngine, TextAlign,
};
use crate::dom_arena::{NodeId, ZeroCopyDomArena, NO_NODE};
use crate::input_forms::FormControlKind;

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
    DrawMediaBox {
        node_id: NodeId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        label: String,
        media_src: String,
        is_video: bool,
    },
}

#[derive(Debug, Clone)]
pub struct DisplayListFrame {
    pub page_title: String,
    pub background_argb: u32,
    pub content_height: f32,
    pub commands: Vec<PaintCommand>,
    pub dom_nodes_count: usize,
    pub ram_used_bytes: usize,
    pub layout_time_us: u32,
    pub budget_capped: bool,
}

struct LayoutPacer {
    start_us: u64,
    budget_us: u64,
    visited_nodes: u32,
    capped: bool,
}

impl LayoutPacer {
    fn new(budget_us: u64) -> Self {
        Self {
            start_us: now_us(),
            budget_us,
            visited_nodes: 0,
            capped: false,
        }
    }

    #[inline]
    fn check_exceeded(&mut self) -> bool {
        if self.capped {
            return true;
        }
        self.visited_nodes = self.visited_nodes.wrapping_add(1);
        if self.budget_us > 0 && (self.visited_nodes & 0x3F) == 0 {
            if now_us().saturating_sub(self.start_us) >= self.budget_us {
                self.capped = true;
                return true;
            }
        }
        false
    }
}

impl DisplayListFrame {
    pub fn build(
        dom: &ZeroCopyDomArena,
        css: &MicroCssEngine,
        viewport_width: f32,
    ) -> DisplayListFrame {
        Self::build_paced(dom, css, viewport_width, 0.0, f32::INFINITY, 6_000)
    }

    pub fn build_culled(
        dom: &ZeroCopyDomArena,
        css: &MicroCssEngine,
        viewport_width: f32,
        scroll_y: f32,
        viewport_height: f32,
    ) -> DisplayListFrame {
        Self::build_paced(dom, css, viewport_width, scroll_y, viewport_height, 6_000)
    }

    /// Fixed-Latency Paced Frame Builder:
    /// 1. Single-pass precomputed CSS style tree (zero duplicate style recomputations).
    /// 2. C++ zero-allocation UTF-8 text wrapping.
    /// 3. Hard microsecond compute pacing (`budget_us`) so heavy & light sites stay within
    ///    the exact same deterministic compute budget without freezing or crashing.
    pub fn build_paced(
        dom: &ZeroCopyDomArena,
        css: &MicroCssEngine,
        viewport_width: f32,
        scroll_y: f32,
        viewport_height: f32,
        budget_us: u64,
    ) -> DisplayListFrame {
        let frame_start = now_us();
        let mut pacer = LayoutPacer::new(budget_us);
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
            let style_budget = if budget_us > 0 { budget_us / 2 } else { 0 };
            let styles = css.compute_style_tree(dom, style_budget);
            layout_node_recursive(
                0,
                dom,
                &styles,
                horizontal_padding,
                (viewport_width - (horizontal_padding * 2.0)).max(80.0),
                &mut cursor_y,
                &mut page_bg,
                None,
                None,
                cull_min_y,
                cull_max_y,
                &mut pacer,
                &mut commands,
            );
        }

        let elapsed = now_us().saturating_sub(frame_start).min(u32::MAX as u64) as u32;
        DisplayListFrame {
            page_title: dom.page_title.clone(),
            background_argb: page_bg,
            content_height: cursor_y + 32.0,
            commands,
            dom_nodes_count: dom.nodes().len(),
            ram_used_bytes: dom.used_bytes(),
            layout_time_us: elapsed,
            budget_capped: pacer.capped,
        }
    }

    pub fn to_compact_wire_string(&self) -> String {
        let mut out = String::with_capacity(256 + self.commands.len() * 48);
        out.push_str(&format!(
            "MANTA_FRAME|title={}|bg=#{:08X}|height={:.1}|nodes={}|ramKB={:.2}|us={}|capped={}\n",
            self.page_title.replace('|', " "),
            self.background_argb,
            self.content_height,
            self.dom_nodes_count,
            self.ram_used_bytes as f32 / 1024.0,
            self.layout_time_us,
            u8::from(self.budget_capped)
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
                PaintCommand::DrawMediaBox {
                    node_id,
                    x,
                    y,
                    width,
                    height,
                    label,
                    media_src,
                    is_video,
                } => {
                    out.push_str(&format!(
                        "MEDIA|{}|{:.1}|{:.1}|{:.1}|{:.1}|{}|{}|{}\n",
                        node_id,
                        x,
                        y,
                        width,
                        height,
                        u8::from(*is_video),
                        media_src.replace('|', ""),
                        label.replace('\n', " ").replace('|', " ")
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

struct InlineSegment<'a> {
    text: &'a str,
    color_argb: u32,
    font_size_sp: f32,
    bold: bool,
    line_height_mult: f32,
    link_href: Option<String>,
}

fn collect_inline_segments<'a>(
    node_id: NodeId,
    dom: &'a ZeroCopyDomArena,
    styles: &[ComputedStyle],
    inherited_link: Option<String>,
    out: &mut Vec<InlineSegment<'a>>,
) {
    let Some(node) = dom.nodes().get(node_id as usize) else {
        return;
    };
    let style = styles[node_id as usize];
    if style.display == DisplayMode::None || style.opacity <= 0.0 {
        return;
    }
    if let Some(text) = dom.node_text(node_id) {
        if !text.is_empty() {
            out.push(InlineSegment {
                text,
                color_argb: style.text_color,
                font_size_sp: style.font_size_sp,
                bold: style.font_weight_bold,
                line_height_mult: style.line_height_mult,
                link_href: inherited_link,
            });
        }
        return;
    }
    let mut link = inherited_link;
    if dom.node_tag(node_id) == Some("a") {
        if let Some(href) = dom.get_attribute(node_id, "href") {
            link = Some(href);
        }
    }
    let mut child = node.first_child;
    while child != NO_NODE {
        collect_inline_segments(child, dom, styles, link.clone(), out);
        child = dom.nodes()[child as usize].next_sibling;
    }
}

#[allow(clippy::too_many_arguments)]
fn flush_inline_segments(
    segments: Vec<InlineSegment<'_>>,
    base_x: f32,
    avail_width: f32,
    text_align: TextAlign,
    cursor_y: &mut f32,
    visibility_hidden: bool,
    list_prefix: Option<&str>,
    cull_min_y: f32,
    cull_max_y: f32,
    commands: &mut Vec<PaintCommand>,
) {
    if segments.is_empty() {
        return;
    }

    // Fast path for single-segment paragraphs using C++ zero-allocation line wrapper!
    if segments.len() == 1 {
        let seg = &segments[0];
        let owned;
        let full_text: &str = if let Some(pfx) = list_prefix {
            owned = format!("{}{}", pfx, seg.text);
            &owned
        } else {
            seg.text
        };
        let char_w = seg.font_size_sp * 0.54;
        let line_h = seg.font_size_sp * seg.line_height_mult + 2.0;
        let mut starts = [0u32; 64];
        let mut lens = [0u32; 64];
        let n_lines = wrap_text_lines(full_text, char_w, avail_width, &mut starts, &mut lens);

        for i in 0..n_lines {
            let s = starts[i] as usize;
            let e = s + lens[i] as usize;
            if let Some(line_slice) = full_text.get(s..e) {
                if !visibility_hidden && is_visible_band(*cursor_y, line_h, cull_min_y, cull_max_y)
                {
                    let text_w = (line_slice.chars().count() as f32) * char_w;
                    let aligned_x = match text_align {
                        TextAlign::Center => base_x + ((avail_width - text_w) * 0.5).max(0.0),
                        TextAlign::Right => base_x + (avail_width - text_w).max(0.0),
                        TextAlign::Left => base_x,
                    };
                    commands.push(PaintCommand::DrawText {
                        x: aligned_x,
                        y: *cursor_y,
                        text: line_slice.to_string(),
                        color_argb: seg.color_argb,
                        font_size_sp: seg.font_size_sp,
                        bold: seg.bold,
                        link_href: seg.link_href.clone(),
                    });
                }
                *cursor_y += line_h;
            }
        }
        return;
    }

    let mut line_x = base_x;
    let mut max_line_h = 0.0f32;
    let mut prefix_pending = list_prefix;

    for seg in segments {
        let char_w = seg.font_size_sp * 0.54;
        let line_h = seg.font_size_sp * seg.line_height_mult + 2.0;
        if line_h > max_line_h {
            max_line_h = line_h;
        }
        let mut current_chunk = String::new();
        if let Some(pfx) = prefix_pending.take() {
            current_chunk.push_str(pfx);
            line_x += (pfx.chars().count() as f32) * char_w;
        }
        let mut chunk_start_x = if current_chunk.is_empty() { line_x } else { base_x };

        for word in seg.text.split_whitespace() {
            let word_w = (word.chars().count() as f32 + 1.0) * char_w;
            if line_x > base_x && line_x + word_w > base_x + avail_width {
                if !current_chunk.is_empty() && !visibility_hidden {
                    if is_visible_band(*cursor_y, max_line_h, cull_min_y, cull_max_y) {
                        commands.push(PaintCommand::DrawText {
                            x: chunk_start_x,
                            y: *cursor_y,
                            text: std::mem::take(&mut current_chunk),
                            color_argb: seg.color_argb,
                            font_size_sp: seg.font_size_sp,
                            bold: seg.bold,
                            link_href: seg.link_href.clone(),
                        });
                    } else {
                        current_chunk.clear();
                    }
                }
                *cursor_y += max_line_h;
                line_x = base_x;
                chunk_start_x = base_x;
            }
            if !current_chunk.is_empty() && !current_chunk.ends_with(' ') {
                current_chunk.push(' ');
            }
            current_chunk.push_str(word);
            line_x += word_w;
        }

        if !current_chunk.is_empty() && !visibility_hidden {
            let text_w = (current_chunk.chars().count() as f32) * char_w;
            let aligned_x = if chunk_start_x == base_x && line_x <= base_x + avail_width {
                match text_align {
                    TextAlign::Center => base_x + ((avail_width - text_w) * 0.5).max(0.0),
                    TextAlign::Right => base_x + (avail_width - text_w).max(0.0),
                    TextAlign::Left => chunk_start_x,
                }
            } else {
                chunk_start_x
            };
            if is_visible_band(*cursor_y, max_line_h, cull_min_y, cull_max_y) {
                commands.push(PaintCommand::DrawText {
                    x: aligned_x,
                    y: *cursor_y,
                    text: current_chunk,
                    color_argb: seg.color_argb,
                    font_size_sp: seg.font_size_sp,
                    bold: seg.bold,
                    link_href: seg.link_href,
                });
            }
        }
    }

    *cursor_y += max_line_h.max(18.0);
}

#[allow(clippy::too_many_arguments)]
fn layout_node_recursive(
    node_id: NodeId,
    dom: &ZeroCopyDomArena,
    styles: &[ComputedStyle],
    x: f32,
    avail_width: f32,
    cursor_y: &mut f32,
    page_bg: &mut u32,
    inherited_link: Option<String>,
    list_ordinal: Option<usize>,
    cull_min_y: f32,
    cull_max_y: f32,
    pacer: &mut LayoutPacer,
    commands: &mut Vec<PaintCommand>,
) {
    if pacer.check_exceeded() {
        return;
    }
    let Some(node) = dom.nodes().get(node_id as usize) else {
        return;
    };

    let style = styles[node_id as usize];
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

        if matches!(tag, "img" | "video" | "audio") {
            *cursor_y += style.margin_top;
            let default_h = if tag == "audio" { 48.0 } else { 180.0 };
            let box_w = resolve_box_width(&style, avail_width);
            let box_h = style.explicit_height.unwrap_or(default_h);
            let src = dom.get_attribute(node_id, "src").unwrap_or_default();
            let label = dom
                .get_attribute(node_id, "alt")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("<{}>", tag));
            if is_visible_band(*cursor_y, box_h, cull_min_y, cull_max_y) && !style.visibility_hidden
            {
                commands.push(PaintCommand::DrawMediaBox {
                    node_id,
                    x,
                    y: *cursor_y,
                    width: box_w,
                    height: box_h,
                    label,
                    media_src: src,
                    is_video: tag == "video",
                });
            }
            *cursor_y += box_h + style.margin_bottom + 6.0;
            return;
        }

        if let Some(ctrl) = dom.control_for_node(node_id) {
            if ctrl.kind != FormControlKind::Hidden {
                *cursor_y += style.margin_top;
                let box_h = style.explicit_height.unwrap_or(42.0);
                let box_w = resolve_box_width(&style, avail_width);
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

                if is_visible_band(*cursor_y, box_h, cull_min_y, cull_max_y)
                    && !style.visibility_hidden
                {
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
        let resolved_w = resolve_box_width(&style, avail_width);

        let bg_cmd_idx = if (style.background_color != 0 || style.border_width > 0.0)
            && tag != "body"
        {
            let idx = commands.len();
            if style.border_width > 0.0 && style.border_color != 0 {
                commands.push(PaintCommand::FillRect {
                    x,
                    y: box_start_y,
                    width: resolved_w,
                    height: 0.0,
                    color_argb: style.border_color,
                    corner_radius: style.corner_radius,
                });
            }
            if style.background_color != 0 {
                commands.push(PaintCommand::FillRect {
                    x: x + style.border_width,
                    y: box_start_y + style.border_width,
                    width: (resolved_w - style.border_width * 2.0).max(8.0),
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
        let inner_w =
            (resolved_w - style.padding_left - style.padding_right - style.border_width * 2.0)
                .max(40.0);

        if style.display == DisplayMode::Table {
            let mut rows = Vec::new();
            collect_table_rows(node_id, dom, &mut rows);
            for row_id in rows {
                if pacer.check_exceeded() {
                    break;
                }
                let cells = collect_direct_elements(row_id, dom);
                if cells.is_empty() {
                    continue;
                }
                let cell_w = (inner_w / cells.len() as f32).max(32.0);
                let row_y = *cursor_y;
                let mut tallest = row_y;
                for (c_idx, cell_id) in cells.into_iter().enumerate() {
                    let mut cy = row_y;
                    let cx = inner_x + (c_idx as f32) * cell_w;
                    layout_node_recursive(
                        cell_id,
                        dom,
                        styles,
                        cx,
                        cell_w,
                        &mut cy,
                        page_bg,
                        current_link.clone(),
                        None,
                        cull_min_y,
                        cull_max_y,
                        pacer,
                        commands,
                    );
                    if cy > tallest {
                        tallest = cy;
                    }
                }
                *cursor_y = tallest;
            }
        } else if style.display == DisplayMode::Grid && style.grid_columns >= 2 {
            let cols = style.grid_columns as usize;
            let grid_children = collect_non_ws_children(node_id, dom);
            let total_gap = style.gap * (cols.saturating_sub(1) as f32);
            let col_w = ((inner_w - total_gap) / cols as f32).max(32.0);
            for row_chunk in grid_children.chunks(cols) {
                if pacer.check_exceeded() {
                    break;
                }
                let row_y = *cursor_y;
                let mut tallest = row_y;
                for (c_idx, &child_id) in row_chunk.iter().enumerate() {
                    let mut cy = row_y;
                    let cx = inner_x + (c_idx as f32) * (col_w + style.gap);
                    layout_node_recursive(
                        child_id,
                        dom,
                        styles,
                        cx,
                        col_w,
                        &mut cy,
                        page_bg,
                        current_link.clone(),
                        None,
                        cull_min_y,
                        cull_max_y,
                        pacer,
                        commands,
                    );
                    if cy > tallest {
                        tallest = cy;
                    }
                }
                *cursor_y = tallest + style.gap;
            }
        } else if style.display == DisplayMode::Flex && style.flex_direction == FlexDir::Row {
            let flex_children = collect_non_ws_children(node_id, dom);
            if !flex_children.is_empty() {
                let count = flex_children.len() as f32;
                let total_gap = style.gap * (flex_children.len().saturating_sub(1) as f32);
                let item_w = ((inner_w - total_gap) / count).max(40.0);
                let start_offset = match style.justify_content {
                    JustifyContent::Center => {
                        let used = count * item_w + total_gap;
                        ((inner_w - used) * 0.5).max(0.0)
                    }
                    JustifyContent::FlexEnd => {
                        let used = count * item_w + total_gap;
                        (inner_w - used).max(0.0)
                    }
                    _ => 0.0,
                };
                let row_start_y = *cursor_y;
                let mut tallest_bottom = row_start_y;

                for (idx, child_id) in flex_children.into_iter().enumerate() {
                    if pacer.check_exceeded() {
                        break;
                    }
                    let mut child_y = row_start_y;
                    let child_x = inner_x + start_offset + (idx as f32) * (item_w + style.gap);
                    layout_node_recursive(
                        child_id,
                        dom,
                        styles,
                        child_x,
                        item_w,
                        &mut child_y,
                        page_bg,
                        current_link.clone(),
                        None,
                        cull_min_y,
                        cull_max_y,
                        pacer,
                        commands,
                    );
                    if child_y > tallest_bottom {
                        tallest_bottom = child_y;
                    }
                }
                *cursor_y = tallest_bottom;
            }
        } else {
            let mut inline_run: Vec<InlineSegment<'_>> = Vec::new();
            let mut child = node.first_child;
            let is_ol = tag == "ol";
            let mut ol_counter = 1usize;

            let li_prefix_owned = if tag == "li" {
                match list_ordinal {
                    Some(n) => Some(format!("{}. ", n)),
                    None => Some("• ".to_string()),
                }
            } else {
                None
            };
            let mut li_prefix_unused = li_prefix_owned.as_deref();

            while child != NO_NODE {
                if pacer.check_exceeded() {
                    break;
                }
                let is_inline_child = if dom.node_text(child).is_some() {
                    true
                } else {
                    styles[child as usize].display == DisplayMode::Inline
                        && dom.control_for_node(child).is_none()
                };

                if is_inline_child {
                    collect_inline_segments(
                        child,
                        dom,
                        styles,
                        current_link.clone(),
                        &mut inline_run,
                    );
                } else {
                    if !inline_run.is_empty() {
                        flush_inline_segments(
                            std::mem::take(&mut inline_run),
                            inner_x,
                            inner_w,
                            style.text_align,
                            cursor_y,
                            style.visibility_hidden,
                            li_prefix_unused.take(),
                            cull_min_y,
                            cull_max_y,
                            commands,
                        );
                    }
                    let next_ord = if is_ol && dom.node_tag(child) == Some("li") {
                        let n = ol_counter;
                        ol_counter += 1;
                        Some(n)
                    } else {
                        None
                    };
                    layout_node_recursive(
                        child,
                        dom,
                        styles,
                        inner_x,
                        inner_w,
                        cursor_y,
                        page_bg,
                        current_link.clone(),
                        next_ord,
                        cull_min_y,
                        cull_max_y,
                        pacer,
                        commands,
                    );
                }
                child = dom.nodes()[child as usize].next_sibling;
            }
            if !inline_run.is_empty() {
                flush_inline_segments(
                    inline_run,
                    inner_x,
                    inner_w,
                    style.text_align,
                    cursor_y,
                    style.visibility_hidden,
                    li_prefix_unused.take(),
                    cull_min_y,
                    cull_max_y,
                    commands,
                );
            }
        }

        *cursor_y += style.padding_bottom;
        let measured_h = (*cursor_y - box_start_y)
            .max(style.explicit_height.unwrap_or(0.0))
            .max(style.min_height.unwrap_or(0.0));
        *cursor_y = box_start_y + measured_h + style.margin_bottom;

        if let Some(start_idx) = bg_cmd_idx {
            let visible = is_visible_band(box_start_y, measured_h, cull_min_y, cull_max_y)
                && !style.visibility_hidden
                && measured_h > 0.0;
            if visible {
                let has_border = style.border_width > 0.0 && style.border_color != 0;
                if has_border {
                    if let Some(PaintCommand::FillRect { height, .. }) = commands.get_mut(start_idx)
                    {
                        *height = measured_h;
                    }
                    if style.background_color != 0 {
                        if let Some(PaintCommand::FillRect { height, .. }) =
                            commands.get_mut(start_idx + 1)
                        {
                            *height = (measured_h - style.border_width * 2.0).max(0.0);
                        }
                    }
                } else if let Some(PaintCommand::FillRect { height, .. }) =
                    commands.get_mut(start_idx)
                {
                    *height = measured_h;
                }
            } else {
                if style.border_width > 0.0
                    && style.border_color != 0
                    && style.background_color != 0
                {
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
            let seg = InlineSegment {
                text,
                color_argb: style.text_color,
                font_size_sp: style.font_size_sp,
                bold: style.font_weight_bold,
                line_height_mult: style.line_height_mult,
                link_href: current_link,
            };
            flush_inline_segments(
                vec![seg],
                x,
                avail_width,
                style.text_align,
                cursor_y,
                style.visibility_hidden,
                None,
                cull_min_y,
                cull_max_y,
                commands,
            );
        }
        return;
    }

    let mut child = node.first_child;
    while child != NO_NODE {
        if pacer.check_exceeded() {
            break;
        }
        layout_node_recursive(
            child,
            dom,
            styles,
            x,
            avail_width,
            cursor_y,
            page_bg,
            current_link.clone(),
            None,
            cull_min_y,
            cull_max_y,
            pacer,
            commands,
        );
        child = dom.nodes()[child as usize].next_sibling;
    }
}

#[inline]
fn resolve_box_width(style: &ComputedStyle, avail_width: f32) -> f32 {
    let mut w = if let Some(pct) = style.width_percent {
        (avail_width * pct).max(32.0)
    } else {
        style.explicit_width.unwrap_or(avail_width).min(avail_width)
    };
    if let Some(mw) = style.max_width {
        w = w.min(mw);
    }
    w
}

fn collect_non_ws_children(parent_id: NodeId, dom: &ZeroCopyDomArena) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut ch = dom.nodes()[parent_id as usize].first_child;
    while ch != NO_NODE {
        let is_ignorable_ws = dom
            .node_text(ch)
            .map(|t| t.trim().is_empty())
            .unwrap_or(false);
        if !is_ignorable_ws {
            out.push(ch);
        }
        ch = dom.nodes()[ch as usize].next_sibling;
    }
    out
}

fn collect_direct_elements(parent_id: NodeId, dom: &ZeroCopyDomArena) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut ch = dom.nodes()[parent_id as usize].first_child;
    while ch != NO_NODE {
        if dom.node_tag(ch).is_some() {
            out.push(ch);
        }
        ch = dom.nodes()[ch as usize].next_sibling;
    }
    out
}

fn collect_table_rows(node_id: NodeId, dom: &ZeroCopyDomArena, out: &mut Vec<NodeId>) {
    let mut ch = dom.nodes()[node_id as usize].first_child;
    while ch != NO_NODE {
        match dom.node_tag(ch) {
            Some("tr") => out.push(ch),
            Some("thead") | Some("tbody") | Some("tfoot") => collect_table_rows(ch, dom, out),
            _ => {}
        }
        ch = dom.nodes()[ch as usize].next_sibling;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_tokenizer::StreamingHtmlTokenizer;

    #[test]
    fn test_inline_flow_grid_table_ol_and_flexbox_layout() {
        let mut dom = ZeroCopyDomArena::new(64 * 1024);
        let mut tok = StreamingHtmlTokenizer::new();
        tok.feed_chunk(
            b"<p>Click <a href=\"https://manta.dev\">here</a> now</p>\
              <div id=\"row\" style=\"display: flex; gap: 10px;\"><div>Left</div><div>Right</div></div>\
              <table><tr><th>Name</th><th>Score</th></tr><tr><td>Manta</td><td>99</td></tr></table>\
              <ol><li>First</li><li>Second</li></ol>",
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

        // Inline segments inside <p>: "Click", "here", "now" share same horizontal line
        assert!(texts.len() >= 9);
        assert!((texts[0].1 - texts[1].1).abs() < 1.0);
        assert!(texts[1].0 > texts[0].0);

        // Flex row places Left & Right side by side
        let left = texts.iter().find(|t| t.2 == "Left").unwrap();
        let right = texts.iter().find(|t| t.2 == "Right").unwrap();
        assert!((left.1 - right.1).abs() < 8.0);
        assert!(right.0 > left.0);

        // Table places Name & Score on the same horizontal row in separate columns
        let th_name = texts.iter().find(|t| t.2 == "Name").unwrap();
        let th_score = texts.iter().find(|t| t.2 == "Score").unwrap();
        assert!((th_name.1 - th_score.1).abs() < 8.0);
        assert!(th_score.0 > th_name.0);

        // Ordered list numbering
        assert!(texts.iter().any(|t| t.2 == "1. First"));
        assert!(texts.iter().any(|t| t.2 == "2. Second"));
    }
}
