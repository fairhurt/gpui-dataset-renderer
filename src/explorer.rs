//! UI-independent explorer state: tree expansion, table rows, filtering,
//! selection and the lazily-parsed body cache.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{ListAlignment, ListOffset, ListState, ScrollStrategy, SharedString, UniformListScrollHandle, px};
use rayon::prelude::*;

use crate::data::{self, Dataset, Req, xml};
use crate::html::{self, Block};

const BODY_CACHE: usize = 96;

pub struct Body {
    pub blocks: Vec<Block>,
    pub file: String,
    pub xml_bytes: usize,
    pub html_bytes: usize,
    pub parse_time: Duration,
    pub error: Option<String>,
}

/// An in-memory edit of one requirement (the POC never writes back to the source).
pub struct Edit {
    /// The document as the source has it.
    pub original: String,
    /// The saved, edited document.
    pub xml: String,
    /// Metadata re-parsed from `xml`, shown in the table instead of the indexed values.
    pub req: Req,
    pub owner: SharedString,
    pub search_key: Box<str>,
}

pub struct Explorer {
    pub ds: Arc<Dataset>,
    // tree
    pub open: Vec<bool>,
    pub tree_rows: Vec<u32>,
    pub tree_scroll: UniformListScrollHandle,
    // table
    pub rows: Vec<u32>,
    pub list: ListState,
    pub expanded: HashSet<u32>,
    pub selected: Option<u32>,
    // filtering
    pub query: String,
    pub status_filter: Option<u8>,
    pub filter_time: Duration,
    pub edited_only: bool,
    // edits
    pub edits: HashMap<u32, Edit>,
    // bodies
    bodies: HashMap<u32, Rc<Body>>,
    lru: VecDeque<u32>,
}

impl Explorer {
    pub fn new(ds: Dataset) -> Self {
        let ds = Arc::new(ds);
        let n = ds.len();
        let mut open = vec![false; n];
        for &r in &ds.roots {
            open[r as usize] = true;
        }
        let rows = ds.order.clone();
        let list = ListState::new(rows.len(), ListAlignment::Top, px(800.));
        let mut ex = Self {
            ds,
            open,
            tree_rows: Vec::new(),
            tree_scroll: UniformListScrollHandle::new(),
            rows,
            list,
            expanded: HashSet::new(),
            selected: None,
            query: String::new(),
            status_filter: None,
            filter_time: Duration::ZERO,
            edited_only: false,
            edits: HashMap::new(),
            bodies: HashMap::new(),
            lru: VecDeque::new(),
        };
        ex.rebuild_tree();
        ex
    }

    // ---------------------------------------------------------------- tree

