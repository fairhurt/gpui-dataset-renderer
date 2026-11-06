//! The Requirements Explorer view.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, BoxShadow, ClipboardItem, Context, Div, Entity, FocusHandle, FontWeight, KeyDownEvent, SharedString,
    Subscription, Window, div, list, point, prelude::*, px, uniform_list,
};
use gpui_component::{
    IconName, WindowExt as _, h_resizable,
    menu::{ContextMenuExt as _, PopupMenuItem},
    notification::Notification,
    resizable_panel,
};

use crate::data::{self, Dataset, PRIORITIES, Progress, STATUSES, Source, synth::Synth};
use crate::editor::{EditorEvent, Pane, XmlEditor};
use crate::explorer::Explorer;
use crate::html_render::render_blocks;
use crate::theme::*;
use crate::ui::{self, Icon, Variant};

const TREE_ROW_H: f32 = 26.0;
const ROW_H: f32 = 38.0;

// Column widths (table header and rows share these).
const W_CHEVRON: f32 = 28.0;
const W_ID: f32 = 92.0;
const W_SECTION: f32 = 112.0;
const W_TYPE: f32 = 104.0;
const W_STATUS: f32 = 116.0;
const W_PRIORITY: f32 = 84.0;
const W_CHILDREN: f32 = 72.0;
const W_OWNER: f32 = 116.0;
const W_UPDATED: f32 = 96.0;

pub struct Options {
    pub count: u32,
    pub dir: Option<PathBuf>,
    /// Requirement ID to open in the editor once the dataset is loaded.
    pub edit: Option<String>,
}

type LoadSlot = Arc<Mutex<Option<Result<Dataset, String>>>>;

struct Loading {
    progress: Arc<Progress>,
    slot: LoadSlot,
    started: Instant,
    label: String,
}

pub struct ReqApp {
    focus: FocusHandle,
    search_focus: FocusHandle,
    loading: Option<Loading>,
    error: Option<String>,
    ex: Option<Explorer>,
    total_load: f32,
    editor: Option<Entity<XmlEditor>>,
    _editor_sub: Option<Subscription>,
    pending_edit: Option<String>,
}

impl ReqApp {
    pub fn new(opts: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let opts_edit = opts.edit.clone();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        let progress = Arc::new(Progress::default());
        let slot: LoadSlot = Arc::new(Mutex::new(None));
        let label = match &opts.dir {
            Some(d) => format!("Reading XML files from {}", d.display()),
            None => format!("Generating & parsing {} synthetic XML documents", ui_int(opts.count as usize)),
        };
        {
            let progress = progress.clone();
            let slot = slot.clone();
            std::thread::spawn(move || {
                progress.phase.store(data::PHASE_SHAPE, Ordering::Relaxed);
                let source = match opts.dir {
                    Some(d) => Source::open_dir(d),
                    None => Ok(Source::Synthetic(Arc::new(Synth::new(opts.count, 7)))),
                };
                let result = source.map(|s| data::load(s, progress));
                *slot.lock().unwrap() = Some(result);
            });
        }

        Self {
            focus,
            search_focus: cx.focus_handle(),
            loading: Some(Loading { progress, slot, started: Instant::now(), label }),
            error: None,
            ex: None,
            total_load: 0.0,
            editor: None,
            _editor_sub: None,
            pending_edit: opts_edit,
        }
    }

    fn poll_loading(&mut self) {
        let Some(l) = &self.loading else { return };
        let res = l.slot.lock().unwrap().take();
        if let Some(res) = res {
            self.total_load = l.started.elapsed().as_secs_f32();
            self.loading = None;
            match res {
                Ok(ds) => self.ex = Some(Explorer::new(ds)),
                Err(e) => self.error = Some(e),
            }
        }
    }

    // ------------------------------------------------------------ actions

    // ------------------------------------------------------------- editor

