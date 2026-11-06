//! XML editor panel, built from gpui-component pieces:
//!
//! - `input::Editor` code editor with tree-sitter XML highlighting (HTML injected into CDATA),
//!   line numbers, folding, search (Ctrl+F) and soft wrap,
//! - live validation (well-formedness via quick-xml + requirement schema checks) shown as
//!   editor diagnostics and a clickable problems list,
//! - a changes view (`diff::Diff`, unified or split) of the buffer against the original,
//! - a rendered HTML preview of the description, using the same renderer as the table.
//!
//! Saving emits [`EditorEvent::Save`]; the app keeps edits in memory (the POC never writes
//! back to the source).

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding, SharedString, Subscription,
    Window, actions, div, prelude::*, px,
};
use gpui_component::{
    Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    diff::{Diff, DiffFile, DiffMode, DiffState},
    highlighter::{Diagnostic, DiagnosticSeverity, GrammarConfig, LanguageRegistry},
    input::{Editor, EditorState, InputEvent, Position},
    tab::{Tab, TabBar},
};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::data::{Dataset, PRIORITIES, STATUSES, xml};
use crate::html::{self, Block};
use crate::html_render::render_blocks;
use crate::theme::*;
use crate::ui;

actions!(xml_editor, [Save]);

const CONTEXT: &str = "XmlEditor";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-s", Save, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-s", Save, Some(CONTEXT)),
    ]);
}

// ------------------------------------------------------------- language ----

/// Registers tree-sitter-xml as language "xml". The CDATA body is injected as HTML, so the
/// description markup gets highlighted too.
pub fn register_xml_language() {
    let config = GrammarConfig::new(
        "xml",
        tree_sitter::Language::new(tree_sitter_xml::LANGUAGE_XML),
        vec!["html".into()],
        &xml_highlights(),
        XML_INJECTIONS,
        "",
    );
    LanguageRegistry::singleton().register("xml", &config);
}

const XML_INJECTIONS: &str = "((CData) @injection.content\n (#set! injection.language \"html\"))\n";

/// The grammar's own query, minus the capture that paints the whole CDATA body as one
/// "raw" span (it would cover the injected HTML highlighting).
fn xml_highlights() -> String {
    let q = tree_sitter_xml::XML_HIGHLIGHT_QUERY.replace("\r\n", "\n");
    q.replace(
        "(CDSect\n  (CDStart) @markup.heading\n  (CData) @markup.raw\n  \"]]>\" @markup.heading)",
        "(CDSect\n  (CDStart) @keyword\n  \"]]>\" @keyword)",
    )
}

// ----------------------------------------------------------- validation ----

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug)]
pub struct Problem {
    /// Byte range in the document.
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
}

impl Problem {
    fn new(severity: Severity, range: Range<usize>, message: impl Into<String>) -> Self {
        Self { range, severity, message: message.into() }
    }
}

