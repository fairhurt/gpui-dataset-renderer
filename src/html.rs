//! A small, forgiving HTML parser that turns requirement bodies into a
//! renderable block model. Handles entities, void/self-closing tags, implicit
//! closes (p, li, td/th, tr, thead/tbody), unclosed and stray tags, comments,
//! and script/style content.

use std::ops::Range;

// ------------------------------------------------------------------ DOM ----

#[derive(Debug, Clone)]
pub enum Node {
    Element { tag: String, attrs: Vec<(String, String)>, children: Vec<Node> },
    Text(String),
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        match self {
            Node::Element { attrs, .. } => attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr",
];
const BLOCKS_CLOSING_P: &[&str] = &[
    "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "table", "pre", "blockquote", "hr", "section",
    "article", "header", "footer", "dl", "figure",
];

struct Open {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

pub fn parse_dom(html: &str) -> Vec<Node> {
    let b = html.as_bytes();
    let mut stack: Vec<Open> = vec![Open { tag: String::new(), attrs: vec![], children: vec![] }];
    let mut i = 0;
    let mut text_start = 0;

    let flush_text = |stack: &mut Vec<Open>, s: &str| {
        if !s.is_empty() {
            stack.last_mut().unwrap().children.push(Node::Text(decode_entities(s)));
        }
    };

    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &html[i..];
        // Comment
        if rest.starts_with("<!--") {
            flush_text(&mut stack, &html[text_start..i]);
            i = rest.find("-->").map(|e| i + e + 3).unwrap_or(b.len());
            text_start = i;
            continue;
        }
        // Doctype / processing instruction / CDATA-ish
        if rest.starts_with("<!") || rest.starts_with("<?") {
            flush_text(&mut stack, &html[text_start..i]);
            i = rest.find('>').map(|e| i + e + 1).unwrap_or(b.len());
            text_start = i;
            continue;
        }
        let closing = rest.as_bytes().get(1) == Some(&b'/');
        let name_start = i + if closing { 2 } else { 1 };
        if !b.get(name_start).is_some_and(|c| c.is_ascii_alphabetic()) {
            // A bare '<' in text, e.g. "a < b".
            i += 1;
            continue;
        }
        let Some(end) = find_tag_end(b, name_start) else {
            i += 1;
            continue;
        };
        flush_text(&mut stack, &html[text_start..i]);
        let inner = &html[name_start..end];
        let self_closing = inner.ends_with('/');
        let inner = inner.trim_end_matches('/');
        let name_len = inner
            .find(|c: char| c.is_whitespace())
            .unwrap_or(inner.len());
        let tag = inner[..name_len].to_ascii_lowercase();
        i = end + 1;
        text_start = i;

        if closing {
            close_tag(&mut stack, &tag);
            continue;
        }

        let attrs = parse_attrs(&inner[name_len..]);
        implicit_close(&mut stack, &tag);

        if VOID.contains(&tag.as_str()) || self_closing {
            stack.last_mut().unwrap().children.push(Node::Element { tag, attrs, children: vec![] });
            continue;
        }

        // Raw text elements: skip their content entirely.
        if tag == "script" || tag == "style" {
            let needle = format!("</{tag}");
            let lower = html[i..].to_ascii_lowercase();
            i = lower.find(&needle).map(|e| i + e).unwrap_or(b.len());
            i = html[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
            text_start = i;
            continue;
        }

        stack.push(Open { tag, attrs, children: vec![] });
    }
    flush_text(&mut stack, &html[text_start..]);
    while stack.len() > 1 {
        pop_into_parent(&mut stack);
    }
    stack.pop().map(|o| o.children).unwrap_or_default()
}

/// Finds the closing '>' of a tag, respecting quoted attribute values.
fn find_tag_end(b: &[u8], from: usize) -> Option<usize> {
    let mut quote = 0u8;
    for (k, &c) in b.iter().enumerate().skip(from) {
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return Some(k);
        } else if c == b'<' {
            return None;
        }
    }
    None
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let ks = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        if ks == i {
            break;
        }
        let key = s[ks..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let vs = i + 1;
                i = vs;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                val = decode_entities(&s[vs..i.min(b.len())]);
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                val = decode_entities(&s[vs..i]);
            }
        }
        out.push((key, val));
    }
    out
}

fn pop_into_parent(stack: &mut Vec<Open>) {
    let o = stack.pop().unwrap();
    stack.last_mut().unwrap().children.push(Node::Element { tag: o.tag, attrs: o.attrs, children: o.children });
}

fn close_tag(stack: &mut Vec<Open>, tag: &str) {
    if let Some(pos) = stack.iter().rposition(|o| o.tag == tag) {
        if pos == 0 {
            return;
        }
        while stack.len() > pos {
            pop_into_parent(stack);
        }
    }
    // Unmatched close tags are ignored.
}

