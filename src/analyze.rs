//! SCC computation, condensation, layering, and back-edge (feedback-arc-set) heuristic.

use std::collections::{HashMap, HashSet};

use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use serde::Serialize;

use crate::graph::{ModuleGraph, Occurrence};

/// A reference occurrence rendered for output.
#[derive(Clone, Debug, Serialize)]
pub struct OccurrenceOut {
    pub file: String,
    pub line: usize,
    pub path: String,
}

impl OccurrenceOut {
    fn from(o: &Occurrence) -> Self {
        OccurrenceOut {
            file: o.file.display().to_string(),
            line: o.line,
            path: o.path.clone(),
        }
    }
}

/// A proposed crate (one SCC).
#[derive(Clone, Debug, Serialize)]
pub struct CrateProposal {
    /// Suggested crate name, e.g. `<prefix>-models`.
    pub crate_name: String,
    /// Module labels contained in this crate.
    pub modules: Vec<String>,
    /// Total LOC across contained modules.
    pub loc: usize,
    /// Total file count across contained modules.
    pub file_count: usize,
    /// Dependency layer (0 = foundation/sink crates first... see `Layer`).
    pub layer: usize,
    /// Names of other proposed crates this crate depends on.
    pub depends_on: Vec<String>,
    /// True if this SCC contains a real cycle (>1 module).
    pub is_cycle: bool,
    /// Internal SCC index (for cross-referencing back-edges).
    pub scc: usize,
}

/// A back-edge to remove to break a cycle within a multi-module SCC.
#[derive(Clone, Debug, Serialize)]
pub struct BackEdge {
    pub source_module: String,
    pub target_module: String,
    pub occurrence_count: usize,
    pub occurrences: Vec<OccurrenceOut>,
    /// The SCC (crate proposal) this cycle belongs to.
    pub scc: usize,
}

/// A dependency layer grouping crate proposals.
#[derive(Clone, Debug, Serialize)]
pub struct Layer {
    pub level: usize,
    pub crates: Vec<String>,
}

/// Full analysis result.
#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub package_name: String,
    pub crate_prefix: String,
    pub granularity: usize,
    pub proposals: Vec<CrateProposal>,
    pub layers: Vec<Layer>,
    pub back_edges: Vec<BackEdge>,
    pub summary: Summary,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub module_count: usize,
    pub proposed_crate_count: usize,
    pub largest_crate_loc: usize,
    pub cycle_count: usize,
}

