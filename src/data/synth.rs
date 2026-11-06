//! Synthetic requirements dataset.
//!
//! Produces exactly what the real system stores on disk: one XML document per
//! requirement, with child links in an attribute and an HTML body inside a
//! CDATA section. Documents are generated deterministically from their index,
//! so the POC never needs 150k files on disk (but `--write <dir>` can dump them).

use std::collections::VecDeque;
use std::fmt::Write as _;

pub const ID_BASE: u32 = 100;

pub fn id_for(i: u32) -> String {
    format!("R{}", i + ID_BASE)
}

/// Tiny deterministic PRNG (splitmix64).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }
    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

pub struct Synth {
    pub count: u32,
    seed: u64,
    children: Vec<Vec<u32>>,
}

const ROOTS: &[&str] = &[
    "System Overview",
    "Functional Requirements",
    "Performance",
    "Safety & Reliability",
    "Interfaces",
    "Security",
    "Data Management",
    "Verification & Compliance",
];

impl Synth {
    /// Builds a breadth-first tree shape with `count` nodes.
    pub fn new(count: u32, seed: u64) -> Self {
        let n = count.max(ROOTS.len() as u32) as usize;
        let mut rng = Rng::new(seed);
        let mut children = vec![Vec::new(); n];
        let mut depth = vec![0u8; n];
        let roots = ROOTS.len();
        let mut queue: VecDeque<usize> = (0..roots).collect();
        let mut next = roots;
        while next < n {
            let Some(p) = queue.pop_front() else { break };
            let (lo, hi) = match depth[p] {
                0 => (12, 20),
                1 => (6, 12),
                2 => (4, 10),
                3 => (2, 8),
                4 => (0, 6),
                _ => (0, 4),
            };
            let k = rng.range(lo, hi) as usize;
            for _ in 0..k {
                if next >= n {
                    break;
                }
                children[p].push(next as u32);
                depth[next] = depth[p] + 1;
                queue.push_back(next);
                next += 1;
            }
        }
        Self { count: n as u32, seed, children }
    }

    /// The XML document for requirement `i`, exactly as it would be on disk.
    pub fn xml(&self, i: u32) -> String {
        let mut r = Rng::new(self.seed.wrapping_mul(31).wrapping_add(i as u64 * 0x2545_F491));
        let id = id_for(i);
        let is_root = (i as usize) < ROOTS.len();

        let kind = *r.pick(&["Functional", "Performance", "Safety", "Interface", "Security", "Usability", "Regulatory"]);
        let status = *r.pick(&[
            "Approved", "Approved", "Approved", "Implemented", "Implemented", "Verified", "Verified",
            "In Review", "In Review", "Draft", "Draft", "Rejected",
        ]);
        let priority = *r.pick(&["Critical", "High", "High", "Medium", "Medium", "Medium", "Low"]);
        let owner = *r.pick(&OWNERS);
        let version = r.range(1, 14);
        let updated = format!("2026-{:02}-{:02}", r.range(1, 10), r.range(1, 28));

        let title = if is_root {
            ROOTS[i as usize].to_string()
        } else {
            title(&mut r)
        };

        let kids = &self.children[i as usize];
        let mut children_attr = String::with_capacity(kids.len() * 8);
        for (k, c) in kids.iter().enumerate() {
            if k > 0 {
                children_attr.push(',');
            }
            children_attr.push_str(&id_for(*c));
        }

        let html = body_html(&mut r, &id, kind);

        let mut x = String::with_capacity(html.len() + 512);
        x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = write!(
            x,
            "<requirement id=\"{id}\" type=\"{kind}\" status=\"{status}\" priority=\"{priority}\" owner=\"{owner}\" version=\"{version}\" updated=\"{updated}\""
        );
        if !kids.is_empty() {
            let _ = write!(x, " children=\"{children_attr}\"");
        }
        x.push_str(">\n  <title>");
        xml_escape_into(&mut x, &title);
        x.push_str("</title>\n  <description><![CDATA[");
        // A literal "]]>" cannot live inside CDATA; split it the standard way.
        x.push_str(&html.replace("]]>", "]]]]><![CDATA[>"));
        x.push_str("]]></description>\n</requirement>\n");
        x
    }
}

fn xml_escape_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

const OWNERS: [&str; 16] = [
    "A. Chen", "M. Okafor", "S. Lindqvist", "R. Patel", "J. Moreau", "K. Tanaka", "L. García", "D. Novak",
    "E. Fischer", "P. Kowalski", "N. Haddad", "T. Brennan", "Y. Sato", "C. Rossi", "F. Adeyemi", "H. Kim",
];

const SUBJECTS: &[&str] = &[
    "Brake controller", "Telemetry service", "Battery management system", "Operator console", "Flight data recorder",
    "Inverter firmware", "Navigation module", "Thermal subsystem", "Gateway ECU", "Audit logger", "Update agent",
    "Sensor fusion pipeline", "Payment API", "Charging station", "HMI display", "Diagnostics service",
];
const VERBS: &[&str] = &[
    "report", "validate", "persist", "encrypt", "debounce", "arbitrate", "log", "reject", "rate-limit",
    "acknowledge", "calibrate", "publish", "throttle", "reconcile", "time-stamp",
];
const OBJECTS: &[&str] = &[
    "fault codes", "CAN frames", "configuration changes", "user sessions", "cell voltages", "firmware images",
    "heartbeat messages", "sensor samples", "alarm events", "torque requests", "audit records", "GNSS fixes",
];
const CONDITIONS: &[&str] = &[
    "the vehicle speed exceeds 5 km/h", "a watchdog timeout occurs", "supply voltage drops below 9 V",
    "the operator acknowledges the alarm", "network connectivity is lost", "ambient temperature is ≥ 85 °C",
    "a signed update is received", "the system enters degraded mode",
];

