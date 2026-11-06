//! Dataset: sources (synthetic or a folder of XML files), a parallel indexer
//! that parses every document once, and the resolved requirement tree.

pub mod synth;
pub mod xml;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui::SharedString;
use rayon::prelude::*;

pub const NONE: u32 = u32::MAX;

// ------------------------------------------------------------- enums ----

pub const STATUSES: [&str; 6] = ["Draft", "In Review", "Approved", "Implemented", "Verified", "Rejected"];
pub const PRIORITIES: [&str; 4] = ["Critical", "High", "Medium", "Low"];

fn status_ix(s: &str) -> u8 {
    STATUSES
        .iter()
        .position(|x| x.eq_ignore_ascii_case(s))
        .map(|i| i as u8)
        .unwrap_or(u8::MAX)
}

fn priority_ix(s: &str) -> u8 {
    PRIORITIES
        .iter()
        .position(|x| x.eq_ignore_ascii_case(s))
        .map(|i| i as u8)
        .unwrap_or(u8::MAX)
}

// ------------------------------------------------------------ source ----

#[derive(Clone)]
pub enum Source {
    Synthetic(Arc<synth::Synth>),
    Dir { root: PathBuf, files: Arc<Vec<PathBuf>> },
}

impl Source {
    pub fn len(&self) -> usize {
        match self {
            Source::Synthetic(s) => s.count as usize,
            Source::Dir { files, .. } => files.len(),
        }
    }

    pub fn read(&self, i: usize) -> Result<String, String> {
        match self {
            Source::Synthetic(s) => Ok(s.xml(i as u32)),
            Source::Dir { files, .. } => std::fs::read_to_string(&files[i]).map_err(|e| e.to_string()),
        }
    }

    pub fn file_name(&self, i: usize, id: &str) -> String {
        match self {
            Source::Synthetic(_) => format!("{id}.xml (generated)"),
            Source::Dir { files, .. } => files[i].display().to_string(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Source::Synthetic(_) => "synthetic, generated in memory".into(),
            Source::Dir { root, .. } => root.display().to_string(),
        }
    }

    pub fn open_dir(root: PathBuf) -> Result<Self, String> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
            .map_err(|e| format!("{}: {e}", root.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(format!("no .xml files in {}", root.display()));
        }
        Ok(Source::Dir { root, files: Arc::new(files) })
    }
}

// ---------------------------------------------------------- progress ----

pub const PHASE_SHAPE: u8 = 0;
pub const PHASE_PARSE: u8 = 1;
pub const PHASE_LINK: u8 = 2;
pub const PHASE_DONE: u8 = 3;

#[derive(Default)]
pub struct Progress {
    pub phase: AtomicU8,
    pub total: AtomicUsize,
    pub done: AtomicUsize,
    pub bytes: AtomicU64,
}

// ----------------------------------------------------------- dataset ----

pub struct Req {
    pub id: SharedString,
    pub title: SharedString,
    pub kind: SharedString,
    pub status: u8,
    pub priority: u8,
    pub owner: u16,
    pub version: u32,
    pub updated: SharedString,
    pub html_len: u32,
}

pub struct Dataset {
    pub source: Source,
    pub reqs: Vec<Req>,
    pub owners: Vec<SharedString>,
    /// CSR child lists.
    child_start: Vec<u32>,
    child_list: Vec<u32>,
    pub parent: Vec<u32>,
    pub depth: Vec<u16>,
    pub roots: Vec<u32>,
    /// Depth-first document order and its inverse.
    pub order: Vec<u32>,
    pub pos: Vec<u32>,
    pub section: Vec<SharedString>,
    /// Lower-cased "id title" for search.
    pub search_key: Vec<Box<str>>,
    pub by_id: HashMap<SharedString, u32>,
    pub stats: Stats,
}

#[derive(Default, Clone)]
pub struct Stats {
    pub files: usize,
    pub bytes: u64,
    pub parse_time: Duration,
    pub link_time: Duration,
    pub threads: usize,
    pub max_depth: u16,
    pub parse_errors: usize,
    pub dangling_links: usize,
    pub extra_parents: usize,
    pub first_error: Option<String>,
}

impl Dataset {
    pub fn children(&self, n: u32) -> &[u32] {
        let s = self.child_start[n as usize] as usize;
        let e = self.child_start[n as usize + 1] as usize;
        &self.child_list[s..e]
    }