/// Derive a crate-name prefix from the package name (sanitized, no trailing -).
fn crate_prefix(package_name: &str) -> String {
    let mut s: String = package_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .to_lowercase();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

/// Suggest a crate name for an SCC given its module labels.
fn crate_name_for(prefix: &str, modules: &[String]) -> String {
    // Use the shortest (most foundational-looking) module label as the suffix,
    // sanitized into a crate-name fragment.
    let base = modules
        .iter()
        .min_by_key(|m| (m.len(), m.as_str()))
        .cloned()
        .unwrap_or_else(|| "crate".to_string());
    let frag: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .to_lowercase();
    let frag = frag.trim_matches('-').to_string();
    if prefix.is_empty() {
        frag
    } else {
        format!("{prefix}-{frag}")
    }
}

/// Run SCC + condensation + layering + back-edge analysis.
pub fn analyze(mg: &ModuleGraph, package_name: &str) -> Analysis {
    let g = &mg.graph;
    let prefix = crate_prefix(package_name);

    // 1. SCCs (tarjan_scc returns components in reverse topological order).
    let sccs = petgraph::algo::tarjan_scc(g);

    // node index -> scc index
    let mut scc_of: HashMap<NodeIndex, usize> = HashMap::new();
    for (i, comp) in sccs.iter().enumerate() {
        for &n in comp {
            scc_of.insert(n, i);
        }
    }

    // 2. Condensation: SCCs as nodes; edges between distinct SCCs.
    //    Guaranteed acyclic.
    let scc_count = sccs.len();
    let mut cond_succ: Vec<HashSet<usize>> = vec![HashSet::new(); scc_count];
    let mut cond_pred: Vec<HashSet<usize>> = vec![HashSet::new(); scc_count];
    for edge in g.edge_references() {
        let s = scc_of[&edge.source()];
        let t = scc_of[&edge.target()];
        if s != t {
            cond_succ[s].insert(t);
            cond_pred[t].insert(s);
        }
    }

    // 3. Layering on the condensation DAG.
    //    Layer 0 = sinks (no outgoing edges = foundation crates).
    //    A node's layer = longest path to any sink + ... we compute as the
    //    longest downward chain length so foundations sit at layer 0.
    let layer_of = compute_layers(&cond_succ);

    // 4. Build crate proposals (one per SCC).
    let mut proposals: Vec<CrateProposal> = Vec::with_capacity(scc_count);
    for (i, comp) in sccs.iter().enumerate() {
        let mut modules: Vec<String> = comp.iter().map(|&n| g[n].label()).collect();
        modules.sort();
        let loc: usize = comp.iter().map(|&n| g[n].loc).sum();
        let file_count: usize = comp.iter().map(|&n| g[n].file_count).sum();
        let crate_name = crate_name_for(&prefix, &modules);
        proposals.push(CrateProposal {
            crate_name,
            modules,
            loc,
            file_count,
            layer: layer_of[i],
            depends_on: Vec::new(), // filled after names are final
            is_cycle: comp.len() > 1,
            scc: i,
        });
    }

    // Disambiguate duplicate crate names by appending the layer/index.
    dedup_crate_names(&mut proposals);
    let scc_to_name: Vec<String> = proposals.iter().map(|p| p.crate_name.clone()).collect();

    // Fill depends_on using the condensation successors.
    for p in &mut proposals {
        let mut deps: Vec<String> = cond_succ[p.scc]
            .iter()
            .map(|&t| scc_to_name[t].clone())
            .collect();
        deps.sort();
        deps.dedup();
        p.depends_on = deps;
    }

    // 5. Back-edges for multi-module SCCs.
    let mut back_edges: Vec<BackEdge> = Vec::new();
    for (i, comp) in sccs.iter().enumerate() {
        if comp.len() <= 1 {
            continue;
        }
        back_edges.extend(scc_back_edges(g, comp, i));
    }
    // Rank globally by fewest occurrences first (cheapest to remove).
    back_edges.sort_by(|a, b| {
        a.occurrence_count
            .cmp(&b.occurrence_count)
            .then(a.source_module.cmp(&b.source_module))
    });

    // Sort proposals by layer (foundation first), then name.
    proposals.sort_by(|a, b| a.layer.cmp(&b.layer).then(a.crate_name.cmp(&b.crate_name)));

    // Build layer groupings.
    let mut layer_map: HashMap<usize, Vec<String>> = HashMap::new();
    for p in &proposals {
        layer_map.entry(p.layer).or_default().push(p.crate_name.clone());
    }
    let mut layers: Vec<Layer> = layer_map
        .into_iter()
        .map(|(level, mut crates)| {
            crates.sort();
            Layer { level, crates }
        })
        .collect();
    layers.sort_by_key(|l| l.level);

    let module_count = g.node_count();
    let largest_crate_loc = proposals.iter().map(|p| p.loc).max().unwrap_or(0);
    let cycle_count = proposals.iter().filter(|p| p.is_cycle).count();

    Analysis {
        package_name: package_name.to_string(),
        crate_prefix: prefix,
        granularity: mg.granularity,
        summary: Summary {
            module_count,
            proposed_crate_count: proposals.len(),
            largest_crate_loc,
            cycle_count,
        },
        proposals,
        layers,
        back_edges,
    }
}

/// Longest-downward-chain layering: sinks (no successors) = layer 0.
fn compute_layers(succ: &[HashSet<usize>]) -> Vec<usize> {
    let n = succ.len();
    let mut layer = vec![0usize; n];
    let mut memo: Vec<Option<usize>> = vec![None; n];
    // DFS with memoization for longest path to a sink.
    fn longest(
        node: usize,
        succ: &[HashSet<usize>],
        memo: &mut Vec<Option<usize>>,
        on_stack: &mut Vec<bool>,
    ) -> usize {
        if let Some(v) = memo[node] {
            return v;
        }
        if on_stack[node] {
            // Should not happen on a DAG; guard against accidental cycles.
            return 0;
        }
        on_stack[node] = true;
        let mut best = 0;
        for &s in &succ[node] {
            let d = 1 + longest(s, succ, memo, on_stack);
            if d > best {
                best = d;
            }
        }
        on_stack[node] = false;
        memo[node] = Some(best);
        best
    }
    let mut on_stack = vec![false; n];
    for (i, slot) in layer.iter_mut().enumerate() {
        *slot = longest(i, succ, &mut memo, &mut on_stack);
    }
    layer
}

/// Disambiguate duplicate crate names in place.
fn dedup_crate_names(proposals: &mut [CrateProposal]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in proposals.iter() {
        *counts.entry(p.crate_name.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for p in proposals.iter_mut() {
        if counts.get(&p.crate_name).copied().unwrap_or(0) > 1 {
            let n = seen.entry(p.crate_name.clone()).or_default();
            *n += 1;
            p.crate_name = format!("{}-{}", p.crate_name, *n);
        }
    }
}

/// Compute back-edges within a single SCC via a DFS finishing order.
/// Any intra-SCC edge from a later-finished node to an earlier position in
/// the order is a back-edge candidate (heuristic minimal feedback arc set).
fn scc_back_edges(
    g: &petgraph::graph::DiGraph<crate::graph::ModuleNode, crate::graph::EdgeWeight>,
    comp: &[NodeIndex],
    scc_index: usize,
) -> Vec<BackEdge> {
    let member: HashSet<NodeIndex> = comp.iter().copied().collect();

    // DFS over the subgraph induced by `comp`, recording finishing order.
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut finish: Vec<NodeIndex> = Vec::new();

    // Deterministic start order.
    let mut start_nodes = comp.to_vec();
    start_nodes.sort_by_key(|&n| g[n].label());

    fn dfs(
        node: NodeIndex,
        g: &petgraph::graph::DiGraph<crate::graph::ModuleNode, crate::graph::EdgeWeight>,
        member: &HashSet<NodeIndex>,
        visited: &mut HashSet<NodeIndex>,
        finish: &mut Vec<NodeIndex>,
    ) {
        visited.insert(node);
        let mut succ: Vec<NodeIndex> = g
            .neighbors_directed(node, Direction::Outgoing)
            .filter(|n| member.contains(n))
            .collect();
        succ.sort_by_key(|&n| g[n].label());
        for s in succ {
            if !visited.contains(&s) {
                dfs(s, g, member, visited, finish);
            }
        }
        finish.push(node);
    }

    for &n in &start_nodes {
        if !visited.contains(&n) {
            dfs(n, g, &member, &mut visited, &mut finish);
        }
    }

    // Topological-ish order = reverse finishing order. Position in this order.
    let order: Vec<NodeIndex> = finish.iter().rev().copied().collect();
    let pos: HashMap<NodeIndex, usize> = order
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i))
        .collect();

    // Any intra-SCC edge whose target comes before its source in `order`
    // is a back-edge.
    let mut out: Vec<BackEdge> = Vec::new();
    for edge in g.edge_references() {
        let s = edge.source();
        let t = edge.target();
        if !member.contains(&s) || !member.contains(&t) {
            continue;
        }
        if s == t {
            continue;
        }
        let (sp, tp) = (pos[&s], pos[&t]);
        if tp <= sp {
            // back-edge (target earlier-or-equal in topo order)
            let occs: Vec<OccurrenceOut> =
                edge.weight().iter().map(OccurrenceOut::from).collect();
            out.push(BackEdge {
                source_module: g[s].label(),
                target_module: g[t].label(),
                occurrence_count: occs.len(),
                occurrences: occs,
                scc: scc_index,
            });
        }
    }
    out.sort_by(|a, b| {
        a.occurrence_count
            .cmp(&b.occurrence_count)
            .then(a.source_module.cmp(&b.source_module))
    });
    out
}