/// Closes the nearest open element in `targets`, unless a `boundary` is hit first.
fn close_nearest(stack: &mut Vec<Open>, targets: &[&str], boundaries: &[&str]) {
    for pos in (1..stack.len()).rev() {
        let t = stack[pos].tag.as_str();
        if targets.contains(&t) {
            while stack.len() > pos {
                pop_into_parent(stack);
            }
            return;
        }
        if boundaries.contains(&t) {
            return;
        }
    }
}

fn implicit_close(stack: &mut Vec<Open>, tag: &str) {
    if BLOCKS_CLOSING_P.contains(&tag) && stack.last().is_some_and(|o| o.tag == "p") {
        pop_into_parent(stack);
    }
    match tag {
        "li" => close_nearest(stack, &["li"], &["ul", "ol"]),
        "td" | "th" => close_nearest(stack, &["td", "th"], &["tr", "table"]),
        "tr" => close_nearest(stack, &["tr"], &["table", "thead", "tbody", "tfoot"]),
        "thead" | "tbody" | "tfoot" => close_nearest(stack, &["thead", "tbody", "tfoot"], &["table"]),
        "dt" | "dd" => close_nearest(stack, &["dt", "dd"], &["dl"]),
        _ => {}
    }
}

// ------------------------------------------------------------- entities ----

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after.find(';').filter(|&k| k > 0 && k <= 10);
        let decoded = semi.and_then(|k| {
            let name = &after[..k];
            let ch = if let Some(num) = name.strip_prefix('#') {
                let code = if let Some(hex) = num.strip_prefix(['x', 'X']) {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    num.parse::<u32>().ok()
                };
                code.and_then(char::from_u32).map(|c| c.to_string())
            } else {
                named_entity(name).map(str::to_string)
            };
            ch.map(|c| (c, k))
        });
        match decoded {
            Some((c, k)) => {
                out.push_str(&c);
                rest = &after[k + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "deg" => "°",
        "plusmn" => "±",
        "micro" => "µ",
        "middot" => "·",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "laquo" => "«",
        "raquo" => "»",
        "le" => "≤",
        "ge" => "≥",
        "ne" => "≠",
        "asymp" => "≈",
        "infin" => "∞",
        "times" => "×",
        "divide" => "÷",
        "minus" => "−",
        "sect" => "§",
        "para" => "¶",
        "bull" => "•",
        "dagger" => "†",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "larr" => "←",
        "rarr" => "→",
        "uarr" => "↑",
        "darr" => "↓",
        "harr" => "↔",
        "rArr" => "⇒",
        "sup1" => "¹",
        "sup2" => "²",
        "sup3" => "³",
        "frac12" => "½",
        "frac14" => "¼",
        "frac34" => "¾",
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "Delta" => "Δ",
        "lambda" => "λ",
        "mu" => "μ",
        "pi" => "π",
        "sigma" => "σ",
        "Sigma" => "Σ",
        "tau" => "τ",
        "omega" => "ω",
        "Omega" => "Ω",
        "check" => "✓",
        "shy" => "",
        "zwj" => "",
        "zwnj" => "",
        "thinsp" => "\u{2009}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        _ => return None,
    })
}

// ---------------------------------------------------------- block model ----

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const CODE: u8 = 4;
pub const LINK: u8 = 8;
pub const UNDERLINE: u8 = 16;
pub const STRIKE: u8 = 32;
pub const MARK: u8 = 64;

#[derive(Debug, Clone, Default)]
pub struct Inline {
    pub text: String,
    /// Contiguous, non-overlapping styled ranges (bytes) with style flags.
    pub spans: Vec<(Range<usize>, u8)>,
}

impl Inline {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub header: bool,
    pub colspan: u16,
    pub align: Align,
    pub content: Vec<Block>,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub header: bool,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone)]
pub enum Block {
    Para(Inline),
    Heading(u8, Inline),
    List { ordered: bool, start: u32, items: Vec<Vec<Block>> },
    Table { caption: Option<Inline>, rows: Vec<Row>, columns: u16 },
    Quote(Vec<Block>),
    Code(String),
    Rule,
    Image(String),
}

pub fn to_blocks(html: &str) -> Vec<Block> {
    let dom = parse_dom(html);
    blocks_of(&dom)
}

fn blocks_of(nodes: &[Node]) -> Vec<Block> {
    let mut b = Builder::default();
    b.walk(nodes, 0);
    b.flush();
    b.out
}

fn inline_of(nodes: &[Node], flags: u8) -> Inline {
    let mut b = Builder::default();
    b.walk(nodes, flags);
    b.inl.finish()
}

#[derive(Default)]
struct InlineBuilder {
    text: String,
    spans: Vec<(Range<usize>, u8)>,
    last_space: bool,
}

