//! Requirement XML parsing (quick-xml, zero-copy where possible).
//!
//! Expected shape (attribute and element names are matched case-insensitively,
//! and unknown ones are ignored, so real exports with extra fields still load):
//!
//! ```xml
//! <requirement id="R121" type="Functional" status="Approved" priority="High"
//!              owner="A. Chen" version="3" updated="2026-03-14" children="R122,R123">
//!   <title>…</title>
//!   <description><![CDATA[ <p>HTML…</p><table>…</table> ]]></description>
//! </requirement>
//! ```

use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Default, Debug, Clone)]
pub struct ParsedReq {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub priority: String,
    pub owner: String,
    pub version: u32,
    pub updated: String,
    pub children: Vec<String>,
    /// The HTML body (only filled when `with_body` is true).
    pub html: String,
    pub html_len: usize,
}

#[derive(PartialEq)]
enum In {
    None,
    Title,
    Body,
}

pub fn parse(xml: &str, with_body: bool) -> Result<ParsedReq, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut out = ParsedReq::default();
    let mut inside = In::None;
    let mut seen_root = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.local_name();
                let name = name.as_ref();
                if !seen_root {
                    seen_root = true;
                    for attr in e.attributes().flatten() {
                        let key = attr.key.local_name();
                        let value = attr
                            .unescape_value()
                            .map(|v| v.into_owned())
                            .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).into_owned());
                        match key.as_ref().to_ascii_lowercase().as_slice() {
                            b"id" => out.id = value,
                            b"type" | b"kind" => out.kind = value,
                            b"status" | b"state" => out.status = value,
                            b"priority" => out.priority = value,
                            b"owner" | b"author" => out.owner = value,
                            b"version" | b"rev" => out.version = value.trim().parse().unwrap_or(0),
                            b"updated" | b"modified" => out.updated = value,
                            b"children" | b"child" | b"childids" | b"child-ids" => {
                                out.children = value
                                    .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_string)
                                    .collect();
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                inside = match name.to_ascii_lowercase().as_slice() {
                    b"title" | b"name" | b"summary" => In::Title,
                    b"description" | b"text" | b"body" | b"content" => In::Body,
                    _ => In::None,
                };
            }
            Ok(Event::End(_)) => inside = In::None,
            Ok(Event::Text(t)) => {
                if inside == In::Title {
                    let s = t.unescape().map_err(|e| e.to_string())?;
                    out.title.push_str(&s);
                } else if inside == In::Body {
                    // HTML stored escaped instead of in CDATA also works.
                    let s = t.unescape().map_err(|e| e.to_string())?;
                    out.html_len += s.len();
                    if with_body {
                        out.html.push_str(&s);
                    }
                }
            }
            Ok(Event::CData(c)) => {
                if inside == In::Body || inside == In::Title {
                    let bytes = c.into_inner();
                    let s = String::from_utf8_lossy(&bytes);
                    if inside == In::Title {
                        out.title.push_str(&s);
                    } else {
                        out.html_len += s.len();
                        if with_body {
                            out.html.push_str(&s);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(format!("XML error at byte {}: {e}", reader.error_position()));
            }
        }
    }

    if out.id.is_empty() {
        return Err("missing id attribute".into());
    }
    out.title = out.title.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(out)
}
