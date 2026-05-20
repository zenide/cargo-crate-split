//! Build the petgraph `DiGraph` of module-to-module dependencies.

use std::collections::HashMap;
use std::path::PathBuf;

use petgraph::graph::{DiGraph, NodeIndex};

use crate::discover::Discovery;
use crate::module_path::{ModuleKind, ModulePath};
use crate::parse::references_in_file;

/// A node in the module graph.
#[derive(Clone, Debug)]
pub struct ModuleNode {
    /// Module path truncated to the chosen granularity.
    pub module: ModulePath,
    /// Total lines of code across files mapping to this module.
    pub loc: usize,
    /// Number of files mapping to this module.
    pub file_count: usize,
    /// Whether this module is a binary root.
    pub bin: Option<String>,
}

impl ModuleNode {
    pub fn label(&self) -> String {
        match &self.bin {
            Some(name) => format!("bin:{name}"),
            None => self.module.label(),
        }
    }
}

/// A single occurrence of an edge (one `crate::...` reference).
#[derive(Clone, Debug)]
pub struct Occurrence {
    pub file: PathBuf,
    pub line: usize,
    pub path: String,
}

/// Edge weight: all occurrences that produced this `source -> target` edge.
pub type EdgeWeight = Vec<Occurrence>;

/// The fully-built module graph.
pub struct ModuleGraph {
    pub graph: DiGraph<ModuleNode, EdgeWeight>,
    /// Granularity used to build this graph.
    pub granularity: usize,
}

/// A key identifying a graph node: module path (lib) or a bin root.
fn node_key(module: &ModulePath, kind: &ModuleKind, granularity: usize) -> ModulePath {
    match kind {
        // Bin roots are kept distinct by prefixing a synthetic segment.
        ModuleKind::Bin(name) => {
            let mut segments = vec![format!("\u{1}bin:{name}")];
            segments.extend(module.truncate(granularity.saturating_sub(1)).segments);
            ModulePath { segments }
        }
        ModuleKind::Lib => module.truncate(granularity),
    }
}

/// Build the graph from a discovery result at the given granularity.
pub fn build_graph(disc: &Discovery, granularity: usize) -> anyhow::Result<ModuleGraph> {
    let granularity = granularity.max(1);
    let mut graph: DiGraph<ModuleNode, EdgeWeight> = DiGraph::new();
    let mut index_of: HashMap<ModulePath, NodeIndex> = HashMap::new();

    // First pass: create nodes (one per truncated module key) with LOC/file counts.
    for f in &disc.files {
        let key = node_key(&f.module, &f.kind, granularity);
        let bin = match &f.kind {
            ModuleKind::Bin(name) => Some(name.clone()),
            ModuleKind::Lib => None,
        };
        let idx = *index_of.entry(key.clone()).or_insert_with(|| {
            graph.add_node(ModuleNode {
                module: f.module.truncate(granularity),
                loc: 0,
                file_count: 0,
                bin: bin.clone(),
            })
        });
        let node = &mut graph[idx];
        node.loc += f.loc;
        node.file_count += 1;
        if node.bin.is_none() {
            node.bin = bin;
        }
    }

    // Second pass: parse files, accumulate edges (collapsing occurrences).
    // Edge target keys are always lib top-level modules (truncated).
    let mut edge_acc: HashMap<(NodeIndex, NodeIndex), Vec<Occurrence>> = HashMap::new();

    for f in &disc.files {
        let refs = references_in_file(&f.file, &f.module)?;
        for r in refs {
            // Source node key derives from the reference's enclosing module
            // (which tracks inline `mod x {}` nesting), keyed by the file kind.
            // If that nested key has no node (inline submodule with no own file),
            // fall back to the file's own module node.
            let nested_key = node_key(&r.source, &f.kind, granularity);
            let source_idx = match index_of.get(&nested_key) {
                Some(&i) => i,
                None => {
                    let file_key = node_key(&f.module, &f.kind, granularity);
                    match index_of.get(&file_key) {
                        Some(&i) => i,
                        None => continue,
                    }
                }
            };
            // Target is the lib module identified by the top segment, truncated.
            // The reference `crate::A::B::...` -> the lib module A (or A::B at g=2).
            let target_full = target_module_path(&r.target_top, &r.path);
            let target_key = target_full.truncate(granularity);

            let Some(&target_idx) = index_of.get(&target_key) else {
                // Target module not discovered (e.g. macro-only) — skip.
                continue;
            };
            if source_idx == target_idx {
                continue; // skip self-edges
            }
            edge_acc
                .entry((source_idx, target_idx))
                .or_default()
                .push(Occurrence {
                    file: r.file,
                    line: r.line,
                    path: r.path,
                });
        }
    }

    for ((s, t), occs) in edge_acc {
        graph.add_edge(s, t, occs);
    }

    Ok(ModuleGraph { graph, granularity })
}

/// A directed edge between two module labels, used to identify cuts
/// independently of `NodeIndex` (which is graph-instance specific).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LabelEdge {
    pub source: String,
    pub target: String,
}

impl ModuleGraph {
    /// Compute the strongly-connected components of this graph with the given
    /// set of label-edges removed (`G − cuts`).
    ///
    /// This does NOT mutate the underlying graph: it builds a filtered adjacency
    /// view and runs Tarjan's SCC on that view. Returns the SCCs as groups of
    /// `NodeIndex` (in petgraph's reverse-topological order), matching the shape
    /// of `petgraph::algo::tarjan_scc`.
    ///
    /// An edge `s -> t` is removed iff the `(label(s), label(t))` pair is present
    /// in `cuts`. All parallel occurrences of that module pair are removed
    /// together (cuts are reasoned about at the module-pair level).
    pub fn sccs_without(&self, cuts: &std::collections::HashSet<LabelEdge>) -> Vec<Vec<NodeIndex>> {
        use petgraph::visit::EdgeRef;

        let g = &self.graph;
        // Build a filtered DiGraph carrying the same node count; node weights are
        // not needed for SCC, so we use unit nodes and preserve index parity by
        // adding nodes in NodeIndex order.
        let mut filtered: DiGraph<(), ()> = DiGraph::new();
        let n = g.node_count();
        let mut idx_map: Vec<NodeIndex> = Vec::with_capacity(n);
        for _ in 0..n {
            idx_map.push(filtered.add_node(()));
        }
        for edge in g.edge_references() {
            let s = edge.source();
            let t = edge.target();
            let le = LabelEdge {
                source: g[s].label(),
                target: g[t].label(),
            };
            if cuts.contains(&le) {
                continue;
            }
            filtered.add_edge(idx_map[s.index()], idx_map[t.index()], ());
        }
        // Map filtered NodeIndex back to original NodeIndex (they share index()).
        petgraph::algo::tarjan_scc(&filtered)
            .into_iter()
            .map(|comp| comp.into_iter().map(|n| NodeIndex::new(n.index())).collect())
            .collect()
    }
}

/// Build the target ModulePath from a `crate::...` reference's path string.
/// We reconstruct from the full path so granularity>1 can resolve deeper.
fn target_module_path(target_top: &str, full_path: &str) -> ModulePath {
    // full_path is like "crate::a::b::Func"; drop the leading "crate" and any
    // leading-colon artifacts. We keep all segments after `crate`.
    let cleaned = full_path.trim_start_matches("::");
    let mut segs: Vec<String> = cleaned.split("::").map(|s| s.to_string()).collect();
    if segs.first().map(|s| s.as_str()) == Some("crate") {
        segs.remove(0);
    }
    if segs.is_empty() {
        segs.push(target_top.to_string());
    }
    ModulePath { segments: segs }
}