/// Well-formedness (quick-xml) plus requirement-specific checks.
///
/// `original_children` is the `children` attribute of the indexed document; `known_id` tells
/// whether a requirement ID exists in the dataset.
pub fn validate(text: &str, original_children: &[String], known_id: &dyn Fn(&str) -> bool) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);

    let mut stack: Vec<(String, Range<usize>)> = Vec::new();
    let mut root: Option<(String, Range<usize>)> = None;
    let mut fatal = false;

    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(ev @ (Event::Start(_) | Event::Empty(_))) => {
                let after = reader.buffer_position() as usize;
                let (name, is_start) = match &ev {
                    Event::Start(e) => (String::from_utf8_lossy(e.name().as_ref()).into_owned(), true),
                    Event::Empty(e) => (String::from_utf8_lossy(e.name().as_ref()).into_owned(), false),
                    _ => unreachable!(),
                };
                let span = before..after;
                if root.is_none() {
                    root = Some((name.clone(), span.clone()));
                } else if stack.is_empty() {
                    out.push(Problem::new(Severity::Error, span.clone(), format!("second root element <{name}>")));
                }
                if is_start {
                    stack.push((name, span));
                }
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(t)) if stack.is_empty() && root.is_some() => {
                if !t.iter().all(u8::is_ascii_whitespace) {
                    let after = reader.buffer_position() as usize;
                    out.push(Problem::new(Severity::Error, before..after, "text after the root element"));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                let pos = (reader.error_position() as usize).min(text.len());
                out.push(Problem::new(Severity::Error, rest_of_line(text, pos), e.to_string()));
                fatal = true;
                break;
            }
        }
    }

    if !fatal {
        for (name, span) in stack.iter().rev() {
            out.push(Problem::new(Severity::Error, span.clone(), format!("<{name}> is never closed")));
        }
    }
    let Some((root_name, root_span)) = root else {
        // After a syntax error the root may simply not have been reached yet.
        if !fatal {
            out.push(Problem::new(Severity::Error, 0..text.len().min(1), "no root element"));
        }
        return out;
    };
    if fatal || !stack.is_empty() {
        return out;
    }

    // Requirement schema.
    let parsed = match xml::parse(text, false) {
        Ok(p) => p,
        Err(e) => {
            out.push(Problem::new(Severity::Error, root_span, e));
            return out;
        }
    };
    if !root_name.eq_ignore_ascii_case("requirement") {
        let r = root_span.start + 1..root_span.start + 1 + root_name.len();
        out.push(Problem::new(Severity::Warning, r, format!("root element is <{root_name}>, expected <requirement>")));
    }
    let attr = |names: &[&str]| attr_value(text, root_span.clone(), names);
    let enum_check = |out: &mut Vec<Problem>, names: &[&str], what: &str, allowed: &[&str]| {
        if let Some(r) = attr(names) {
            let v = &text[r.clone()];
            if !allowed.iter().any(|a| a.eq_ignore_ascii_case(v)) {
                out.push(Problem::new(
                    Severity::Warning,
                    widen(text, r),
                    format!("unknown {what} \"{v}\"; expected one of: {}", allowed.join(", ")),
                ));
            }
        } else {
            out.push(Problem::new(Severity::Warning, root_span.clone(), format!("missing {what} attribute")));
        }
    };
    enum_check(&mut out, &["status", "state"], "status", &STATUSES);
    enum_check(&mut out, &["priority"], "priority", &PRIORITIES);

    if let Some(r) = attr(&["version", "rev"]) {
        if text[r.clone()].trim().parse::<u32>().is_err() {
            out.push(Problem::new(Severity::Warning, widen(text, r), "version should be a whole number"));
        }
    }
    if let Some(r) = attr(&["updated", "modified"]) {
        if !is_iso_date(&text[r.clone()]) {
            out.push(Problem::new(Severity::Warning, widen(text, r), "updated should be a date like 2026-03-14"));
        }
    }
    if parsed.title.trim().is_empty() {
        out.push(Problem::new(Severity::Warning, root_span.clone(), "requirement has no <title>"));
    }
    if let Some(r) = attr(&["children", "child", "childids", "child-ids"]) {
        let value = &text[r.clone()];
        let mut i = 0;
        for part in value.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
            let start = r.start + i;
            i += part.len() + 1;
            if !part.is_empty() && !known_id(part) {
                out.push(Problem::new(Severity::Warning, start..start + part.len(), format!("{part} does not exist")));
            }
        }
        if parsed.children != original_children {
            out.push(Problem::new(
                Severity::Info,
                widen(text, r),
                "child links are read when the dataset is indexed; the outline updates after a reload",
            ));
        }
    }
    out.sort_by_key(|p| (p.severity, p.range.start));
    out
}

/// Value range (without quotes) of the first attribute in `span` matching one of `names`.
fn attr_value(text: &str, span: Range<usize>, names: &[&str]) -> Option<Range<usize>> {
    let lower = text[span.clone()].to_ascii_lowercase();
    let b = lower.as_bytes();
    for name in names {
        let mut from = 0;
        while let Some(i) = lower[from..].find(name) {
            let start = from + i;
            from = start + name.len();
            if start == 0 || !b[start - 1].is_ascii_whitespace() {
                continue;
            }
            let mut j = from;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if b.get(j) != Some(&b'=') {
                continue;
            }
            j += 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let q = *b.get(j)?;
            if q != b'"' && q != b'\'' {
                continue;
            }
            let close = lower[j + 1..].find(q as char)? + j + 1;
            return Some(span.start + j + 1..span.start + close);
        }
    }
    None
}

/// Empty attribute values still need something to underline: take the quotes.
fn widen(text: &str, r: Range<usize>) -> Range<usize> {
    if r.is_empty() { r.start.saturating_sub(1)..(r.end + 1).min(text.len()) } else { r }
}