    pub fn has_children(&self, n: u32) -> bool {
        self.child_start[n as usize] != self.child_start[n as usize + 1]
    }

    pub fn len(&self) -> usize {
        self.reqs.len()
    }

    /// Ancestors from the root down to (excluding) `n`.
    pub fn ancestors(&self, n: u32) -> Vec<u32> {
        let mut v = Vec::new();
        let mut p = self.parent[n as usize];
        while p != NONE {
            v.push(p);
            p = self.parent[p as usize];
        }
        v.reverse();
        v
    }

    /// Re-reads and parses one document (lazy body load).
    pub fn load_body(&self, n: u32) -> Result<(xml::ParsedReq, usize), String> {
        let xml = self.source.read(n as usize)?;
        let len = xml.len();
        xml::parse(&xml, true).map(|p| (p, len))
    }
}

fn numeric_key(id: &str) -> (u64, &str) {
    let digits: String = id.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits.parse().unwrap_or(u64::MAX), id)
}

/// Parses every document in parallel and links the tree.
pub fn load(source: Source, progress: Arc<Progress>) -> Dataset {
    let n = source.len();
    progress.total.store(n, Ordering::Relaxed);
    progress.phase.store(PHASE_PARSE, Ordering::Relaxed);
    let t0 = Instant::now();

    let parsed: Vec<Result<xml::ParsedReq, String>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let r = source.read(i).and_then(|x| {
                progress.bytes.fetch_add(x.len() as u64, Ordering::Relaxed);
                xml::parse(&x, false)
            });
            progress.done.fetch_add(1, Ordering::Relaxed);
            r
        })
        .collect();
    let parse_time = t0.elapsed();

    progress.phase.store(PHASE_LINK, Ordering::Relaxed);
    let t1 = Instant::now();
    let mut stats = Stats {
        files: n,
        bytes: progress.bytes.load(Ordering::Relaxed),
        parse_time,
        threads: rayon::current_num_threads(),
        ..Default::default()
    };

    let mut owners: Vec<SharedString> = Vec::new();
    let mut owner_ix: HashMap<String, u16> = HashMap::new();
    let mut kinds: HashMap<String, SharedString> = HashMap::new();
    let mut dates: HashMap<String, SharedString> = HashMap::new();
    let mut reqs = Vec::with_capacity(n);
    let mut child_ids: Vec<Vec<String>> = Vec::with_capacity(n);

    for (i, p) in parsed.into_iter().enumerate() {
        let p = match p {
            Ok(p) => p,
            Err(e) => {
                stats.parse_errors += 1;
                if stats.first_error.is_none() {
                    stats.first_error = Some(format!("{}: {e}", source.file_name(i, "?")));
                }
                xml::ParsedReq {
                    id: format!("ERR{i}"),
                    title: format!("⚠ unparseable document: {e}"),
                    status: "Draft".into(),
                    ..Default::default()
                }
            }
        };
        let owner = *owner_ix.entry(p.owner.clone()).or_insert_with(|| {
            owners.push(p.owner.clone().into());
            (owners.len() - 1) as u16
        });
        let kind = kinds.entry(p.kind.clone()).or_insert_with(|| p.kind.clone().into()).clone();
        let updated = dates.entry(p.updated.clone()).or_insert_with(|| p.updated.clone().into()).clone();
        reqs.push(Req {
            id: p.id.into(),
            title: p.title.into(),
            kind,
            status: status_ix(&p.status),
            priority: priority_ix(&p.priority),
            owner,
            version: p.version,
            updated,
            html_len: p.html_len as u32,
        });
        child_ids.push(p.children);
    }

    let by_id: HashMap<SharedString, u32> =
        reqs.iter().enumerate().map(|(i, r)| (r.id.clone(), i as u32)).collect();

    // Resolve child links → parent + CSR children.
    let mut parent = vec![NONE; n];
    let mut child_start = Vec::with_capacity(n + 1);
    let mut child_list = Vec::with_capacity(n);
    child_start.push(0u32);
    for (i, kids) in child_ids.iter().enumerate() {
        for k in kids {
            match by_id.get(k.as_str()) {
                Some(&c) if c as usize != i && parent[c as usize] == NONE => {
                    parent[c as usize] = i as u32;
                    child_list.push(c);
                }
                Some(_) => stats.extra_parents += 1,
                None => stats.dangling_links += 1,
            }
        }
        child_start.push(child_list.len() as u32);
    }
    drop(child_ids);

    // Roots = no parent. (A cycle with no entry point would be unreachable;
    // the DFS below re-roots anything left unvisited.)
    let mut roots: Vec<u32> = (0..n as u32).filter(|&i| parent[i as usize] == NONE).collect();
    roots.sort_by(|a, b| numeric_key(&reqs[*a as usize].id).cmp(&numeric_key(&reqs[*b as usize].id)));

    let mut ds = Dataset {
        source,
        reqs,
        owners,
        child_start,
        child_list,
        parent,
        depth: vec![0; n],
        roots,
        order: Vec::with_capacity(n),
        pos: vec![NONE; n],
        section: vec![SharedString::default(); n],
        search_key: Vec::new(),
        by_id,
        stats: Stats::default(),
    };

    // Iterative DFS for document order, depth and section numbers.
    let mut stack: Vec<(u32, u16, String)> = Vec::new();
    let mut root_list = ds.roots.clone();
    let mut extra_root_ix = 0u32;
    loop {
        for (k, r) in root_list.iter().enumerate().rev() {
            stack.push((*r, 0, format!("{}", k as u32 + 1 + extra_root_ix)));
        }
        extra_root_ix += root_list.len() as u32;
        while let Some((node, d, sec)) = stack.pop() {
            if ds.pos[node as usize] != NONE {
                continue;
            }
            ds.pos[node as usize] = ds.order.len() as u32;
            ds.order.push(node);
            ds.depth[node as usize] = d;
            stats.max_depth = stats.max_depth.max(d);
            let kids: Vec<u32> = ds.children(node).to_vec();
            for (k, c) in kids.iter().enumerate().rev() {
                stack.push((*c, d + 1, format!("{sec}.{}", k + 1)));
            }
            ds.section[node as usize] = sec.into();
        }
        // Unreached nodes (cycles) become extra roots.
        root_list = (0..n as u32).filter(|&i| ds.pos[i as usize] == NONE).take(1).collect();
        if root_list.is_empty() {
            break;
        }
        ds.parent[root_list[0] as usize] = NONE;
        ds.roots.push(root_list[0]);
    }

    ds.search_key = ds
        .reqs
        .par_iter()
        .map(|r| format!("{} {}", r.id, r.title).to_lowercase().into_boxed_str())
        .collect();

    stats.link_time = t1.elapsed();
    ds.stats = stats;
    progress.phase.store(PHASE_DONE, Ordering::Relaxed);
    ds
}

/// Writes the synthetic dataset to `dir` as one XML file per requirement.
pub fn write_synthetic(dir: &std::path::Path, count: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let synth = synth::Synth::new(count, 7);
    let t0 = Instant::now();
    let written = AtomicUsize::new(0);
    (0..synth.count).into_par_iter().try_for_each(|i| {
        let path = dir.join(format!("{}.xml", synth::id_for(i)));
        std::fs::write(path, synth.xml(i))?;
        let w = written.fetch_add(1, Ordering::Relaxed) + 1;
        if w % 10_000 == 0 {
            println!("  {w} / {}", synth.count);
        }
        Ok::<_, std::io::Error>(())
    })?;
    println!("Wrote {} files to {} in {:.2?}", synth.count, dir.display(), t0.elapsed());
    Ok(())
}
