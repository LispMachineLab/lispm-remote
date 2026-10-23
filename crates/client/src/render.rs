// lispm-remote: a screen, drawn on a canvas.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Drawing a screen with the client's own fonts and images, in four
//! passes: backgrounds, images under the text (negative layers, as Kitty
//! draws them over backgrounds but under text), text and its lines, then
//! images over the text and the cursor.

use std::collections::HashMap;

use lispm_remote_display::Attrs;
use lispm_remote_display::Cell;
use lispm_remote_display::Color;
use lispm_remote_display::CursorShape;
use lispm_remote_display::Placement;
use lispm_remote_display::ResourceId;
use lispm_remote_display::Rgb;
use lispm_remote_display::Screen;
use lispm_remote_display::Style;
use lispm_remote_display::Underline;
use web_sys::CanvasRenderingContext2d;
use web_sys::ImageBitmap;

/// The fonts cells are drawn in, the first one present winning.
const FAMILY: &str = "\"SF Mono\", Menlo, Monaco, Consolas, \"DejaVu Sans Mono\", \
                      \"Noto Sans Mono CJK SC\", \"PingFang SC\", monospace";

/// A cell's size and the font, in CSS pixels.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    /// The font size.
    pub size: f64,
    /// A cell's width.
    pub cell_width: f64,
    /// A cell's height.
    pub cell_height: f64,
    /// Device pixels per CSS pixel.
    pub ratio: f64,
}

/// The colours `Color::Default` stands for.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Text.
    pub foreground: Rgb,
    /// Background.
    pub background: Rgb,
}

/// The CSS font for a cell's attributes.
#[must_use]
pub fn font(size: f64, bold: bool, italic: bool) -> String {
    format!(
        "{}{}{size}px {FAMILY}",
        if italic { "italic " } else { "" },
        if bold { "bold " } else { "" }
    )
}

fn css(rgb: Rgb) -> String {
    format!("rgb({},{},{})", rgb.r, rgb.g, rgb.b)
}

/// The text and background colours of `style`.
fn colors(style: &Style, theme: Theme) -> (Rgb, Rgb) {
    let fg = match style.fg {
        Color::Default => theme.foreground,
        Color::Rgb(rgb) => rgb,
    };
    let bg = match style.bg {
        Color::Default => theme.background,
        Color::Rgb(rgb) => rgb,
    };
    let (fg, bg) = if style.attrs.contains(Attrs::INVERSE) {
        (bg, fg)
    } else {
        (fg, bg)
    };
    if style.attrs.contains(Attrs::INVISIBLE) {
        (bg, bg)
    } else {
        (fg, bg)
    }
}

/// Draws `screen` on `context`, which is `width` × `height` CSS pixels.
pub fn draw(
    context: &CanvasRenderingContext2d,
    metrics: &Metrics,
    screen: &Screen,
    images: &HashMap<ResourceId, ImageBitmap>,
    theme: Theme,
    focused: bool,
) {
    let canvas = context.canvas();
    let (width, height) = canvas.map_or((0.0, 0.0), |canvas| {
        (f64::from(canvas.width()), f64::from(canvas.height()))
    });
    context.set_fill_style_str(&css(theme.background));
    context.fill_rect(0.0, 0.0, width / metrics.ratio, height / metrics.ratio);
    for (row, line) in (0_u16..).zip(screen.rows()) {
        backgrounds(context, metrics, row, line.cells(), theme);
    }
    let mut placements: Vec<&Placement> = screen.placements.iter().collect();
    placements.sort_by_key(|placement| placement.z);
    let (under, over): (Vec<&Placement>, Vec<&Placement>) = placements
        .into_iter()
        .partition(|placement| placement.z < 0);
    for placement in under {
        image(context, metrics, placement, images);
    }
    context.set_text_baseline("middle");
    let mut font_now = String::new();
    for (row, line) in (0_u16..).zip(screen.rows()) {
        text(context, metrics, row, line.cells(), theme, &mut font_now);
    }
    for placement in over {
        image(context, metrics, placement, images);
    }
    cursor(context, metrics, screen, theme, focused);
}

fn backgrounds(
    context: &CanvasRenderingContext2d,
    metrics: &Metrics,
    row: u16,
    cells: &[Cell],
    theme: Theme,
) {
    let y = f64::from(row) * metrics.cell_height;
    let mut col = 0_u16;
    for run in cells.chunk_by(|a, b| colors(&a.style, theme).1 == colors(&b.style, theme).1) {
        let len = u16::try_from(run.len()).unwrap_or(u16::MAX);
        if let Some(first) = run.first() {
            let bg = colors(&first.style, theme).1;
            if bg != theme.background {
                context.set_fill_style_str(&css(bg));
                context.fill_rect(
                    f64::from(col) * metrics.cell_width,
                    y,
                    f64::from(len) * metrics.cell_width,
                    metrics.cell_height,
                );
            }
        }
        col = col.saturating_add(len);
    }
}