impl InlineBuilder {
    fn push(&mut self, s: &str, flags: u8) {
        let start = self.text.len();
        for ch in s.chars() {
            if ch.is_whitespace() && ch != '\u{a0}' {
                if !self.last_space && !self.text.is_empty() {
                    self.text.push(' ');
                    self.last_space = true;
                }
            } else {
                self.text.push(ch);
                self.last_space = false;
            }
        }
        self.mark(start, flags);
    }

    fn newline(&mut self) {
        while self.text.ends_with(' ') {
            self.text.pop();
        }
        let len = self.text.len();
        self.spans.retain_mut(|(r, _)| {
            r.end = r.end.min(len);
            r.start < r.end
        });
        self.text.push('\n');
        self.last_space = true;
        self.mark(len, 0);
    }

    fn mark(&mut self, start: usize, flags: u8) {
        let end = self.text.len();
        if end <= start {
            return;
        }
        if let Some((r, f)) = self.spans.last_mut() {
            if *f == flags && r.end == start {
                r.end = end;
                return;
            }
        }
        self.spans.push((start..end, flags));
    }

    fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    fn finish(&mut self) -> Inline {
        // Trim trailing whitespace/newlines and clamp spans.
        while self.text.ends_with([' ', '\n']) {
            self.text.pop();
        }
        let len = self.text.len();
        let mut spans: Vec<(Range<usize>, u8)> = std::mem::take(&mut self.spans)
            .into_iter()
            .filter_map(|(r, f)| {
                let e = r.end.min(len);
                (r.start < e).then_some((r.start..e, f))
            })
            .collect();
        spans.dedup_by(|b, a| {
            if a.1 == b.1 && a.0.end == b.0.start {
                a.0.end = b.0.end;
                true
            } else {
                false
            }
        });
        self.last_space = false;
        Inline { text: std::mem::take(&mut self.text), spans }
    }
}

#[derive(Default)]
struct Builder {
    out: Vec<Block>,
    inl: InlineBuilder,
}

impl Builder {
    fn flush(&mut self) {
        if !self.inl.is_empty() {
            let i = self.inl.finish();
            self.out.push(Block::Para(i));
        } else {
            self.inl = InlineBuilder::default();
        }
    }

    fn walk(&mut self, nodes: &[Node], flags: u8) {
        for node in nodes {
            match node {
                Node::Text(t) => self.inl.push(t, flags),
                Node::Element { tag, children, .. } => {
                    let t = tag.as_str();
                    match t {
                        "b" | "strong" => self.walk(children, flags | BOLD),
                        "i" | "em" | "cite" | "dfn" | "var" => self.walk(children, flags | ITALIC),
                        "code" | "kbd" | "samp" | "tt" => self.walk(children, flags | CODE),
                        "a" => self.walk(children, flags | LINK),
                        "u" | "ins" => self.walk(children, flags | UNDERLINE),
                        "s" | "strike" | "del" => self.walk(children, flags | STRIKE),
                        "mark" => self.walk(children, flags | MARK),
                        "sup" => {
                            let txt = text_content(children);
                            self.inl.push(&superscript(&txt), flags);
                        }
                        "sub" => {
                            let txt = text_content(children);
                            self.inl.push(&subscript(&txt), flags);
                        }
                        "br" => self.inl.newline(),
                        "img" => {
                            let alt = node.attr("alt").unwrap_or("image").to_string();
                            self.flush();
                            self.out.push(Block::Image(alt));
                        }
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            self.flush();
                            let level = t.as_bytes()[1] - b'0';
                            let inl = inline_of(children, flags | BOLD);
                            if !inl.is_empty() {
                                self.out.push(Block::Heading(level, inl));
                            }
                        }
                        "ul" | "ol" | "menu" => {
                            self.flush();
                            let start = node.attr("start").and_then(|s| s.parse().ok()).unwrap_or(1);
                            let mut items = Vec::new();
                            for c in children {
                                match c {
                                    Node::Element { tag, children, .. } if tag == "li" => {
                                        items.push(blocks_of(children))
                                    }
                                    Node::Text(t) if t.trim().is_empty() => {}
                                    other => items.push(blocks_of(std::slice::from_ref(other))),
                                }
                            }
                            if !items.is_empty() {
                                self.out.push(Block::List { ordered: t == "ol", start, items });
                            }
                        }
                        "table" => {
                            self.flush();
                            self.out.push(table(children));
                        }
                        "blockquote" => {
                            self.flush();
                            self.out.push(Block::Quote(blocks_of(children)));
                        }
                        "pre" | "xmp" | "listing" => {
                            self.flush();
                            let mut s = pre_text(children);
                            if s.starts_with('\n') {
                                s.remove(0);
                            }
                            while s.ends_with(['\n', ' ']) {
                                s.pop();
                            }
                            self.out.push(Block::Code(s));
                        }
                        "hr" => {
                            self.flush();
                            self.out.push(Block::Rule);
                        }
                        "head" | "title" | "script" | "style" | "template" | "noscript" => {}
                        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside"
                        | "center" | "address" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "li" | "form"
                        | "fieldset" | "details" | "summary" | "body" | "html" | "tr" | "td" | "th" | "thead"
                        | "tbody" | "tfoot" | "caption" => {
                            self.flush();
                            self.walk(children, flags);
                            self.flush();
                        }
                        _ => self.walk(children, flags),
                    }
                }
            }
        }
    }
}