    /// Opens the XML editor on `node`, asking first if another document has unsaved edits.
    pub fn open_editor(&mut self, node: u32, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = &self.editor {
            let (same, dirty, cur) = {
                let e = ed.read(cx);
                (e.node == node, e.is_dirty(), e.id.clone())
            };
            if same {
                ed.update(cx, |e, cx| e.set_pane(pane, window, cx));
                return;
            }
            if dirty {
                let weak = cx.entity().downgrade();
                let next = self.ex.as_ref().map(|ex| ex.req(node).id.clone()).unwrap_or_default();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let weak = weak.clone();
                    alert
                        .title("Discard unsaved changes?")
                        .description(format!("{cur} has unsaved edits. Opening {next} discards them."))
                        .show_cancel(true)
                        .ok_text("Discard and open")
                        .on_ok(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.force_open_editor(node, pane, window, cx));
                            true
                        })
                });
                return;
            }
        }
        self.force_open_editor(node, pane, window, cx);
    }

    fn force_open_editor(&mut self, node: u32, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ex) = self.ex.as_mut() else { return };
        let docs = ex.original_xml(node).and_then(|o| ex.current_xml(node).map(|c| (o, c)));
        let (original, current) = match docs {
            Ok(d) => d,
            Err(e) => {
                window.push_notification(Notification::error(format!("Cannot open: {e}")), cx);
                return;
            }
        };
        let id = ex.req(node).id.clone();
        let ds = ex.ds.clone();
        ex.selected = Some(node);
        ex.reveal_in_tree(node);
        if let Some(ix) = ex.row_of(node) {
            ex.list.scroll_to_reveal_item(ix);
        }
        let ed = cx.new(|cx| XmlEditor::new(node, id, original, current, ds, pane, window, cx));
        self._editor_sub = Some(cx.subscribe_in(&ed, window, Self::on_editor_event));
        ed.update(cx, |e, cx| e.focus_editor(cx));
        self.editor = Some(ed);
        cx.notify();
    }

    fn on_editor_event(&mut self, ed: &Entity<XmlEditor>, ev: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            EditorEvent::Save { node, xml } => {
                let Some(ex) = self.ex.as_mut() else { return };
                match ex.save_edit(*node, xml.clone()) {
                    Ok(()) => {
                        let id = ex.req(*node).id.clone();
                        ed.update(cx, |e, cx| e.mark_saved(xml.clone(), cx));
                        window.push_notification(Notification::success(format!("Saved {id} (in memory).")), cx);
                    }
                    Err(e) => window.push_notification(Notification::error(format!("Not saved: {e}")), cx),
                }
            }
            EditorEvent::RevertToOriginal { node } => self.revert_node(*node, window, cx),
            EditorEvent::Close => {
                self.editor = None;
                self._editor_sub = None;
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    fn revert_node(&mut self, node: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ex) = self.ex.as_mut() else { return };
        let was_edited = ex.is_edited(node);
        let original = ex.original_xml(node);
        ex.revert(node);
        ex.apply_filter();
        if let (Some(ed), Ok(original)) = (&self.editor, original) {
            if ed.read(cx).node == node {
                ed.update(cx, |e, cx| e.reload(original, window, cx));
            }
        }
        if was_edited {
            let id = ex.req(node).id.clone();
            window.push_notification(Notification::info(format!("Reverted {id} to the source document.")), cx);
        }
        cx.notify();
    }

    fn on_root_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Only when the table itself has focus: typing in the editor must not drive the table.
        if !self.focus.is_focused(window) {
            return;
        }
        let Some(ex) = self.ex.as_mut() else { return };
        let ks = &ev.keystroke;
        match ks.key.as_str() {
            "down" => ex.move_selection(1),
            "up" => ex.move_selection(-1),
            "pagedown" => ex.move_selection(20),
            "pageup" => ex.move_selection(-20),
            "enter" | "space" | "right" | "left" => {
                if let Some(s) = ex.selected {
                    let open = ex.expanded.contains(&s);
                    let want = match ks.key.as_str() {
                        "right" => true,
                        "left" => false,
                        _ => !open,
                    };
                    if want != open {
                        ex.toggle_row(s);
                    }
                    ex.reveal_in_tree(s);
                }
            }
            "escape" => ex.collapse_all_rows(),
            "/" => window.focus(&self.search_focus, cx),
            "f" if ks.modifiers.control || ks.modifiers.platform => window.focus(&self.search_focus, cx),
            _ => return,
        }
        cx.notify();
    }

    fn on_search_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ex) = self.ex.as_mut() else { return };
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "backspace" => {
                if m.control {
                    ex.query.clear();
                } else {
                    ex.query.pop();
                }
            }
            "escape" => {
                if ex.query.is_empty() {
                    window.focus(&self.focus, cx);
                } else {
                    ex.query.clear();
                }
            }
            "enter" | "down" => {
                if let Some(&first) = ex.rows.first() {
                    ex.show(first, true);
                    ex.reveal_in_tree(first);
                }
                window.focus(&self.focus, cx);
                cx.stop_propagation();
                cx.notify();
                return;
            }
            "tab" => return,
            key => {
                if m.control || m.alt || m.platform {
                    return;
                }
                if let Some(ch) = ks.key_char.as_ref().filter(|c| !c.chars().any(char::is_control)) {
                    ex.query.push_str(ch);
                } else if key == "space" {
                    ex.query.push(' ');
                } else if key.chars().count() == 1 {
                    ex.query.push_str(key);
                } else {
                    return;
                }
            }
        }
        ex.apply_filter();
        cx.stop_propagation();
        cx.notify();
    }

    // ------------------------------------------------------------- render

    fn render_loading(&self) -> AnyElement {
        let body = if let Some(l) = &self.loading {
            let p = &l.progress;
            let total = p.total.load(Ordering::Relaxed).max(1);
            let done = p.done.load(Ordering::Relaxed);
            let bytes = p.bytes.load(Ordering::Relaxed);
            let phase = p.phase.load(Ordering::Relaxed);
            let (title, frac) = match phase {
                data::PHASE_SHAPE => ("Preparing dataset…", 0.0),
                data::PHASE_PARSE => ("Parsing XML documents", done as f32 / total as f32),
                data::PHASE_LINK => ("Linking requirement tree", 1.0),
                _ => ("Finishing…", 1.0),
            };
            let secs = l.started.elapsed().as_secs_f32();
            ui::card()
                .w(px(460.))
                .p_6()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("Indexing requirements"))
                        .child(div().text_sm().text_color(muted_foreground()).child(l.label.clone())),
                )
                .child(ui::progress(frac))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_xs()
                        .font_family(MONO)
                        .text_color(muted_foreground())
                        .child(format!("{title} · {} / {}", ui_int(done), ui_int(total)))
                        .child(format!("{} · {secs:.1}s", ui_bytes(bytes))),
                )
        } else {
            ui::card().w(px(520.)).p_6().gap_3().child(
                div().text_base().font_weight(FontWeight::SEMIBOLD).child("Could not load requirements"),
            ).child(
                div()
                    .text_sm()
                    .text_color(destructive())
                    .child(self.error.clone().unwrap_or_default()),
            )
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(background())
            .text_color(foreground())
            .child(body)
            .into_any_element()
    }

    fn render_header(&self) -> Div {
        let ex = self.ex.as_ref().unwrap();
        let ds = &ex.ds;
        let crumbs: Vec<SharedString> = ex
            .selected
            .map(|s| {
                let mut v: Vec<SharedString> = ds.ancestors(s).iter().map(|a| ex.req(*a).id.clone()).collect();
                v.push(ex.req(s).id.clone());
                v
            })
            .unwrap_or_default();
        let n_crumbs = crumbs.len();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(px(52.))
            .px_4()
            .border_b_1()
            .border_color(border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_w(px(0.))
                    .child(
                        div()
                            .size(px(26.))
                            .rounded_md()
                            .bg(primary())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(ui::icon(Icon::File, primary_foreground(), 16.)),
                    )
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Requirements Explorer"))
                    .child(ui::vseparator())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_sm()
                            .text_color(muted_foreground())
                            .child("SRS")
                            .children(crumbs.into_iter().enumerate().map(|(i, c)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(ui::icon(Icon::ChevronRight, muted_foreground(), 12.))
                                    .child(div().font_family(MONO).text_xs().when(i + 1 == n_crumbs, |d| d.text_color(foreground())).child(c))
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!ex.edits.is_empty(), |d| {
                        d.child(
                            ui::badge(format!("{} edited in memory", ex.edits.len()))
                                .text_color(chart(3))
                                .child(ui::dot(chart(3))),
                        )
                    })
                    .child(ui::badge(format!("{} requirements", ui_int(ds.len()))).font_family(MONO))
                    .child(ui::badge(format!("ready in {:.2}s", self.total_load)).font_family(MONO).child(ui::dot(chart(1))))
                    .child(ui::badge(RENDERER).bg(muted()).border_color(gpui::transparent_black())),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let ex = self.ex.as_ref().unwrap();
        let count = ex.tree_rows.len();
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(360.))
            .h_full()
            .border_r_1()
            .border_color(border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(48.))
                    .px_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Outline"))
                            .child(div().text_xs().font_family(MONO).text_color(muted_foreground()).child(format!("{} shown", ui_int(count)))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                ui::button("expand-all", Variant::Ghost)
                                    .px_2()
                                    .child(ui::icon(Icon::Expand, muted_foreground(), 14.))
                                    .child(div().text_xs().child("Expand"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(ex) = this.ex.as_mut() {
                                            ex.set_all_open(true);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                ui::button("collapse-all", Variant::Ghost)
                                    .px_2()
                                    .child(ui::icon(Icon::Collapse, muted_foreground(), 14.))
                                    .child(div().text_xs().child("Collapse"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(ex) = this.ex.as_mut() {
                                            ex.set_all_open(false);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(ui::separator())
            .child(
                uniform_list(
                    "tree",
                    count,
                    cx.processor(|this, range: Range<usize>, _window, cx| this.render_tree_rows(range, cx)),
                )
                .track_scroll(&ex.tree_scroll)
                .flex_1()
                .px_2()
                .py_1(),
            )
    }

    fn render_tree_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(ex) = self.ex.as_ref() else { return vec![] };
        let ds = ex.ds.clone();
        range
            .filter_map(|ix| ex.tree_rows.get(ix).copied())
            .map(|node| {
                let r = ex.req(node);
                let edited = ex.is_edited(node);
                let depth = ds.depth[node as usize] as f32;
                let has_kids = ds.has_children(node);
                let open = ex.open[node as usize];
                let selected = ex.selected == Some(node);
                div()
                    .id(("tree", node as usize))
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(TREE_ROW_H))
                    .pl(px(4. + depth * 14.))
                    .pr_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .when(selected, |d| d.bg(muted()).text_color(foreground()))
                    .when(!selected, |d| d.text_color(alpha(foreground(), 0.85)).hover(|s| s.bg(alpha(muted(), 0.5))))
                    .child(
                        div()
                            .id(("tree-chevron", node as usize))
                            .size(px(18.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .when(has_kids, |d| {
                                d.hover(|s| s.bg(alpha(muted(), 0.9)))
                                    .child(ui::icon(
                                        if open { Icon::ChevronDown } else { Icon::ChevronRight },
                                        muted_foreground(),
                                        14.,
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(ex) = this.ex.as_mut() {
                                            ex.toggle_open(node);
                                        }
                                        cx.stop_propagation();
                                        cx.notify();
                                    }))
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(MONO)
                            .text_xs()
                            .text_color(if edited { chart(3) } else if selected { foreground() } else { muted_foreground() })
                            .child(r.id.clone()),
                    )
                    .child(div().flex_1().min_w(px(0.)).truncate().child(r.title.clone()))
                    .when(has_kids, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .font_family(MONO)
                                .text_color(alpha(muted_foreground(), 0.7))
                                .child(ds.children(node).len().to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(ex) = this.ex.as_mut() {
                            if ds_has_children(ex, node) {
                                if ex.selected == Some(node) {
                                    ex.toggle_open(node);
                                } else if !ex.open[node as usize] {
                                    ex.toggle_open(node);
                                }
                            }
                            ex.show(node, true);
                        }
                        window.focus(&this.focus, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let ex = self.ex.as_ref().unwrap();
        let focused = self.search_focus.is_focused(window);
        let query = ex.query.clone();
        let mut counts = [0usize; 6];
        // Cheap enough at 150k (a single byte scan) and keeps the tab counts live.
        for r in &ex.ds.reqs {
            if (r.status as usize) < counts.len() {
                counts[r.status as usize] += 1;
            }
        }
        for (&n, e) in &ex.edits {
            let before = ex.ds.reqs[n as usize].status as usize;
            if before < counts.len() {
                counts[before] -= 1;
            }
            if (e.req.status as usize) < counts.len() {
                counts[e.req.status as usize] += 1;
            }
        }
        let n_edits = ex.edits.len();
        let edited_only = ex.edited_only;

        let search = div()
            .id("search")
            .track_focus(&self.search_focus)
            .flex()
            .items_center()
            .gap_2()
            .h(px(34.))
            .w(px(320.))
            .flex_shrink_0()
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(if focused { ring() } else { input() })
            .bg(alpha(input(), 0.3))
            .shadow(if focused {
                vec![BoxShadow {
                    color: alpha(ring(), 0.25),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }]
            } else {
                vec![]
            })
            .cursor_text()
            .text_sm()
            .child(ui::icon(Icon::Search, muted_foreground(), 14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .when(query.is_empty(), |d| {
                        d.child(div().text_color(muted_foreground()).child("Search ID or title…"))
                    })
                    .when(!query.is_empty(), |d| d.child(div().whitespace_nowrap().child(query.clone())))
                    .when(focused, |d| d.child(div().w(px(1.)).h(px(16.)).bg(foreground()))),
            )
            .when(!query.is_empty(), |d| {
                d.child(
                    div()
                        .id("search-clear")
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(muted()))
                        .child(ui::icon(Icon::Close, muted_foreground(), 14.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(ex) = this.ex.as_mut() {
                                ex.query.clear();
                                ex.apply_filter();
                            }
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
            })
            .when(!focused && query.is_empty(), |d| d.child(ui::kbd("/")))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.search_focus, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::on_search_key));

        let status_filter = ex.status_filter;
        let mut tabs = ui::tabs_list().child(
            ui::tab_trigger("status-all", status_filter.is_none() && !edited_only)
                .child("All")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(ex) = this.ex.as_mut() {
                        ex.status_filter = None;
                        ex.edited_only = false;
                        ex.apply_filter();
                    }
                    cx.notify();
                })),
        );
        for (i, name) in STATUSES.iter().enumerate() {
            let active = status_filter == Some(i as u8);
            tabs = tabs.child(
                ui::tab_trigger(("status", i), active)
                    .child(ui::dot(status_color(i as u8)))
                    .child(*name)
                    .child(
                        div()
                            .font_family(MONO)
                            .text_color(muted_foreground())
                            .child(compact(counts[i])),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(ex) = this.ex.as_mut() {
                            ex.status_filter = if ex.status_filter == Some(i as u8) { None } else { Some(i as u8) };
                            ex.apply_filter();
                        }
                        cx.notify();
                    })),
            );
        }

        tabs = tabs.child(
            ui::tab_trigger("status-edited", edited_only)
                .child(ui::dot(chart(3)))
                .child("Edited")
                .child(div().font_family(MONO).text_color(muted_foreground()).child(n_edits.to_string()))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(ex) = this.ex.as_mut() {
                        ex.edited_only = !ex.edited_only;
                        ex.apply_filter();
                    }
                    cx.notify();
                })),
        );

        div()
            .flex()
            .flex_shrink_0()
            .flex_wrap()
            .items_center()
            .gap_3()
            .child(search)
            .child(tabs)
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .font_family(MONO)
                    .text_color(muted_foreground())
                    .whitespace_nowrap()
                    .child(format!(
                        "{} of {} · {:.1} ms",
                        ui_int(ex.rows.len()),
                        ui_int(ex.ds.len()),
                        ex.filter_time.as_secs_f64() * 1000.0
                    )),
            )
    }

    fn render_table(&self, cx: &mut Context<Self>) -> Div {
        let compact = self.editor.is_some();
        let ex = self.ex.as_ref().unwrap();
        let empty = ex.rows.is_empty();
        let head = |w: Option<f32>, label: &'static str| {
            let d = div().flex_shrink_0().px_2().truncate().child(label);
            match w {
                Some(w) => d.w(px(w)),
                None => d.flex_1().min_w(px(0.)),
            }
        };
        ui::card()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .h(px(38.))
                    .px_2()
                    .border_b_1()
                    .border_color(border())
                    .bg(alpha(muted(), 0.3))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted_foreground())
                    .child(div().w(px(W_CHEVRON)).flex_shrink_0())
                    .child(head(Some(W_ID), "ID"))
                    .when(!compact, |d| d.child(head(Some(W_SECTION), "Section")))
                    .child(head(None, "Title"))
                    .child(head(Some(W_TYPE), "Type"))
                    .child(head(Some(W_STATUS), "Status"))
                    .child(head(Some(W_PRIORITY), "Priority"))
                    .child(head(Some(W_CHILDREN), "Children").text_right())
                    .when(!compact, |d| d.child(head(Some(W_OWNER), "Owner")).child(head(Some(W_UPDATED), "Updated"))),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(muted_foreground())
                        .child("No requirements match the current filters."),
                )
            })
            .when(!empty, |d| {
                d.child(
                    list(
                        ex.list.clone(),
                        cx.processor(|this, ix: usize, _window, cx| this.render_row(ix, cx)),
                    )
                    .flex_1()
                    .w_full(),
                )
            })
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let compact = self.editor.is_some();
        let Some(ex) = self.ex.as_mut() else { return div().into_any_element() };
        let Some(&node) = ex.rows.get(ix) else { return div().into_any_element() };
        let ds = ex.ds.clone();
        let r = ex.req(node).clone();
        let r = &r;
        let owner = ex.owner(node);
        let edited = ex.is_edited(node);
        let expanded = ex.expanded.contains(&node);
        let selected = ex.selected == Some(node);
        let depth = ds.depth[node as usize] as f32;
        let n_children = ds.children(node).len();

        let cell = |w: f32| div().w(px(w)).flex_shrink_0().px_2().min_w(px(0.)).truncate();
        let status_name = STATUSES.get(r.status as usize).copied().unwrap_or("—");
        let priority_name = PRIORITIES.get(r.priority as usize).copied().unwrap_or("—");

        let row = div()
            .id(("row", node as usize))
            .flex()
            .w_full()
            .items_center()
            .h(px(ROW_H))
            .px_2()
            .text_sm()
            .border_b_1()
            .border_color(border())
            .cursor_pointer()
            .when(selected, |d| d.bg(alpha(muted(), 0.7)))
            .when(!selected, |d| d.hover(|s| s.bg(alpha(muted(), 0.35))))
            .child(
                div()
                    .w(px(W_CHEVRON))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .child(ui::icon(
                        if expanded { Icon::ChevronDown } else { Icon::ChevronRight },
                        muted_foreground(),
                        14.,
                    )),
            )
            .child(
                cell(W_ID)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .font_family(MONO)
                    .text_xs()
                    .when(edited, |d| d.text_color(chart(3)))
                    .child(r.id.clone())
                    .when(edited, |d| d.child(ui::dot(chart(3)))),
            )
            .when(!compact, |d| {
                d.child(cell(W_SECTION).font_family(MONO).text_xs().text_color(muted_foreground()).child(ds.section[node as usize].clone()))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .px_2()
                    .pl(px(8. + (depth * 10.).min(60.)))
                    .truncate()
                    .when(depth == 0.0, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(r.title.clone()),
            )
            .child(cell(W_TYPE).text_color(muted_foreground()).child(r.kind.clone()))
            .child(
                cell(W_STATUS).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(ui::dot(status_color(r.status)))
                        .child(status_name),
                ),
            )
            .child(cell(W_PRIORITY).text_color(priority_color(r.priority)).child(priority_name))
            .child(
                cell(W_CHILDREN)
                    .text_right()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(if n_children > 0 { foreground() } else { alpha(muted_foreground(), 0.5) })
                    .child(if n_children > 0 { n_children.to_string() } else { "—".into() }),
            )
            .when(!compact, |d| {
                d.child(cell(W_OWNER).text_color(muted_foreground()).child(owner.clone()))
                    .child(cell(W_UPDATED).font_family(MONO).text_xs().text_color(muted_foreground()).child(r.updated.clone()))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(ex) = this.ex.as_mut() {
                    ex.toggle_row(node);
                    ex.reveal_in_tree(node);
                }
                window.focus(&this.focus, cx);
                cx.notify();
            }));

        let weak = cx.entity().downgrade();
        let row_id = r.id.clone();
        let row = row.context_menu(move |menu, _window, _cx| {
            let open = |label: &'static str, icon: IconName, pane: Pane| {
                let weak = weak.clone();
                PopupMenuItem::new(label).icon(icon).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.open_editor(node, pane, window, cx));
                })
            };
            let weak2 = weak.clone();
            let weak3 = weak.clone();
            let weak4 = weak.clone();
            let id = row_id.clone();
            menu.item(open("Edit XML", IconName::FileText, Pane::Xml))
                .item(open("View changes", IconName::Replace, Pane::Changes).disabled(!edited))
                .item(open("Preview description", IconName::Eye, Pane::Preview))
                .separator()
                .item(
                    PopupMenuItem::new("Revert to original")
                        .icon(IconName::Undo2)
                        .disabled(!edited)
                        .on_click(move |_, window, cx| {
                            let _ = weak2.update(cx, |this, cx| this.revert_node(node, window, cx));
                        }),
                )
                .separator()
                .item(PopupMenuItem::new(if expanded { "Collapse row" } else { "Expand row" }).on_click(
                    move |_, _, cx| {
                        let _ = weak3.update(cx, |this, cx| {
                            if let Some(ex) = this.ex.as_mut() {
                                ex.toggle_row(node);
                            }
                            cx.notify();
                        });
                    },
                ))
                .item(PopupMenuItem::new("Reveal in outline").icon(IconName::Map).on_click(move |_, _, cx| {
                    let _ = weak4.update(cx, |this, cx| {
                        if let Some(ex) = this.ex.as_mut() {
                            ex.selected = Some(node);
                            ex.reveal_in_tree(node);
                        }
                        cx.notify();
                    });
                }))
                .item(PopupMenuItem::new("Copy ID").icon(IconName::Copy).on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()));
                    window.push_notification(Notification::info(format!("Copied {id}")), cx);
                }))
        });

        if !expanded {
            return row.into_any_element();
        }

        let body = ex.body(node);
        let parent = ds.parent[node as usize];
        let kids: Vec<u32> = ds.children(node).to_vec();
        let shown = kids.len().min(30);

        let chip = |id: &str, target: u32, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("chip-{node}-{target}")))
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(border())
                .bg(alpha(muted(), 0.4))
                .font_family(MONO)
                .text_xs()
                .cursor_pointer()
                .hover(|s| s.bg(muted()).border_color(input()))
                .child(id.to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(ex) = this.ex.as_mut() {
                        ex.show(target, false);
                        ex.reveal_in_tree(target);
                    }
                    cx.notify();
                }))
        };

        let mut child_chips = Vec::with_capacity(shown);
        for &k in &kids[..shown] {
            child_chips.push(chip(&ds.reqs[k as usize].id, k, cx));
        }
        let parent_chip = (parent != data::NONE).then(|| chip(&ds.reqs[parent as usize].id, parent, cx));

        let side_heading = |s: &'static str| {
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted_foreground())
                .child(s)
        };
        let meta = |k: &'static str, v: String| {
            div()
                .flex()
                .justify_between()
                .gap_2()
                .text_xs()
                .child(div().text_color(muted_foreground()).child(k))
                .child(div().font_family(MONO).truncate().child(v))
        };

        let detail = div()
            .flex()
            .w_full()
            .gap_6()
            .px_6()
            .py_5()
            .bg(alpha(muted(), 0.16))
            .border_b_1()
            .border_color(border())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .flex_1()
                    .min_w(px(0.))
                    .max_w(px(980.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(ui::badge(r.id.clone()).font_family(MONO).bg(muted()))
                                    .child(ui::badge(status_name).child(ui::dot(status_color(r.status))))
                                    .child(ui::badge(priority_name).text_color(priority_color(r.priority)))
                                    .child(ui::badge(r.kind.clone()))
                                    .child(ui::badge(format!("v{}", r.version)).font_family(MONO))
                                    .when(edited, |d| d.child(ui::badge("edited").text_color(chart(3)).child(ui::dot(chart(3)))))
                                    .child(div().flex_1())
                                    .child(
                                        ui::button(("edit-xml", node as usize), Variant::Outline)
                                            .h(px(26.))
                                            .text_xs()
                                            .child("Edit XML")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.open_editor(node, Pane::Xml, window, cx)
                                            })),
                                    )
                                    .when(edited, |d| {
                                        d.child(
                                            ui::button(("view-changes", node as usize), Variant::Ghost)
                                                .h(px(26.))
                                                .text_xs()
                                                .child("View changes")
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    this.open_editor(node, Pane::Changes, window, cx)
                                                })),
                                        )
                                    }),
                            )
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(r.title.clone())),
                    )
                    .child(ui::separator())
                    .child(match &body.error {
                        Some(e) => div().text_sm().text_color(destructive()).child(format!("Failed to load: {e}")),
                        None if body.blocks.is_empty() => {
                            div().text_sm().text_color(muted_foreground()).child("No description.")
                        }
                        None => render_blocks(&body.blocks),
                    }),
            )
            .child(
                div()
                    .w(px(250.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(side_heading("Parent"))
                            .child(match parent_chip {
                                Some(c) => div().flex().child(c),
                                None => div().text_xs().text_color(muted_foreground()).child("Top-level requirement"),
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(side_heading(if kids.is_empty() { "No children" } else { "Children" }))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .children(child_chips)
                                    .when(kids.len() > shown, |d| {
                                        d.child(
                                            div()
                                                .text_xs()
                                                .text_color(muted_foreground())
                                                .px_1()
                                                .child(format!("+{} more", kids.len() - shown)),
                                        )
                                    }),
                            ),
                    )
                    .child(ui::separator())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(side_heading("Source"))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(MONO)
                                    .text_color(foreground())
                                    .child(body.file.clone()),
                            )
                            .child(meta("XML", ui_bytes(body.xml_bytes as u64)))
                            .child(meta("HTML (CDATA)", ui_bytes(body.html_bytes as u64)))
                            .child(meta("Blocks", body.blocks.len().to_string()))
                            .child(meta("Load + parse", format!("{:.0} µs", body.parse_time.as_secs_f64() * 1e6)))
                            .child(meta("Owner", owner.to_string()))
                            .child(meta("Updated", r.updated.to_string()))
                            .child(meta("Section", ds.section[node as usize].to_string())),
                    ),
            );

        div().flex().flex_col().w_full().child(row).child(detail).into_any_element()
    }

    fn render_status_bar(&self) -> Div {
        let ex = self.ex.as_ref().unwrap();
        let s = &ex.ds.stats;
        let secs = s.parse_time.as_secs_f64().max(1e-6);
        let sep = || div().text_color(alpha(muted_foreground(), 0.4)).child("·");
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .h(px(30.))
            .px_4()
            .border_t_1()
            .border_color(border())
            .text_xs()
            .text_color(muted_foreground())
            .child(ui::icon(Icon::Folder, muted_foreground(), 13.))
            .child(div().truncate().max_w(px(260.)).child(ex.ds.source.describe()))
            .child(sep())
            .child(format!("{} XML files · {}", ui_int(s.files), ui_bytes(s.bytes)))
            .child(sep())
            .child(format!(
                "parsed in {:.2}s on {} threads ({}/s)",
                secs,
                s.threads,
                ui_bytes((s.bytes as f64 / secs) as u64)
            ))
            .child(sep())
            .child(format!("tree linked in {} ms", s.link_time.as_millis()))
            .child(sep())
            .child(format!("{} roots · depth {}", ex.ds.roots.len(), s.max_depth + 1))
            .when(s.parse_errors + s.dangling_links + s.extra_parents > 0, |d| {
                d.child(sep()).child(
                    div().text_color(destructive()).child(format!(
                        "{} parse errors · {} dangling · {} multi-parent",
                        s.parse_errors, s.dangling_links, s.extra_parents
                    )),
                )
            })
            .child(div().flex_1())
            .child(div().flex().items_center().gap_1().child(ui::kbd("↑↓")).child("navigate"))
            .child(div().flex().items_center().gap_1().child(ui::kbd("Enter")).child("expand"))
            .child(div().flex().items_center().gap_1().child(ui::kbd("Esc")).child("collapse all"))
            .child(div().flex().items_center().gap_1().child(ui::kbd("Right-click")).child("edit / diff"))
    }
}