fn text(
    context: &CanvasRenderingContext2d,
    metrics: &Metrics,
    row: u16,
    cells: &[Cell],
    theme: Theme,
    font_now: &mut String,
) {
    let top = f64::from(row) * metrics.cell_height;
    let middle = top + metrics.cell_height / 2.0;
    for (col, cell) in (0_u16..).zip(cells) {
        let text = cell.content.text();
        let x = f64::from(col) * metrics.cell_width;
        let (fg, _) = colors(&cell.style, theme);
        let attrs = cell.style.attrs;
        if !text.trim().is_empty() {
            let font = font(
                metrics.size,
                attrs.contains(Attrs::BOLD),
                attrs.contains(Attrs::ITALIC),
            );
            if *font_now != font {
                context.set_font(&font);
                *font_now = font;
            }
            context.set_fill_style_str(&css(fg));
            context.set_global_alpha(if attrs.contains(Attrs::DIM) { 0.6 } else { 1.0 });
            let _ = context.fill_text(&text, x, middle);
            context.set_global_alpha(1.0);
        }
        lines(context, metrics, cell, x, top, fg);
    }
}

/// The underline, strike-through and overline of a cell.
fn lines(
    context: &CanvasRenderingContext2d,
    metrics: &Metrics,
    cell: &Cell,
    x: f64,
    top: f64,
    fg: Rgb,
) {
    let style = &cell.style;
    let width = metrics.cell_width;
    let thin = (metrics.size / 14.0).max(1.0);
    if style.underline != Underline::None {
        let color = match style.underline_color {
            Color::Default => fg,
            Color::Rgb(rgb) => rgb,
        };
        context.set_fill_style_str(&css(color));
        let y = top + metrics.cell_height - 2.0 * thin;
        match style.underline {
            Underline::Double => {
                context.fill_rect(x, y, width, thin);
                context.fill_rect(x, y - 2.0 * thin, width, thin);
            }
            Underline::Curly => {
                let step = width / 4.0;
                for i in 0..4 {
                    let lift = if i % 2 == 0 { 0.0 } else { thin };
                    context.fill_rect(x + f64::from(i) * step, y - lift, step, thin);
                }
            }
            Underline::Dotted | Underline::Dashed => {
                context.fill_rect(x, y, width / 2.0, thin);
            }
            Underline::Single | Underline::None => context.fill_rect(x, y, width, thin),
        }
    }
    context.set_fill_style_str(&css(fg));
    if style.attrs.contains(Attrs::STRIKE) {
        context.fill_rect(x, top + metrics.cell_height / 2.0, width, thin);
    }
    if style.attrs.contains(Attrs::OVERLINE) {
        context.fill_rect(x, top, width, thin);
    }
}

fn image(
    context: &CanvasRenderingContext2d,
    metrics: &Metrics,
    placement: &Placement,
    images: &HashMap<ResourceId, ImageBitmap>,
) {
    let Some(bitmap) = images.get(&placement.resource) else {
        return;
    };
    let cells = placement.cells;
    let x =
        f64::from(cells.col) * metrics.cell_width + f64::from(placement.offset_x) / metrics.ratio;
    let y =
        f64::from(cells.row) * metrics.cell_height + f64::from(placement.offset_y) / metrics.ratio;
    let w = f64::from(cells.cols) * metrics.cell_width;
    let h = f64::from(cells.rows) * metrics.cell_height;
    let (sx, sy, sw, sh) = placement.crop.map_or(
        (
            0.0,
            0.0,
            f64::from(bitmap.width()),
            f64::from(bitmap.height()),
        ),
        |crop| {
            (
                f64::from(crop.x),
                f64::from(crop.y),
                f64::from(crop.w),
                f64::from(crop.h),
            )
        },
    );
    let _ = context.draw_image_with_image_bitmap_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
        bitmap, sx, sy, sw, sh, x, y, w, h,
    );
}

fn cursor(
    context: &CanvasRenderingContext2d,
    metrics: &Metrics,
    screen: &Screen,
    theme: Theme,
    focused: bool,
) {
    let cursor = screen.cursor;
    if !cursor.visible {
        return;
    }
    let x = f64::from(cursor.col) * metrics.cell_width;
    let y = f64::from(cursor.row) * metrics.cell_height;
    let cell = screen
        .row(cursor.row)
        .and_then(|row| row.cells().get(usize::from(cursor.col)));
    let (fg, bg) = cell.map_or((theme.foreground, theme.background), |cell| {
        colors(&cell.style, theme)
    });
    context.set_fill_style_str(&css(fg));
    let thin = (metrics.size / 7.0).max(2.0);
    match (cursor.shape, focused) {
        (CursorShape::Block, true) => {
            context.fill_rect(x, y, metrics.cell_width, metrics.cell_height);
            if let Some(cell) = cell {
                context.set_fill_style_str(&css(bg));
                let _ = context.fill_text(&cell.content.text(), x, y + metrics.cell_height / 2.0);
            }
        }
        (CursorShape::Block, false) => {
            context.set_stroke_style_str(&css(fg));
            context.stroke_rect(
                x + 0.5,
                y + 0.5,
                metrics.cell_width - 1.0,
                metrics.cell_height - 1.0,
            );
        }
        (CursorShape::Bar, _) => context.fill_rect(x, y, thin, metrics.cell_height),
        (CursorShape::Underline, _) => {
            context.fill_rect(x, y + metrics.cell_height - thin, metrics.cell_width, thin)
        }
    }
}
