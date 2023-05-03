use std::cmp::min;

use helix_core::doc_formatter::{DocumentFormatter, GraphemeSource, TextFormat};
use helix_core::graphemes::Grapheme;
use helix_core::str_utils::char_to_byte_idx;
use helix_core::syntax::Highlight;
use helix_core::syntax::HighlightEvent;
use helix_core::text_annotations::TextAnnotations;
use helix_core::{visual_offset_from_block, Position, RopeSlice};
use helix_view::editor::{WhitespaceConfig, WhitespaceRenderValue};
use helix_view::graphics::Rect;
use helix_view::theme::Style;
use helix_view::view::ViewPosition;
use helix_view::{Document, Theme};
use tui::buffer::Buffer as Surface;

use crate::ui::text_decorations::DecorationManager;

/// A wrapper around a HighlightIterator
/// that merges the layered highlights to create the final text style
/// and yields the active text style and the char_idx where the active
/// style will have to be recomputed.
struct StyleIter<'a, H: Iterator<Item = HighlightEvent>> {
    text_style: Style,
    active_highlights: Vec<Highlight>,
    highlight_iter: H,
    theme: &'a Theme,
}

impl<H: Iterator<Item = HighlightEvent>> Iterator for StyleIter<'_, H> {
    type Item = (Style, usize);
    fn next(&mut self) -> Option<(Style, usize)> {
        while let Some(event) = self.highlight_iter.next() {
            match event {
                HighlightEvent::HighlightStart(highlights) => {
                    self.active_highlights.push(highlights)
                }
                HighlightEvent::HighlightEnd => {
                    self.active_highlights.pop();
                }
                HighlightEvent::Source { end, .. } => {
                    let style = self
                        .active_highlights
                        .iter()
                        .fold(self.text_style, |acc, span| {
                            acc.patch(self.theme.highlight(span.0))
                        });
                    return Some((style, end));
                }
            }
        }
        None
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct LinePos {
    /// Indicates whether the given visual line
    /// is the first visual line of the given document line
    pub first_visual_line: bool,
    /// The line index of the document line that contains the given visual line
    pub doc_line: usize,
    /// Vertical offset from the top of the inner view area
    pub visual_line: u16,
}

#[allow(clippy::too_many_arguments)]
pub fn render_document(
    surface: &mut Surface,
    viewport: Rect,
    doc: &Document,
    offset: ViewPosition,
    doc_annotations: &TextAnnotations,
    highlight_iter: impl Iterator<Item = HighlightEvent>,
    theme: &Theme,
    decorations: DecorationManager,
) {
    let mut renderer = TextRenderer::new(surface, doc, theme, offset.horizontal_offset, viewport);
    render_text(
        &mut renderer,
        doc.text().slice(..),
        offset,
        &doc.text_format(viewport.width, Some(theme)),
        doc_annotations,
        highlight_iter,
        theme,
        decorations,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_text<'t>(
    renderer: &mut TextRenderer,
    text: RopeSlice<'t>,
    offset: ViewPosition,
    text_fmt: &TextFormat,
    text_annotations: &TextAnnotations,
    highlight_iter: impl Iterator<Item = HighlightEvent>,
    theme: &Theme,
    mut decorations: DecorationManager,
) {
    let mut row_off = visual_offset_from_block(
        text,
        offset.anchor,
        offset.anchor,
        text_fmt,
        text_annotations,
    )
    .0
    .row;
    row_off += offset.vertical_offset;

    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_fmt, text_annotations, offset.anchor);
    let mut styles = StyleIter {
        text_style: renderer.text_style,
        active_highlights: Vec::with_capacity(64),
        highlight_iter,
        theme,
    };
    let mut last_line_pos = LinePos {
        first_visual_line: false,
        doc_line: usize::MAX,
        visual_line: u16::MAX,
    };
    let mut is_in_indent_area = true;
    let mut last_line_indent_level = 0;
    let mut style_span = styles
        .next()
        .unwrap_or_else(|| (Style::default(), usize::MAX));
    let mut reached_view_top = false;

    loop {
        let Some(mut grapheme) = formatter.next() else { break };

        // skip any graphemes on visual lines before the block start
        if grapheme.visual_pos.row < row_off {
            if grapheme.char_idx >= style_span.1 {
                style_span = if let Some(style_span) = styles.next() {
                    style_span
                } else {
                    break;
                }
            }
            continue;
        }
        grapheme.visual_pos.row -= row_off;
        if !reached_view_top {
            decorations.prepare_for_rendering(grapheme.char_idx);
            reached_view_top = true;
        }

        // if the end of the viewport is reached stop rendering
        if grapheme.visual_pos.row as u16 >= renderer.viewport.height {
            break;
        }

        // apply decorations before rendering a new line
        if grapheme.visual_pos.row as u16 != last_line_pos.visual_line {
            // we initiate doc_line with usize::MAX because no file
            // can reach that size (memory allocations are limited to isize::MAX)
            // initially there is no "previous" line (so doc_line is set to usize::MAX)
            // in that case we don't need to draw indent guides/virtual text
            if last_line_pos.doc_line != usize::MAX {
                // draw indent guides for the last line
                renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
                is_in_indent_area = true;
                decorations.render_virtual_lines(renderer, last_line_pos)
            }
            last_line_pos = LinePos {
                first_visual_line: grapheme.line_idx != last_line_pos.doc_line,
                doc_line: grapheme.line_idx,
                visual_line: grapheme.visual_pos.row as u16,
            };
            decorations.decorate_line(renderer, last_line_pos);
        }

        // aquire the correct grapheme style
        while grapheme.char_idx >= style_span.1 {
            style_span = styles.next().unwrap_or((Style::default(), usize::MAX));
        }

        let grapheme_style = if let GraphemeSource::VirtualText { highlight } = grapheme.source {
            let style = renderer.text_style;
            if let Some(highlight) = highlight {
                style.patch(theme.highlight(highlight.0))
            } else {
                style
            }
        } else {
            style_span.0
        };
        decorations.decorate_grapheme(renderer, &grapheme);

        let virt = grapheme.is_virtual();
        renderer.draw_grapheme(
            grapheme.raw,
            grapheme_style,
            virt,
            &mut last_line_indent_level,
            &mut is_in_indent_area,
            grapheme.visual_pos,
        );
    }

    renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
    decorations.render_virtual_lines(renderer, last_line_pos)
}

#[derive(Debug)]
pub struct TextRenderer<'a> {
    pub surface: &'a mut Surface,
    pub text_style: Style,
    pub whitespace_style: Style,
    pub indent_guide_char: String,
    pub indent_guide_style: Style,
    pub newline: String,
    pub nbsp: String,
    pub space: String,
    pub tab: String,
    pub virtual_tab: String,
    pub indent_width: u16,
    pub starting_indent: usize,
    pub draw_indent_guides: bool,
    pub col_offset: usize,
    pub viewport: Rect,
}