impl ReqApp {
    fn render_main(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let table = div()
            .flex()
            .flex_col()
            .size_full()
            .min_w(px(0.))
            .gap_3()
            .p_4()
            .child(self.render_toolbar(window, cx))
            .child(self.render_table(cx));
        match &self.editor {
            None => div().flex().flex_1().min_w(px(0.)).child(table).into_any_element(),
            Some(ed) => div()
                .flex()
                .flex_1()
                .min_w(px(0.))
                .child(
                    h_resizable("main-split")
                        .child(resizable_panel().child(table))
                        .child(
                            resizable_panel()
                                .size(px(620.))
                                .size_range(px(420.)..px(1600.))
                                .child(div().size_full().border_l_1().border_color(border()).child(ed.clone())),
                        ),
                )
                .into_any_element(),
        }
    }
}

fn ds_has_children(ex: &Explorer, node: u32) -> bool {
    ex.ds.has_children(node)
}

impl Render for ReqApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.loading.is_some() {
            self.poll_loading();
            if self.loading.is_some() {
                window.request_animation_frame();
            }
        }
        if let Some(id) = self.ex.is_some().then(|| self.pending_edit.take()).flatten() {
            let node = self.ex.as_ref().and_then(|ex| ex.ds.by_id.get(id.as_str()).copied());
            match node {
                Some(node) => self.open_editor(node, Pane::Xml, window, cx),
                None => window.push_notification(Notification::error(format!("--edit: no requirement {id}")), cx),
            }
        }
        if self.ex.is_none() {
            return self.render_loading();
        }

        div()
            .id("root")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(background())
            .text_color(foreground())
            .child(self.render_header())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.render_sidebar(cx))
                    .child(self.render_main(window, cx)),
            )
            .child(self.render_status_bar())
            .on_key_down(cx.listener(Self::on_root_key))
            .into_any_element()
    }
}

// -------------------------------------------------------------- formatting

pub fn ui_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn compact(n: usize) -> String {
    if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() }
}

pub fn ui_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.1} KB", b / 1e3)
    } else {
        format!("{b:.0} B")
    }
}