fn table(children: &[Node]) -> Block {
    let mut rows = Vec::new();
    let mut caption = None;
    fn collect_rows(nodes: &[Node], header: bool, rows: &mut Vec<Row>, caption: &mut Option<Inline>) {
        for n in nodes {
            if let Node::Element { tag, children, .. } = n {
                match tag.as_str() {
                    "thead" => collect_rows(children, true, rows, caption),
                    "tbody" | "tfoot" => collect_rows(children, false, rows, caption),
                    "caption" => *caption = Some(inline_of(children, 0)),
                    "tr" => {
                        let mut cells = Vec::new();
                        for c in children {
                            if let Node::Element { tag, children, .. } = c {
                                if tag == "td" || tag == "th" {
                                    let colspan = c.attr("colspan").and_then(|s| s.trim().parse().ok()).unwrap_or(1u16).clamp(1, 64);
                                    let align = c
                                        .attr("align")
                                        .map(str::to_string)
                                        .or_else(|| {
                                            c.attr("style").and_then(|s| {
                                                s.split(';').find_map(|d| {
                                                    let (k, v) = d.split_once(':')?;
                                                    (k.trim() == "text-align").then(|| v.trim().to_string())
                                                })
                                            })
                                        })
                                        .map(|a| match a.as_str() {
                                            "center" => Align::Center,
                                            "right" => Align::Right,
                                            _ => Align::Left,
                                        })
                                        .unwrap_or(Align::Left);
                                    cells.push(Cell { header: header || tag == "th", colspan, align, content: blocks_of(children) });
                                }
                            }
                        }
                        if !cells.is_empty() {
                            let all_th = cells.iter().all(|c| c.header);
                            rows.push(Row { header: header || all_th, cells });
                        }
                    }
                    // Tolerate stray cells directly in the table.
                    _ => {}
                }
            }
        }
    }
    collect_rows(children, false, &mut rows, &mut caption);
    let columns = rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.colspan).sum::<u16>())
        .max()
        .unwrap_or(1)
        .max(1);
    Block::Table { caption, rows, columns }
}

fn text_content(nodes: &[Node]) -> String {
    let mut s = String::new();
    for n in nodes {
        match n {
            Node::Text(t) => s.push_str(t),
            Node::Element { children, .. } => s.push_str(&text_content(children)),
        }
    }
    s
}

fn pre_text(nodes: &[Node]) -> String {
    let mut s = String::new();
    for n in nodes {
        match n {
            Node::Text(t) => s.push_str(t),
            Node::Element { tag, children, .. } => {
                if tag == "br" {
                    s.push('\n');
                } else {
                    s.push_str(&pre_text(children));
                }
            }
        }
    }
    s
}

fn superscript(s: &str) -> String {
    map_chars(s, "0123456789+-=()n", "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁿ").unwrap_or_else(|| format!("^{s}"))
}

fn subscript(s: &str) -> String {
    map_chars(s, "0123456789+-=()", "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎").unwrap_or_else(|| format!("_{s}"))
}

fn map_chars(s: &str, from: &str, to: &str) -> Option<String> {
    let from: Vec<char> = from.chars().collect();
    let to: Vec<char> = to.chars().collect();
    s.trim()
        .chars()
        .map(|c| from.iter().position(|f| *f == c).map(|i| to[i]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities() {
        assert_eq!(decode_entities("a &lt; b &amp;&amp; c &gt; d &#169; &#x2264; &bogus; &"), "a < b && c > d © ≤ &bogus; &");
    }

    #[test]
    fn implicit_closes() {
        let b = to_blocks("<p>one<p>two<ul><li>a<li>b</ul><table><tr><td>1<td>2<tr><td colspan=2>x</table>");
        assert!(matches!(b[0], Block::Para(_)));
        assert!(matches!(b[1], Block::Para(_)));
        match &b[2] {
            Block::List { items, .. } => assert_eq!(items.len(), 2),
            _ => panic!(),
        }
        match &b[3] {
            Block::Table { rows, columns, .. } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(*columns, 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn stray_and_unclosed() {
        let b = to_blocks("</div>text <b>bold <i>both</b> after a < b<br>next");
        match &b[0] {
            Block::Para(i) => assert!(i.text.contains("a < b\nnext")),
            _ => panic!(),
        }
    }
}