fn rest_of_line(text: &str, pos: usize) -> Range<usize> {
    let end = text[pos..].find('\n').map(|i| pos + i).unwrap_or(text.len());
    if end > pos {
        pos..end
    } else {
        let mut s = pos.saturating_sub(1);
        while !text.is_char_boundary(s) {
            s -= 1;
        }
        s..pos
    }
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Byte offset → (0-based line, UTF-16 column), the editor's coordinate system.
pub fn position(text: &str, offset: usize) -> Position {
    let mut o = offset.min(text.len());
    while !text.is_char_boundary(o) {
        o -= 1;
    }
    let before = &text[..o];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = before[line_start..].encode_utf16().count();
    Position::new(line as u32, col as u32)
}

// ----------------------------------------------------------------- diff ----

/// Unified git-style patch of `a` → `b`, plus added/removed line counts.
pub fn make_patch(file: &str, a: &str, b: &str) -> (String, usize, usize) {
    let diff = similar::TextDiff::from_lines(a, b);
    let (mut added, mut removed) = (0, 0);
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    let body = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{file}"), &format!("b/{file}"))
        .to_string();
    (format!("diff --git a/{file} b/{file}\n{body}"), added, removed)
}

// ---------------------------------------------------------------- panel ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Xml,
    Changes,
    Preview,
}

pub enum EditorEvent {
    Save { node: u32, xml: String },
    RevertToOriginal { node: u32 },
    Close,
}

pub struct XmlEditor {
    pub node: u32,
    pub id: SharedString,
    file: SharedString,
    /// The source document.
    original: String,
    /// The last saved text (the original, or a saved in-memory edit).
    saved: String,
    original_children: Vec<String>,
    ds: Arc<Dataset>,

    editor: Entity<EditorState>,
    diff: Entity<DiffState>,
    pane: Pane,
    diff_mode: DiffMode,
    soft_wrap: bool,

    // derived from the buffer on every change
    text: String,
    dirty: bool,
    problems: Vec<Problem>,
    preview: Result<Vec<Block>, String>,
    added: usize,
    removed: usize,
    check_time: Duration,
    /// Focus the text on the next render (a focus request before the editor is first
    /// painted is dropped by GPUI).
    focus_pending: bool,