impl<'a> TextRenderer<'a> {
    pub fn new(
        surface: &'a mut Surface,
        doc: &Document,
        theme: &Theme,
        col_offset: usize,
        viewport: Rect,
    ) -> TextRenderer<'a> {
        let editor_config = doc.config.load();
        let WhitespaceConfig {
            render: ws_render,
            characters: ws_chars,
        } = &editor_config.whitespace;

        let tab_width = doc.tab_width();
        let tab = if ws_render.tab() == WhitespaceRenderValue::All {
            std::iter::once(ws_chars.tab)
                .chain(std::iter::repeat(ws_chars.tabpad).take(tab_width - 1))
                .collect()
        } else {
            " ".repeat(tab_width)
        };
        let virtual_tab = " ".repeat(tab_width);
        let newline = if ws_render.newline() == WhitespaceRenderValue::All {
            ws_chars.newline.into()
        } else {
            " ".to_owned()
        };

        let space = if ws_render.space() == WhitespaceRenderValue::All {
            ws_chars.space.into()
        } else {
            " ".to_owned()
        };
        let nbsp = if ws_render.nbsp() == WhitespaceRenderValue::All {
            ws_chars.nbsp.into()
        } else {
            " ".to_owned()
        };

        let text_style = theme.get("ui.text");

        let indent_width = doc.indent_style.indent_width(tab_width) as u16;

