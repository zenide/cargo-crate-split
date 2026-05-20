//! SCC computation, condensation, layering, and back-edge (feedback-arc-set) heuristic.

use std::collections::{HashMap, HashSet};

use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use serde::Serialize;

use crate::fas::{min_fas_order, NodeId, WeightedEdge};
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
///
/// `respect_order`, when `Some`, is a user-supplied module order (foundation
/// first) used to detect back-edges instead of the ELS-computed ordering: any
/// intra-SCC edge that points "upward" relative to this order is reported as a
/// violation. Modules not named in the spec keep their ELS-computed position
/// (placed after the named ones they depend on). When `None`, the weighted ELS
/// (GR) heuristic computes the order per SCC.
pub fn analyze(mg: &ModuleGraph, package_name: &str, respect_order: Option<&[String]>) -> Analysis {
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
        back_edges.extend(scc_back_edges(g, comp, i, respect_order));
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

/// Compute back-edges within a single SCC.
///
/// Default mode: a weighted vertex ordering (Eades–Lin–Smyth / GR heuristic,
/// weighted by occurrence count) that approximately maximizes forward-edge
/// weight. Back-edges = intra-SCC edges `u -> v` where `v` precedes `u` in the
/// order. Because the order maximizes forward weight, the cut edges are the
/// light minority-direction ones.
///
/// `respect_order` mode: use the user's pinned module order (foundation first)
/// as the vertex order. Modules not named keep their ELS-computed position. Any
/// edge that points "upward" (target later than source in the intended order)
/// is reported as a violation to cut.
fn scc_back_edges(
    g: &petgraph::graph::DiGraph<crate::graph::ModuleNode, crate::graph::EdgeWeight>,
    comp: &[NodeIndex],
    scc_index: usize,
    respect_order: Option<&[String]>,
) -> Vec<BackEdge> {
    let member: HashSet<NodeIndex> = comp.iter().copied().collect();

    // Map this SCC's NodeIndex set to contiguous FAS NodeIds.
    let mut to_id: HashMap<NodeIndex, NodeId> = HashMap::new();
    let mut to_node: Vec<NodeIndex> = Vec::with_capacity(comp.len());
    let mut sorted_comp = comp.to_vec();
    sorted_comp.sort_by_key(|&n| g[n].label());
    for &n in &sorted_comp {
        to_id.insert(n, to_node.len());
        to_node.push(n);
    }
    let nodes: Vec<NodeId> = (0..to_node.len()).collect();

    // Build weighted edges among the SCC members (collapse parallel edges by
    // summing occurrence counts).
    let mut weighted: Vec<WeightedEdge> = Vec::new();
    for edge in g.edge_references() {
        let s = edge.source();
        let t = edge.target();
        if s == t || !member.contains(&s) || !member.contains(&t) {
            continue;
        }
        weighted.push(WeightedEdge {
            from: to_id[&s],
            to: to_id[&t],
            weight: edge.weight().len() as u64,
        });
    }

    // Determine the vertex order, oriented FOUNDATION-FIRST (dependencies
    // before dependents) so both modes share one back-edge rule.
    //
    // `min_fas_order` returns a source-first order: a "forward" edge `u -> v`
    // (kept, not cut) has `u` before `v`. Since our edges mean "source depends
    // on target", source-first puts dependents first. We reverse it to get a
    // foundation-first order, matching the `--respect-order` spec orientation.
    // The pinned spec is already foundation-first.
    let order: Vec<NodeId> = match respect_order {
        None => {
            let mut o = min_fas_order(&nodes, &weighted);
            o.reverse();
            o
        }
        Some(spec) => pinned_order(g, &sorted_comp, &to_id, &nodes, &weighted, spec),
    };
    let pos: HashMap<NodeId, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    // In a foundation-first order a healthy edge points "downward" (a dependent
    // references a more-foundational module: pos(target) < pos(source)). A
    // back-edge / violation is `u -> v` where `v` is LATER (higher) than `u` —
    // a module depending "upward".
    let mut out: Vec<BackEdge> = Vec::new();
    for edge in g.edge_references() {
        let s = edge.source();
        let t = edge.target();
        if s == t || !member.contains(&s) || !member.contains(&t) {
            continue;
        }
        let (sp, tp) = (pos[&to_id[&s]], pos[&to_id[&t]]);
        if tp > sp {
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

/// Build a pinned vertex order from a user `--respect-order` spec.
///
/// Named modules take the positions given by `spec` (foundation first).
/// Unlisted modules are appended after all named ones, placed by their
/// ELS-computed relative order so they still sort after the pinned modules they
/// depend on. The result is a permutation of `nodes`.
fn pinned_order(
    g: &petgraph::graph::DiGraph<crate::graph::ModuleNode, crate::graph::EdgeWeight>,
    sorted_comp: &[NodeIndex],
    to_id: &HashMap<NodeIndex, NodeId>,
    nodes: &[NodeId],
    weighted: &[WeightedEdge],
    spec: &[String],
) -> Vec<NodeId> {
    // Label -> NodeId for members of this SCC.
    let label_to_id: HashMap<String, NodeId> = sorted_comp
        .iter()
        .map(|&n| (g[n].label(), to_id[&n]))
        .collect();

    let mut placed: HashSet<NodeId> = HashSet::new();
    let mut order: Vec<NodeId> = Vec::with_capacity(nodes.len());

    // 1. Named modules present in this SCC, in spec order.
    for label in spec {
        if let Some(&id) = label_to_id.get(label) {
            if placed.insert(id) {
                order.push(id);
            }
        }
    }

    // 2. Unlisted modules: appended after all named ones (so they sort after
    //    the pinned modules they depend on), using their ELS-relative order
    //    reversed to foundation-first to match the spec orientation.
    let mut els = min_fas_order(nodes, weighted);
    els.reverse();
    for id in els {
        if placed.insert(id) {
            order.push(id);
        }
    }

    order
}