    _subs: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for XmlEditor {}

impl Focusable for XmlEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

impl XmlEditor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        node: u32,
        id: SharedString,
        original: String,
        current: String,
        ds: Arc<Dataset>,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("xml")
                .line_number(true)
                .folding(true)
                .soft_wrap(true)
                .default_value(current.clone())
        });
        let diff = cx.new(|cx| DiffState::new(Vec::<DiffFile>::new(), cx).with_mode(DiffMode::Unified));
        let subs = vec![cx.subscribe_in(&editor, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.refresh(window, cx);
            }
        })];
        let original_children = xml::parse(&original, false).map(|p| p.children).unwrap_or_default();
        let mut this = Self {
            node,
            file: format!("{id}.xml").into(),
            id,
            original,
            saved: current,
            original_children,
            ds,
            editor,
            diff,
            pane,
            diff_mode: DiffMode::Unified,
            soft_wrap: true,
            text: String::new(),
            dirty: false,
            problems: Vec::new(),
            preview: Ok(Vec::new()),
            added: 0,
            removed: 0,
            check_time: Duration::ZERO,
            focus_pending: true,
            _subs: subs,
        };
        this.refresh(window, cx);
        this
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn errors(&self) -> usize {
        self.problems.iter().filter(|p| p.severity == Severity::Error).count()
    }

    pub fn set_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = pane;
        if pane == Pane::Xml {
            self.editor.update(cx, |e, cx| e.focus(window, cx));
        }
        cx.notify();
    }

    pub fn focus_editor(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        cx.notify();
    }

    /// The app saved our text: it is the new baseline.
    pub fn mark_saved(&mut self, xml: String, cx: &mut Context<Self>) {
        self.saved = xml;
        self.dirty = self.text != self.saved;
        cx.notify();
    }

    /// Replace the buffer (e.g. after "revert to original").
    pub fn reload(&mut self, xml: String, window: &mut Window, cx: &mut Context<Self>) {
        self.saved = xml.clone();
        self.editor.update(cx, |e, cx| e.set_value(xml, window, cx));
        self.refresh(window, cx);
    }

    /// Re-derive validation, diagnostics, preview and diff from the buffer.
    fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        self.text = self.editor.read(cx).value().to_string();
        self.dirty = self.text != self.saved;

        let ds = self.ds.clone();
        self.problems = validate(&self.text, &self.original_children, &|id| ds.by_id.contains_key(id));

        let text = &self.text;
        let problems = &self.problems;
        let rope = self.editor.read(cx).text().clone();
        self.editor.update(cx, |e, cx| {
            if let Some(set) = e.diagnostics_mut() {
                set.reset(&rope);
                for p in problems {
                    let severity = match p.severity {
                        Severity::Error => DiagnosticSeverity::Error,
                        Severity::Warning => DiagnosticSeverity::Warning,
                        Severity::Info => DiagnosticSeverity::Info,
                    };
                    set.push(
                        Diagnostic::new(position(text, p.range.start)..position(text, p.range.end), p.message.clone())
                            .with_severity(severity)
                            .with_source("requirements"),
                    );
                }
            }
            cx.notify();
        });

        self.preview = xml::parse(&self.text, true).map(|p| html::to_blocks(&p.html));

        let (patch, added, removed) = make_patch(&self.file, &self.original, &self.text);
        self.added = added;
        self.removed = removed;
        let files = if added + removed == 0 {
            Vec::new()
        } else {
            DiffFile::parse(&patch)
                .map(|files| files.into_iter().map(|f| f.with_language("xml")).collect())
                .unwrap_or_default()
        };
        self.diff.update(cx, |d, cx| d.set_files(files, cx));

        self.check_time = t0.elapsed();
        cx.notify();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.try_save(window, cx);
    }

    fn try_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_component::WindowExt as _;
        let errors = self.errors();
        if errors > 0 {
            window.push_notification(
                gpui_component::notification::Notification::error(format!(
                    "{} has {errors} error{} — fix {} before saving.",
                    self.id,
                    if errors == 1 { "" } else { "s" },
                    if errors == 1 { "it" } else { "them" }
                )),
                cx,
            );
            return;
        }
        if !self.dirty {
            return;
        }
        cx.emit(EditorEvent::Save { node: self.node, xml: self.text.clone() });
    }

    fn jump_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let pos = position(&self.text, offset);
        self.pane = Pane::Xml;
        self.editor.update(cx, |e, cx| {
            e.set_cursor_position(pos, window, cx);
            e.focus(window, cx);
        });
        cx.notify();
    }

    fn set_diff_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        self.diff_mode = mode;
        self.diff.update(cx, |d, cx| d.set_mode(mode, cx));
        cx.notify();
    }

    fn set_soft_wrap(&mut self, wrap: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.soft_wrap = wrap;
        self.editor.update(cx, |e, cx| e.set_soft_wrap(wrap, window, cx));
        cx.notify();
    }

    // ------------------------------------------------------------ render

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let edited = self.saved != self.original;
        let can_save = self.dirty && self.errors() == 0;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .h(px(48.))
            .px_3()
            .border_b_1()
            .border_color(border())
            .child(ui::icon(ui::Icon::File, muted_foreground(), 15.))
            .child(div().font_family(MONO).text_sm().font_weight(FontWeight::SEMIBOLD).child(self.file.clone()))
            .when(self.dirty, |d| d.child(ui::badge("unsaved").text_color(chart(2)).child(ui::dot(chart(2)))))
            .when(!self.dirty && edited, |d| d.child(ui::badge("edited in memory").text_color(chart(3)).child(ui::dot(chart(3)))))
            .child(div().flex_1())
            .child(
                Button::new("revert-original")
                    .ghost()
                    .small()
                    .icon(IconName::Undo2)
                    .label("Revert to original")
                    .disabled(!edited && self.text == self.original)
                    .tooltip("Drop the saved edit and reload the source document")
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(EditorEvent::RevertToOriginal { node: this.node });
                    })),
            )
            .child(
                Button::new("discard")
                    .ghost()
                    .small()
                    .label("Discard")
                    .disabled(!self.dirty)
                    .tooltip("Throw away unsaved changes")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let saved = this.saved.clone();
                        this.reload(saved, window, cx);
                    })),
            )
            .child(
                Button::new("save")
                    .primary()
                    .small()
                    .label("Save")
                    .disabled(!can_save)
                    .tooltip("Save (Ctrl+S) — kept in memory")
                    .on_click(cx.listener(|this, _, window, cx| this.try_save(window, cx))),
            )
            .child(
                Button::new("close-editor")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .tooltip("Close editor")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(EditorEvent::Close))),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ix = match self.pane {
            Pane::Xml => 0,
            Pane::Changes => 1,
            Pane::Preview => 2,
        };
        let changes = if self.added + self.removed == 0 {
            "Changes".to_string()
        } else {
            format!("Changes  +{} −{}", self.added, self.removed)
        };
        let errors = self.errors();
        let warnings = self.problems.iter().filter(|p| p.severity == Severity::Warning).count();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .pr_3()
            .border_b_1()
            .border_color(border())
            .child(
                TabBar::new("editor-tabs")
                    .underline()
                    .small()
                    .selected_index(ix)
                    .child(Tab::new().label("XML"))
                    .child(Tab::new().label(changes))
                    .child(Tab::new().label("Preview"))
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        let pane = match ix {
                            0 => Pane::Xml,
                            1 => Pane::Changes,
                            _ => Pane::Preview,
                        };
                        this.set_pane(pane, window, cx);
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(if errors > 0 {
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(destructive())
                            .child(ui::dot(destructive()))
                            .child(format!("{errors} error{}", if errors == 1 { "" } else { "s" }))
                    } else {
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(chart(1))
                            .child(ui::dot(chart(1)))
                            .child("Well-formed")
                    })
                    .when(warnings > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(chart(2))
                                .child(ui::dot(chart(2)))
                                .child(format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" })),
                        )
                    }),
            )
    }

    fn render_problems(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("problems")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .max_h(px(150.))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(border())
            .py_1()
            .children(self.problems.iter().enumerate().map(|(i, p)| {
                let pos = position(&self.text, p.range.start);
                let color = match p.severity {
                    Severity::Error => destructive(),
                    Severity::Warning => chart(2),
                    Severity::Info => chart(0),
                };
                let offset = p.range.start;
                div()
                    .id(("problem", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h(px(24.))
                    .text_xs()
                    .cursor_pointer()
                    .hover(|s| s.bg(alpha(muted(), 0.5)))
                    .child(ui::dot(color))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(p.message.clone()))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_color(muted_foreground())
                            .child(format!("Ln {}, Col {}", pos.line + 1, pos.character + 1)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.jump_to(offset, window, cx)))
            }))
    }

    fn render_xml(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .child(div().flex_1().min_h(px(0.)).child(Editor::new(&self.editor).bordered(false).h_full()))
            .when(!self.problems.is_empty(), |d| d.child(self.render_problems(cx)))
    }

    fn render_changes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.diff_mode;
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(40.))
            .border_b_1()
            .border_color(border())
            .child(div().text_xs().text_color(muted_foreground()).child("Original source → current buffer"))
            .child(div().flex_1())
            .child(
                Button::new("diff-unified")
                    .small()
                    .when(mode == DiffMode::Unified, |b| b.primary())
                    .when(mode != DiffMode::Unified, |b| b.ghost())
                    .label("Unified")
                    .on_click(cx.listener(|this, _, _, cx| this.set_diff_mode(DiffMode::Unified, cx))),
            )
            .child(
                Button::new("diff-split")
                    .small()
                    .when(mode == DiffMode::Split, |b| b.primary())
                    .when(mode != DiffMode::Split, |b| b.ghost())
                    .label("Split")
                    .on_click(cx.listener(|this, _, _, cx| this.set_diff_mode(DiffMode::Split, cx))),
            );
        let body = if self.added + self.removed == 0 {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(muted_foreground())
                .child("No changes against the original document.")
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h(px(0.))
                .child(Diff::new(&self.diff).line_number(true).syntax_highlight(true).size_full())
                .into_any_element()
        };
        div().flex().flex_col().flex_1().min_h(px(0.)).child(toolbar).child(body)
    }

    fn render_preview(&self) -> impl IntoElement {
        let content = match &self.preview {
            Err(e) => div().text_sm().text_color(destructive()).child(format!("Cannot render: {e}")),
            Ok(blocks) if blocks.is_empty() => div().text_sm().text_color(muted_foreground()).child("No description."),
            Ok(blocks) => render_blocks(blocks),
        };
        div()
            .id("preview")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px_6()
            .py_5()
            .child(content)
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = self.editor.read(cx).cursor();
        let pos = position(&self.text, cursor);
        let lines = self.text.lines().count();
        let wrap = self.soft_wrap;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .h(px(28.))
            .px_3()
            .border_t_1()
            .border_color(border())
            .text_xs()
            .text_color(muted_foreground())
            .child(format!("Ln {}, Col {}", pos.line + 1, pos.character + 1))
            .child(format!("{lines} lines · {}", crate::app::ui_bytes(self.text.len() as u64)))
            .child(format!("checked in {:.2} ms", self.check_time.as_secs_f64() * 1000.0))
            .child(div().flex_1())
            .child("XML · HTML in CDATA")
            .child(
                Button::new("soft-wrap")
                    .ghost()
                    .xsmall()
                    .label(if wrap { "Wrap: on" } else { "Wrap: off" })
                    .on_click(cx.listener(move |this, _, window, cx| this.set_soft_wrap(!wrap, window, cx))),
            )
            .child(div().flex().items_center().gap_1().child(ui::kbd("Ctrl+F")).child("find"))
            .child(div().flex().items_center().gap_1().child(ui::kbd("Ctrl+S")).child("save"))
    }
}