fn title(r: &mut Rng) -> String {
    let s = r.pick(SUBJECTS);
    let v = r.pick(VERBS);
    let o = r.pick(OBJECTS);
    match r.below(4) {
        0 => format!("{s} shall {v} {o}"),
        1 => format!("{s} shall {v} {o} within {} ms", r.range(5, 500)),
        2 => format!("{s}: {v} {o} when {}", r.pick(CONDITIONS)),
        _ => format!("{s} shall {v} {o} & expose status <{}>", r.pick(&["OK", "WARN", "FAULT"])),
    }
}

/// An HTML body with the kind of markup real requirement tools emit:
/// paragraphs, inline formatting, entities, lists, tables, quotes and code.
fn body_html(r: &mut Rng, id: &str, kind: &str) -> String {
    let mut h = String::with_capacity(2048);
    let s = r.pick(SUBJECTS);
    let v = r.pick(VERBS);
    let o = r.pick(OBJECTS);
    let ms = r.range(5, 250);
    let _ = write!(
        h,
        "<p>The <b>{s}</b> <strong>shall</strong> {v} all <em>{o}</em> within <code>{ms}&nbsp;ms</code> when {cond}. \
         Tolerance is &plusmn;{tol}&#37; and values &lt; {lo} or &gt; {hi} are rejected &amp; logged.</p>",
        cond = r.pick(CONDITIONS),
        tol = r.range(1, 5),
        lo = r.range(0, 10),
        hi = r.range(100, 4000),
    );

    if r.chance(0.55) {
        h.push_str("<h4>Acceptance criteria</h4><ol>");
        for k in 0..r.range(2, 5) {
            let _ = write!(
                h,
                "<li>Given {}, the {} {} within <i>{}&nbsp;ms</i> (case&nbsp;{}-AC{}).</li>",
                r.pick(CONDITIONS),
                s.to_lowercase(),
                r.pick(&["responds", "recovers", "raises DTC", "stays operational", "enters safe state"]),
                r.range(10, 900),
                id,
                k + 1
            );
        }
        h.push_str("</ol>");
    }

    if r.chance(0.6) {
        h.push_str("<table><thead><tr><th>Parameter</th><th>Min</th><th>Nominal</th><th>Max</th><th>Unit</th></tr></thead><tbody>");
        let params = [
            ("Supply voltage", "V"), ("Latency", "ms"), ("Temperature", "&deg;C"), ("Current draw", "mA"),
            ("Sample rate", "Hz"), ("Acceleration", "m/s<sup>2</sup>"), ("Throughput", "msg/s"), ("Humidity", "%RH"),
        ];
        for _ in 0..r.range(2, 6) {
            let (p, u) = r.pick(&params);
            let min = r.range(0, 50);
            let nom = min + r.range(1, 50);
            let max = nom + r.range(1, 100);
            if r.chance(0.12) {
                let _ = write!(h, "<tr><td>{p}</td><td colspan=\"3\"><em>TBD &mdash; pending HW rev&nbsp;C</em></td><td>{u}</td></tr>");
            } else {
                let _ = write!(h, "<tr><td>{p}</td><td>{min}</td><td><b>{nom}</b></td><td>{max}</td><td>{u}</td></tr>");
            }
        }
        h.push_str("</tbody></table>");
    }

    if r.chance(0.4) {
        let _ = write!(
            h,
            "<blockquote><p><em>Rationale:</em> derived from <a href=\"https://example.com/hazard/{n}\">HAZ-{n}</a> \
             and ISO&nbsp;26262-6 &sect;{a}.{b}. See also &ldquo;{kind} design note&rdquo;.</p></blockquote>",
            n = r.range(100, 999),
            a = r.range(5, 9),
            b = r.range(1, 6),
        );
    }

    if r.chance(0.3) {
        let lim = r.range(60, 120);
        let _ = write!(
            h,
            "<pre>if (temp_c &gt; {lim} &amp;&amp; fan_rpm &lt; 1200) {{\n    raise_dtc(0x{:04X});   // &quot;OVERTEMP&quot;\n    if (buf[idx[0]]>0) trip();\n}}</pre>",
            r.below(0xFFFF)
        );
    }

    if r.chance(0.35) {
        h.push_str("<ul>");
        let _ = write!(h, "<li>Traces to <a href=\"#\">{}</a>; verified by <u>test</u>, <s>analysis</s> inspection.</li>", id);
        let _ = write!(h, "<li>Units: SI. H<sub>2</sub>O ingress per IP{}{}.</li>", r.range(5, 6), r.range(5, 7));
        if r.chance(0.5) {
            h.push_str("<li>Note: the following characters must be escaped in payloads: <code>&lt; &gt; &amp; &quot; &#39;</code></li>");
        }
        h.push_str("</ul>");
    }

    if r.chance(0.15) {
        h.push_str("<p>Legacy markup:<br/>line one<br>line two &copy; 2026 &mdash; <span style=\"color:red\">obsolete</span><hr></p>");
    }
    h
}
