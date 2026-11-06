//! Renders the HTML block model with GPUI elements, styled like shadcn "prose".

use crate::html::{Align, BOLD, Block, CODE, ITALIC, Inline, LINK, MARK, STRIKE, UNDERLINE};
use crate::theme::*;
use gpui::{
    AnyElement, Div, FontStyle, FontWeight, Hsla, StrikethroughStyle, StyledText, TextRun, UnderlineStyle, div,
    font, prelude::*, px, relative, rems,
};

const UI_FONT: &str = ".SystemUIFont";

pub fn render_blocks(blocks: &[Block]) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .min_w(px(0.))
        .children(blocks.iter().map(|b| render_block(b, foreground())))
}

fn styled(inl: &Inline, base: Hsla) -> AnyElement {
    let text = inl.text.clone();
    if text.is_empty() {
        return div().into_any_element();
    }
    let mut runs = Vec::with_capacity(inl.spans.len() + 2);
    let mut at = 0;
    let plain = |len: usize| TextRun {
        len,
        font: font(UI_FONT),
        color: base,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    for (r, flags) in &inl.spans {
        if r.start > at {
            runs.push(plain(r.start - at));
        }
        if r.start < at {
            continue;
        }
        let mut f = font(if flags & CODE != 0 { MONO } else { UI_FONT });
        if flags & BOLD != 0 {
            f.weight = FontWeight::SEMIBOLD;
        }
        if flags & ITALIC != 0 {
            f.style = FontStyle::Italic;
        }
        let color = if flags & LINK != 0 { hsl(213.0, 94.0, 68.0) } else { base };
        let background = if flags & CODE != 0 {
            Some(alpha(muted(), 0.9))
        } else if flags & MARK != 0 {
            Some(alpha(chart(2), 0.3))
        } else {
            None
        };
        runs.push(TextRun {
            len: r.end - r.start,
            font: f,
            color,
            background_color: background,
            underline: (flags & (LINK | UNDERLINE) != 0).then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(alpha(color, 0.6)),
                wavy: false,
            }),
            strikethrough: (flags & STRIKE != 0).then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        });
        at = r.end;
    }
    if at < text.len() {
        runs.push(plain(text.len() - at));
    }
    StyledText::new(text).with_runs(runs).into_any_element()
}

fn render_block(block: &Block, base: Hsla) -> AnyElement {
    match block {
        Block::Para(inl) => div()
            .text_sm()
            .line_height(rems(1.45))
            .text_color(base)
            .child(styled(inl, base))
            .into_any_element(),
        Block::Heading(level, inl) => {
            let d = div().mt_1().font_weight(FontWeight::SEMIBOLD).text_color(foreground());
            let d = match level {
                1 => d.text_xl(),
                2 => d.text_lg(),
                3 => d.text_base(),
                _ => d.text_sm(),
            };
            d.child(styled(inl, foreground())).into_any_element()
        }
        Block::List { ordered, start, items } => div()
            .flex()
            .flex_col()
            .gap_1()
            .pl_1()
            .children(items.iter().enumerate().map(|(i, item)| {
                let marker = if *ordered { format!("{}.", *start as usize + i) } else { "•".to_string() };
                div()
                    .flex()
                    .gap_2()
                    .text_sm()
                    .child(
                        div()
                            .w(px(22.))
                            .flex_shrink_0()
                            .text_color(muted_foreground())
                            .when(*ordered, |d| d.font_family(MONO).text_xs().pt(px(2.)))
                            .child(marker),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .min_w(px(0.))
                            .children(item.iter().map(|b| render_block(b, base))),
                    )
            }))
            .into_any_element(),
        Block::Table { caption, rows, columns } => {
            let cols = *columns as f32;
            let n_rows = rows.len();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .when_some(caption.as_ref(), |d, c| {
                    d.child(div().text_xs().text_color(muted_foreground()).child(styled(c, muted_foreground())))
                })
                .child(
                    div()
                        .w_full()
                        .rounded_md()
                        .border_1()
                        .border_color(border())
                        .overflow_hidden()
                        .children(rows.iter().enumerate().map(|(ri, row)| {
                            let n_cells = row.cells.len();
                            div()
                                .flex()
                                .w_full()
                                .when(ri + 1 < n_rows, |d| d.border_b_1().border_color(border()))
                                .when(row.header, |d| d.bg(alpha(muted(), 0.5)))
                                .children(row.cells.iter().enumerate().map(|(ci, cell)| {
                                    let color = if cell.header { muted_foreground() } else { base };
                                    div()
                                        .w(relative(cell.colspan as f32 / cols))
                                        .flex_shrink_0()
                                        .min_w(px(0.))
                                        .px_3()
                                        .py(px(7.))
                                        .text_sm()
                                        .when(cell.header, |d| d.font_weight(FontWeight::MEDIUM).text_xs())
                                        .when(ci + 1 < n_cells, |d| d.border_r_1().border_color(border()))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .when(cell.align == Align::Right, |d| d.items_end())
                                        .when(cell.align == Align::Center, |d| d.items_center())
                                        .children(cell.content.iter().map(|b| render_block(b, color)))
                                }))
                        }))
                )
                .into_any_element()
        }
        Block::Quote(blocks) => div()
            .border_l_2()
            .border_color(border())
            .pl_4()
            .flex()
            .flex_col()
            .gap_2()
            .children(blocks.iter().map(|b| render_block(b, muted_foreground())))
            .into_any_element(),
        Block::Code(text) => div()
            .rounded_md()
            .border_1()
            .border_color(border())
            .bg(alpha(muted(), 0.35))
            .px_4()
            .py_3()
            .font_family(MONO)
            .text_xs()
            .line_height(rems(1.35))
            .text_color(foreground())
            .child(text.clone())
            .into_any_element(),
        Block::Rule => div().h(px(1.)).w_full().bg(border()).into_any_element(),
        Block::Image(alt) => div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(48.))
            .rounded_md()
            .border_1()
            .border_color(border())
            .text_xs()
            .text_color(muted_foreground())
            .child(format!("image: {alt}"))
            .into_any_element(),
    }
}
