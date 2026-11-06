//! shadcn/ui-flavoured primitives built from plain GPUI divs.

use crate::theme::*;
use gpui::{
    Bounds, Div, ElementId, FontWeight, Hsla, PathBuilder, SharedString, Stateful, canvas, div, point, prelude::*,
    px, quad, size,
};

pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_xl()
        .border_1()
        .border_color(border())
        .bg(crate::theme::card())
        .shadow_sm()
}

pub fn separator() -> Div {
    div().h(px(1.)).w_full().flex_shrink_0().bg(border())
}

pub fn vseparator() -> Div {
    div().w(px(1.)).h(px(16.)).flex_shrink_0().bg(border())
}

#[derive(Clone, Copy, PartialEq)]
pub enum Variant {
    Default,
    Outline,
    Ghost,
}

pub fn button(id: impl Into<ElementId>, variant: Variant) -> Stateful<Div> {
    let base = div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(30.))
        .px_3()
        .rounded_md()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .whitespace_nowrap();
    match variant {
        Variant::Default => base
            .bg(primary())
            .text_color(primary_foreground())
            .shadow_sm()
            .hover(|s| s.bg(alpha(primary(), 0.88))),
        Variant::Outline => base
            .border_1()
            .border_color(input())
            .bg(alpha(input(), 0.3))
            .text_color(foreground())
            .shadow_sm()
            .hover(|s| s.bg(alpha(input(), 0.6))),
        Variant::Ghost => base.text_color(muted_foreground()).hover(|s| s.bg(alpha(muted(), 0.6)).text_color(foreground())),
    }
}

pub fn badge(label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .h(px(22.))
        .rounded_md()
        .border_1()
        .border_color(border())
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground())
        .whitespace_nowrap()
        .child(label.into())
}

pub fn dot(color: Hsla) -> Div {
    div().size(px(6.)).flex_shrink_0().rounded_full().bg(color)
}

pub fn kbd(label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(20.))
        .min_w(px(20.))
        .px(px(5.))
        .rounded(px(4.))
        .border_1()
        .border_color(border())
        .bg(alpha(muted(), 0.5))
        .text_xs()
        .font_family(MONO)
        .text_color(muted_foreground())
        .child(label.into())
}

pub fn tabs_list() -> Div {
    div()
        .flex()
        .items_center()
        .h(px(34.))
        .p(px(3.))
        .gap(px(2.))
        .rounded_lg()
        .bg(muted())
}

pub fn tab_trigger(id: impl Into<ElementId>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .h_full()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px_3()
        .rounded_md()
        .border_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .whitespace_nowrap()
        .when(active, |d| d.bg(background()).border_color(input()).text_color(foreground()).shadow_sm())
        .when(!active, |d| {
            d.border_color(gpui::transparent_black())
                .text_color(muted_foreground())
                .hover(|s| s.text_color(foreground()))
        })
}

// --------------------------------------------------------------- icons ----

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    ChevronRight,
    ChevronDown,
    Search,
    File,
    Folder,
    Expand,
    Collapse,
    Close,
}

pub fn icon(kind: Icon, color: Hsla, size_px: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let o = b.origin;
            let s = f32::from(b.size.width);
            let p = |x: f32, y: f32| point(o.x + px(x * s), o.y + px(y * s));
            let stroke = |window: &mut gpui::Window, pts: &[gpui::Point<gpui::Pixels>], w: f32, closed: bool| {
                let mut pb = PathBuilder::stroke(px(w));
                pb.add_polygon(pts, closed);
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            };
            match kind {
                Icon::ChevronRight => stroke(window, &[p(0.38, 0.22), p(0.66, 0.5), p(0.38, 0.78)], 1.5, false),
                Icon::ChevronDown => stroke(window, &[p(0.22, 0.38), p(0.5, 0.66), p(0.78, 0.38)], 1.5, false),
                Icon::Search => {
                    let c = p(0.44, 0.44);
                    let r = px(0.28 * s);
                    let mut pb = PathBuilder::stroke(px(1.5));
                    pb.move_to(point(c.x - r, c.y));
                    pb.arc_to(point(r, r), px(0.), false, true, point(c.x + r, c.y));
                    pb.arc_to(point(r, r), px(0.), false, true, point(c.x - r, c.y));
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, color);
                    }
                    stroke(window, &[p(0.66, 0.66), p(0.88, 0.88)], 1.5, false);
                }
                Icon::File => {
                    stroke(window, &[p(0.24, 0.1), p(0.6, 0.1), p(0.8, 0.3), p(0.8, 0.9), p(0.24, 0.9)], 1.3, true);
                    stroke(window, &[p(0.36, 0.52), p(0.68, 0.52)], 1.2, false);
                    stroke(window, &[p(0.36, 0.7), p(0.62, 0.7)], 1.2, false);
                }
                Icon::Folder => {
                    stroke(
                        window,
                        &[p(0.1, 0.22), p(0.4, 0.22), p(0.5, 0.34), p(0.9, 0.34), p(0.9, 0.82), p(0.1, 0.82)],
                        1.3,
                        true,
                    );
                }
                Icon::Expand => {
                    stroke(window, &[p(0.3, 0.38), p(0.5, 0.18), p(0.7, 0.38)], 1.4, false);
                    stroke(window, &[p(0.3, 0.62), p(0.5, 0.82), p(0.7, 0.62)], 1.4, false);
                }
                Icon::Collapse => {
                    stroke(window, &[p(0.3, 0.18), p(0.5, 0.38), p(0.7, 0.18)], 1.4, false);
                    stroke(window, &[p(0.3, 0.82), p(0.5, 0.62), p(0.7, 0.82)], 1.4, false);
                }
                Icon::Close => {
                    stroke(window, &[p(0.28, 0.28), p(0.72, 0.72)], 1.5, false);
                    stroke(window, &[p(0.72, 0.28), p(0.28, 0.72)], 1.5, false);
                }
            }
        },
    )
    .size(px(size_px))
    .flex_shrink_0()
}

/// A determinate progress bar.
pub fn progress(frac: f32) -> Div {
    div()
        .h(px(8.))
        .w_full()
        .rounded_full()
        .bg(muted())
        .overflow_hidden()
        .child(div().h_full().w(gpui::relative(frac.clamp(0.0, 1.0))).rounded_full().bg(primary()))
}

#[allow(unused)]
pub fn square(color: Hsla, s: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            window.paint_quad(quad(
                Bounds::new(b.origin, size(px(s), px(s))),
                px(2.),
                color,
                px(0.),
                gpui::transparent_black(),
                Default::default(),
            ));
        },
    )
    .size(px(s))
}