impl Render for XmlEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_pending) && self.pane == Pane::Xml {
            self.editor.update(cx, |e, cx| e.focus(window, cx));
        }
        let body = match self.pane {
            Pane::Xml => self.render_xml(cx).into_any_element(),
            Pane::Changes => self.render_changes(cx).into_any_element(),
            Pane::Preview => self.render_preview().into_any_element(),
        };
        div()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::save))
            .size_full()
            .flex()
            .flex_col()
            .bg(background())
            .text_color(foreground())
            .child(self.render_header(cx))
            .child(self.render_tabs(cx))
            .child(body)
            .child(self.render_footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "<requirement id=\"R1\" status=\"Approved\" priority=\"High\" version=\"2\" updated=\"2026-01-02\" children=\"R2\">\n  <title>T</title>\n  <description><![CDATA[<p>x</p>]]></description>\n</requirement>\n";

    fn check(text: &str) -> Vec<Problem> {
        validate(text, &["R2".to_string()], &|id| id == "R1" || id == "R2")
    }

    #[test]
    fn valid_document_has_no_problems() {
        assert!(check(DOC).is_empty(), "{:?}", check(DOC));
    }

    #[test]
    fn mismatched_tag_is_an_error() {
        let p = check(&DOC.replace("</title>", "</titel>"));
        assert_eq!(p[0].severity, Severity::Error);
    }

    #[test]
    fn unclosed_tag_is_an_error() {
        let p = check(&DOC.replace("</requirement>\n", ""));
        assert!(p.iter().any(|p| p.severity == Severity::Error && p.message.contains("never closed")), "{p:?}");
    }

    #[test]
    fn schema_warnings_point_at_the_value() {
        let doc = DOC.replace("Approved", "Approvd").replace("\"R2\"", "\"R2,R404\"");
        let p = check(&doc);
        let status = p.iter().find(|p| p.message.contains("unknown status")).unwrap();
        assert_eq!(&doc[status.range.clone()], "Approvd");
        let missing = p.iter().find(|p| p.message.contains("R404")).unwrap();
        assert_eq!(&doc[missing.range.clone()], "R404");
        assert!(p.iter().any(|p| p.severity == Severity::Info));
    }

    #[test]
    fn broken_root_tag_reports_one_error() {
        // A missing quote in the root tag: one syntax error, no "no root element" follow-up.
        let p = check(&DOC.replacen("Approved\"", "Approved", 1));
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].severity, Severity::Error);
    }

    #[test]
    fn positions_are_line_and_column() {
        let pos = position("ab\ncd", 4);
        assert_eq!((pos.line, pos.character), (1, 1));
    }

    #[test]
    fn highlight_query_drops_raw_cdata_capture() {
        let q = xml_highlights();
        assert!(!q.contains("@markup.raw"));
    }

    #[test]
    fn xml_language_highlights_tags_and_injected_html() {
        use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
        register_xml_language();
        let rope = gpui_component::Rope::from(DOC);
        let mut h = SyntaxHighlighter::new("xml");
        h.update(None, &rope, None);
        let theme = HighlightTheme::default_dark();
        let styles = h.styles(&(0..DOC.len()), &*theme);
        let colored = |needle: &str| {
            let at = DOC.find(needle).unwrap();
            styles.iter().any(|(r, s)| r.contains(&at) && s.color.is_some())
        };
        assert!(colored("requirement"), "root tag not highlighted");
        assert!(colored("status"), "attribute not highlighted");
        // `p` inside CDATA comes from the injected HTML grammar.
        assert!(colored("p>x"), "HTML inside CDATA not highlighted");
    }

    #[test]
    fn patch_parses() {
        let (patch, added, removed) = make_patch("R1.xml", DOC, &DOC.replace("High", "Low"));
        assert_eq!((added, removed), (1, 1));
        let files = DiffFile::parse(&patch).unwrap();
        assert_eq!(files.len(), 1);
    }
}