    pub fn rebuild_tree(&mut self) {
        self.tree_rows.clear();
        let mut stack: Vec<u32> = self.ds.roots.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            self.tree_rows.push(n);
            if self.open[n as usize] {
                stack.extend(self.ds.children(n).iter().rev());
            }
        }
    }

    pub fn set_all_open(&mut self, open: bool) {
        let ds = self.ds.clone();
        for (i, o) in self.open.iter_mut().enumerate() {
            *o = open && ds.has_children(i as u32);
        }
        if !open {
            for &r in &ds.roots {
                self.open[r as usize] = true;
            }
        }
        self.rebuild_tree();
    }

    pub fn toggle_open(&mut self, node: u32) {
        let o = &mut self.open[node as usize];
        *o = !*o;
        self.rebuild_tree();
    }

    pub fn reveal_in_tree(&mut self, node: u32) {
        let mut changed = false;
        for a in self.ds.ancestors(node) {
            if !self.open[a as usize] {
                self.open[a as usize] = true;
                changed = true;
            }
        }
        if changed {
            self.rebuild_tree();
        }
        if let Some(ix) = self.tree_rows.iter().position(|&n| n == node) {
            self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
    }

    // --------------------------------------------------------------- table

    pub fn row_of(&self, node: u32) -> Option<usize> {
        let pos = &self.ds.pos;
        let target = pos[node as usize];
        self.rows.binary_search_by_key(&target, |&n| pos[n as usize]).ok()
    }

    pub fn apply_filter(&mut self) {
        let t0 = Instant::now();
        let q = self.query.trim().to_lowercase();
        let status = self.status_filter;
        let edited_only = self.edited_only;
        let ds = &self.ds;
        let edits = &self.edits;
        self.rows = if q.is_empty() && status.is_none() && !edited_only {
            ds.order.clone()
        } else {
            ds.order
                .par_iter()
                .copied()
                .filter(|&n| {
                    let edit = edits.get(&n);
                    if edited_only && edit.is_none() {
                        return false;
                    }
                    let (req_status, key) = match edit {
                        Some(e) => (e.req.status, &e.search_key),
                        None => (ds.reqs[n as usize].status, &ds.search_key[n as usize]),
                    };
                    status.is_none_or(|s| req_status == s) && (q.is_empty() || key.contains(q.as_str()))
                })
                .collect()
        };
        self.filter_time = t0.elapsed();
        self.list.reset(self.rows.len());
    }

    fn remeasure(&self, node: u32) {
        if let Some(ix) = self.row_of(node) {
            self.list.splice(ix..ix + 1, 1);
        }
    }

    /// Select + expand a requirement in the table and scroll it to the top.
    /// `exclusive` collapses every other expanded row first.
    pub fn show(&mut self, node: u32, exclusive: bool) {
        self.selected = Some(node);
        if self.row_of(node).is_none() {
            self.query.clear();
            self.status_filter = None;
            self.apply_filter();
        }
        if exclusive {
            let others: Vec<u32> = self.expanded.drain().filter(|&n| n != node).collect();
            for o in others {
                self.remeasure(o);
            }
        }
        self.expanded.insert(node);
        self.body(node);
        if let Some(ix) = self.row_of(node) {
            self.list.splice(ix..ix + 1, 1);
            self.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
        }
    }

    pub fn toggle_row(&mut self, node: u32) {
        self.selected = Some(node);
        if !self.expanded.remove(&node) {
            self.expanded.insert(node);
            self.body(node);
        }
        self.remeasure(node);
    }

    pub fn collapse_all_rows(&mut self) {
        let all: Vec<u32> = self.expanded.drain().collect();
        for n in all {
            self.remeasure(n);
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.selected.and_then(|s| self.row_of(s));
        let ix = match cur {
            Some(i) => (i as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize,
            None => 0,
        };
        self.selected = Some(self.rows[ix]);
        self.list.scroll_to_reveal_item(ix);
    }

    // --------------------------------------------------------------- edits

    /// Table metadata for `node`: the edited version if there is one.
    pub fn req(&self, node: u32) -> &Req {
        match self.edits.get(&node) {
            Some(e) => &e.req,
            None => &self.ds.reqs[node as usize],
        }
    }

    pub fn owner(&self, node: u32) -> SharedString {
        match self.edits.get(&node) {
            Some(e) => e.owner.clone(),
            None => self.ds.owners[self.ds.reqs[node as usize].owner as usize].clone(),
        }
    }

    pub fn is_edited(&self, node: u32) -> bool {
        self.edits.contains_key(&node)
    }

    /// The document as the source has it, ignoring edits.
    pub fn original_xml(&self, node: u32) -> Result<String, String> {
        match self.edits.get(&node) {
            Some(e) => Ok(e.original.clone()),
            None => self.ds.source.read(node as usize),
        }
    }

    /// The document including saved edits.
    pub fn current_xml(&self, node: u32) -> Result<String, String> {
        match self.edits.get(&node) {
            Some(e) => Ok(e.xml.clone()),
            None => self.ds.source.read(node as usize),
        }
    }

    /// Stores an edited document. Saving the original text back removes the edit.
    pub fn save_edit(&mut self, node: u32, new_xml: String) -> Result<(), String> {
        let original = self.original_xml(node)?;
        if new_xml == original {
            self.revert(node);
            return Ok(());
        }
        let p = xml::parse(&new_xml, false)?;
        let base = &self.ds.reqs[node as usize];
        let req = Req {
            id: p.id.clone().into(),
            title: p.title.clone().into(),
            kind: p.kind.clone().into(),
            status: data::status_ix(&p.status),
            priority: data::priority_ix(&p.priority),
            owner: base.owner,
            version: p.version,
            updated: p.updated.clone().into(),
            html_len: p.html_len as u32,
        };
        let search_key = format!("{} {}", p.id, p.title).to_lowercase().into_boxed_str();
        self.edits.insert(node, Edit { original, xml: new_xml, req, owner: p.owner.into(), search_key });
        self.after_edit(node);
        Ok(())
    }

    pub fn revert(&mut self, node: u32) {
        if self.edits.remove(&node).is_some() {
            self.after_edit(node);
        }
    }

    fn after_edit(&mut self, node: u32) {
        self.bodies.remove(&node);
        self.lru.retain(|&n| n != node);
        if self.expanded.contains(&node) {
            self.body(node);
        }
        self.remeasure(node);
    }

    // -------------------------------------------------------------- bodies

    pub fn body(&mut self, node: u32) -> Rc<Body> {
        if let Some(b) = self.bodies.get(&node) {
            return b.clone();
        }
        let t0 = Instant::now();
        let id = self.ds.reqs[node as usize].id.clone();
        let file = self.ds.source.file_name(node as usize, &id);
        let loaded = self
            .current_xml(node)
            .and_then(|x| xml::parse(&x, true).map(|p| (p, x.len())));
        let body = match loaded {
            Ok((parsed, xml_bytes)) => {
                let blocks = html::to_blocks(&parsed.html);
                Body {
                    blocks,
                    file,
                    xml_bytes,
                    html_bytes: parsed.html.len(),
                    parse_time: t0.elapsed(),
                    error: None,
                }
            }
            Err(e) => Body {
                blocks: Vec::new(),
                file,
                xml_bytes: 0,
                html_bytes: 0,
                parse_time: t0.elapsed(),
                error: Some(e),
            },
        };
        let body = Rc::new(body);
        self.bodies.insert(node, body.clone());
        self.lru.push_back(node);
        while self.lru.len() > BODY_CACHE {
            if let Some(old) = self.lru.pop_front() {
                if !self.expanded.contains(&old) {
                    self.bodies.remove(&old);
                } else {
                    self.lru.push_back(old);
                    break;
                }
            }
        }
        body
    }
}