        TextRenderer {
            surface,
            indent_guide_char: editor_config.indent_guides.character.into(),
            newline,
            nbsp,
            space,
            tab,
            virtual_tab,
            whitespace_style: theme.get("ui.virtual.whitespace"),
            indent_width,
            starting_indent: col_offset / indent_width as usize
                + (col_offset % indent_width as usize != 0) as usize
                + editor_config.indent_guides.skip_levels as usize,
            indent_guide_style: text_style.patch(
                theme
                    .try_get("ui.virtual.indent-guide")
                    .unwrap_or_else(|| theme.get("ui.virtual.whitespace")),
            ),
            text_style,
            draw_indent_guides: editor_config.indent_guides.render,
            viewport,
            col_offset,
        }
    }
    /// Draws a single `grapheme` at the current render position with a specified `style`.
    pub fn draw_decoration_grapheme(
        &mut self,
        grapheme: Grapheme,
        mut style: Style,
        row: u16,
        col: u16,
    ) -> bool {
        if row >= self.viewport.height || col >= self.viewport.width {
            return false;
        }
        let is_whitespace = grapheme.is_whitespace();

        // TODO is it correct to apply the whitspace style to all unicode white spaces?
        if is_whitespace {
            style = style.patch(self.whitespace_style);
        }

        let grapheme = match grapheme {
            Grapheme::Tab { width } => {
                let grapheme_tab_width = char_to_byte_idx(&self.virtual_tab, width);
                &self.virtual_tab[..grapheme_tab_width]
            }
            Grapheme::Other { ref g } if g == "\u{00A0}" => " ",
            Grapheme::Other { ref g } => g,
            Grapheme::Newline => " ",
        };

        self.surface.set_string(
            self.viewport.x + col,
            self.viewport.y + row as u16,
            grapheme,
            style,
        );
        true
    }

    /// Draws a single `grapheme` at the current render position with a specified `style`.
    pub fn draw_grapheme(
        &mut self,
        grapheme: Grapheme,
        mut style: Style,
        is_virtual: bool,
        last_indent_level: &mut usize,
        is_in_indent_area: &mut bool,
        position: Position,
    ) {
        let cut_off_start = self.col_offset.saturating_sub(position.col);
        let is_whitespace = grapheme.is_whitespace();

        // TODO is it correct to apply the whitespace style to all unicode white spaces?
        if is_whitespace {
            style = style.patch(self.whitespace_style);
        }

        let width = grapheme.width();
        let space = if is_virtual { " " } else { &self.space };
        let nbsp = if is_virtual { " " } else { &self.nbsp };
        let tab = if is_virtual {
            &self.virtual_tab
        } else {
            &self.tab
        };
        let grapheme = match grapheme {
            Grapheme::Tab { width } => {
                let grapheme_tab_width = char_to_byte_idx(tab, width);
                &tab[..grapheme_tab_width]
            }
            // TODO special rendering for other whitespaces?
            Grapheme::Other { ref g } if g == " " => space,
            Grapheme::Other { ref g } if g == "\u{00A0}" => nbsp,
            Grapheme::Other { ref g } => g,
            Grapheme::Newline => &self.newline,
        };

        let in_bounds = self.column_in_bounds(position.col);

        if in_bounds {
            self.surface.set_string(
                self.viewport.x + (position.col - self.col_offset) as u16,
                self.viewport.y + position.row as u16,
                grapheme,
                style,
            );
        } else if cut_off_start != 0 && cut_off_start < width {
            // partially on screen
            let rect = Rect::new(
                self.viewport.x,
                self.viewport.y + position.row as u16,
                (width - cut_off_start) as u16,
                1,
            );
            self.surface.set_style(rect, style);
        }

        if *is_in_indent_area && !is_whitespace {
            *last_indent_level = position.col;
            *is_in_indent_area = false;
        }
    }

    pub fn column_in_bounds(&self, colum: usize) -> bool {
        self.col_offset <= colum && colum < self.viewport.width as usize + self.col_offset
    }

    /// Overlay indentation guides ontop of a rendered line
    /// The indentation level is computed in `draw_lines`.
    /// Therefore this function must always be called afterwards.
    pub fn draw_indent_guides(&mut self, indent_level: usize, row: u16) {
        if !self.draw_indent_guides {
            return;
        }

        // Don't draw indent guides outside of view
        let end_indent = min(
            indent_level,
            // Add indent_width - 1 to round up, since the first visible
            // indent might be a bit after offset.col
            self.col_offset + self.viewport.width as usize + (self.indent_width as usize - 1),
        ) / self.indent_width as usize;

        for i in self.starting_indent..end_indent {
            let x = (self.viewport.x as usize + (i * self.indent_width as usize) - self.col_offset)
                as u16;
            let y = self.viewport.y + row;
            debug_assert!(self.surface.in_bounds(x, y));
            self.surface
                .set_string(x, y, &self.indent_guide_char, self.indent_guide_style);
        }
    }
}
