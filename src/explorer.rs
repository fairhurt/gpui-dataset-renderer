//! UI-independent explorer state: tree expansion, table rows, filtering,
//! selection and the lazily-parsed body cache.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{ListAlignment, ListOffset, ListState, ScrollStrategy, UniformListScrollHandle, px};
use rayon::prelude::*;

use crate::data::Dataset;
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
        let ds = &self.ds;
        self.rows = if q.is_empty() && status.is_none() {
            ds.order.clone()
        } else {
            ds.order
                .par_iter()
                .copied()
                .filter(|&n| {
                    status.is_none_or(|s| ds.reqs[n as usize].status == s)
                        && (q.is_empty() || ds.search_key[n as usize].contains(q.as_str()))
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

    // -------------------------------------------------------------- bodies

    pub fn body(&mut self, node: u32) -> Rc<Body> {
        if let Some(b) = self.bodies.get(&node) {
            return b.clone();
        }
        let t0 = Instant::now();
        let id = self.ds.reqs[node as usize].id.clone();
        let file = self.ds.source.file_name(node as usize, &id);
        let body = match self.ds.load_body(node) {
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
