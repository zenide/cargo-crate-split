//! Weighted feedback-arc-set heuristic via the Eades–Lin–Smyth (GR) greedy
//! linear arrangement.
//!
//! Given a directed (cyclic) graph with positive edge weights, we compute a
//! vertex ordering that approximately *maximizes* the total weight of forward
//! edges (equivalently, minimizes the total weight of backward edges). Edges
//! that point "backward" in the resulting order form the feedback arc set —
//! the cheapest edges to cut to make the graph acyclic.
//!
//! The classic ELS / "GR" heuristic:
//!   - Repeatedly peel any sink (no outgoing edges) and append it to the *end*
//!     of the order.
//!   - Repeatedly peel any source (no incoming edges) and prepend it to the
//!     *start* of the order.
//!   - When neither a sink nor a source remains, remove the vertex maximizing
//!     `weighted_out_degree − weighted_in_degree` and prepend it.
//!
//! On a DAG this reproduces a topological order (no back-edges). On a cycle it
//! cuts the lightest minority-direction edges.

use std::collections::HashMap;

/// Opaque vertex identifier used by the FAS solver. Callers map their own node
/// indices to a contiguous `0..n` range.
pub type NodeId = usize;

/// A weighted directed edge between two vertices in the FAS subgraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightedEdge {
    pub from: NodeId,
    pub to: NodeId,
    /// Edge weight (e.g. number of reference occurrences). Treated as a count.
    pub weight: u64,
}

/// Compute a vertex ordering (foundation/source first ... sink last) that
/// approximately maximizes forward-edge weight using the weighted ELS / GR
/// heuristic.
///
/// `nodes` is the set of vertex ids to order (need not be contiguous, but each
/// must be distinct). `edges` are the weighted directed edges among them; edges
/// referencing a vertex not in `nodes`, and self-loops, are ignored.
///
/// The returned vector is a permutation of `nodes`. A directed edge `u -> v` is
/// a "back-edge" iff `v` appears before `u` in this order.
pub fn min_fas_order(nodes: &[NodeId], edges: &[WeightedEdge]) -> Vec<NodeId> {
    let node_set: std::collections::HashSet<NodeId> = nodes.iter().copied().collect();

    // Weighted out/in degree and adjacency, restricted to `node_set`, no loops.
    let mut out_w: HashMap<NodeId, i64> = nodes.iter().map(|&n| (n, 0)).collect();
    let mut in_w: HashMap<NodeId, i64> = nodes.iter().map(|&n| (n, 0)).collect();
    let mut succ: HashMap<NodeId, Vec<(NodeId, u64)>> =
        nodes.iter().map(|&n| (n, Vec::new())).collect();
    let mut pred: HashMap<NodeId, Vec<(NodeId, u64)>> =
        nodes.iter().map(|&n| (n, Vec::new())).collect();

    for e in edges {
        if e.from == e.to || !node_set.contains(&e.from) || !node_set.contains(&e.to) {
            continue;
        }
        *out_w.get_mut(&e.from).unwrap() += e.weight as i64;
        *in_w.get_mut(&e.to).unwrap() += e.weight as i64;
        succ.get_mut(&e.from).unwrap().push((e.to, e.weight));
        pred.get_mut(&e.to).unwrap().push((e.from, e.weight));
    }

    let mut removed: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    let mut head: Vec<NodeId> = Vec::new(); // built front-to-back (sources)
    let mut tail: Vec<NodeId> = Vec::new(); // built back-to-front (sinks), reversed at end

    // Remaining live degree as we peel vertices.
    let mut live_out: HashMap<NodeId, i64> = out_w.clone();
    let mut live_in: HashMap<NodeId, i64> = in_w.clone();

    // Remove a vertex: subtract its contribution from neighbours' live degree.
    fn remove_vertex(
        v: NodeId,
        succ: &HashMap<NodeId, Vec<(NodeId, u64)>>,
        pred: &HashMap<NodeId, Vec<(NodeId, u64)>>,
        removed: &std::collections::HashSet<NodeId>,
        live_out: &mut HashMap<NodeId, i64>,
        live_in: &mut HashMap<NodeId, i64>,
    ) {
        for &(t, w) in &succ[&v] {
            if !removed.contains(&t) {
                *live_in.get_mut(&t).unwrap() -= w as i64;
            }
        }
        for &(s, w) in &pred[&v] {
            if !removed.contains(&s) {
                *live_out.get_mut(&s).unwrap() -= w as i64;
            }
        }
    }

    // Deterministic iteration order over the not-yet-removed vertices.
    let order_pool: Vec<NodeId> = nodes.to_vec();

    while removed.len() < nodes.len() {
        // 1. Peel all current sinks (no outgoing live edges) -> append to tail.
        let mut progress = true;
        while progress {
            progress = false;
            for &v in &order_pool {
                if removed.contains(&v) {
                    continue;
                }
                if live_out[&v] == 0 {
                    tail.push(v);
                    removed.insert(v);
                    remove_vertex(v, &succ, &pred, &removed, &mut live_out, &mut live_in);
                    progress = true;
                }
            }
        }

        // 2. Peel all current sources (no incoming live edges) -> prepend (push to head).
        progress = true;
        while progress {
            progress = false;
            for &v in &order_pool {
                if removed.contains(&v) {
                    continue;
                }
                if live_in[&v] == 0 {
                    head.push(v);
                    removed.insert(v);
                    remove_vertex(v, &succ, &pred, &removed, &mut live_out, &mut live_in);
                    progress = true;
                }
            }
        }

        // 3. If vertices remain, remove the one maximizing (out - in); prepend to head.
        if removed.len() < nodes.len() {
            let mut best: Option<(NodeId, i64)> = None;
            for &v in &order_pool {
                if removed.contains(&v) {
                    continue;
                }
                let delta = live_out[&v] - live_in[&v];
                match best {
                    Some((_, bd)) if delta <= bd => {}
                    _ => best = Some((v, delta)),
                }
            }
            if let Some((v, _)) = best {
                head.push(v);
                removed.insert(v);
                remove_vertex(v, &succ, &pred, &removed, &mut live_out, &mut live_in);
            } else {
                break; // safety: nothing selectable
            }
        }
    }

    // Final order: head (sources, in selection order) followed by tail reversed
    // (sinks, last-removed first). `head` was pushed front-to-back already.
    let mut order = head;
    tail.reverse();
    order.extend(tail);
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(order: &[NodeId], v: NodeId) -> usize {
        order.iter().position(|&x| x == v).unwrap()
    }

    /// Back-edges under an order: edges u->v where v precedes u.
    fn back_edges(order: &[NodeId], edges: &[WeightedEdge]) -> Vec<WeightedEdge> {
        edges
            .iter()
            .copied()
            .filter(|e| e.from != e.to)
            .filter(|e| position(order, e.to) < position(order, e.from))
            .collect()
    }

    #[test]
    fn dag_has_no_back_edges() {
        // 0 -> 1 -> 2, 0 -> 2. Acyclic.
        let nodes = vec![0, 1, 2];
        let edges = vec![
            WeightedEdge { from: 0, to: 1, weight: 5 },
            WeightedEdge { from: 1, to: 2, weight: 5 },
            WeightedEdge { from: 0, to: 2, weight: 5 },
        ];
        let order = min_fas_order(&nodes, &edges);
        let be = back_edges(&order, &edges);
        assert!(be.is_empty(), "DAG must have no back-edges, got {be:?}");
        // Source 0 first, sink 2 last.
        assert!(position(&order, 0) < position(&order, 1));
        assert!(position(&order, 1) < position(&order, 2));
    }

    #[test]
    fn two_node_cycle_cuts_lighter_edge() {
        // x(0) <-> y(1): x->y weight 10, y->x weight 2. Must cut y->x.
        let nodes = vec![0, 1];
        let edges = vec![
            WeightedEdge { from: 0, to: 1, weight: 10 },
            WeightedEdge { from: 1, to: 0, weight: 2 },
        ];
        let order = min_fas_order(&nodes, &edges);
        let be = back_edges(&order, &edges);
        assert_eq!(be.len(), 1, "exactly one back-edge in a 2-cycle");
        assert_eq!(be[0].from, 1, "must cut y->x (the lighter, weight 2)");
        assert_eq!(be[0].to, 0);
        assert_eq!(be[0].weight, 2);
    }

    #[test]
    fn two_node_cycle_symmetric_other_direction() {
        // Reverse the weights to ensure no positional bias: y->x heavy.
        let nodes = vec![0, 1];
        let edges = vec![
            WeightedEdge { from: 0, to: 1, weight: 2 },
            WeightedEdge { from: 1, to: 0, weight: 10 },
        ];
        let order = min_fas_order(&nodes, &edges);
        let be = back_edges(&order, &edges);
        assert_eq!(be.len(), 1);
        assert_eq!(be[0].from, 0, "must cut x->y (the lighter, weight 2)");
        assert_eq!(be[0].weight, 2);
    }

    #[test]
    fn three_node_cycle_cuts_one_light_edge() {
        // 0->1->2->0 cycle. Heavy forward path 0->1 (20), 1->2 (20); light
        // back 2->0 (1). The minimum FAS cuts only 2->0.
        let nodes = vec![0, 1, 2];
        let edges = vec![
            WeightedEdge { from: 0, to: 1, weight: 20 },
            WeightedEdge { from: 1, to: 2, weight: 20 },
            WeightedEdge { from: 2, to: 0, weight: 1 },
        ];
        let order = min_fas_order(&nodes, &edges);
        let be = back_edges(&order, &edges);
        assert_eq!(be.len(), 1, "one back-edge breaks the 3-cycle, got {be:?}");
        assert_eq!(be[0].from, 2);
        assert_eq!(be[0].to, 0);
        assert_eq!(be[0].weight, 1);
    }

    #[test]
    fn three_node_cycle_total_cut_weight_is_minimal() {
        // Two anti-parallel light edges vs heavy forward chain. The forward
        // chain 0->1->2 should be preserved; light back-edges cut.
        let nodes = vec![0, 1, 2];
        let edges = vec![
            WeightedEdge { from: 0, to: 1, weight: 50 },
            WeightedEdge { from: 1, to: 2, weight: 40 },
            WeightedEdge { from: 2, to: 1, weight: 3 },
            WeightedEdge { from: 1, to: 0, weight: 2 },
        ];
        let order = min_fas_order(&nodes, &edges);
        let be = back_edges(&order, &edges);
        let cut: u64 = be.iter().map(|e| e.weight).sum();
        // Best cut keeps the 50 and 40 forward edges -> cut the 3 and 2 (total 5).
        assert_eq!(cut, 5, "should cut only the light minority edges; got {be:?}");
    }

    #[test]
    fn order_is_permutation_of_nodes() {
        let nodes = vec![7, 3, 9, 1];
        let edges = vec![
            WeightedEdge { from: 7, to: 3, weight: 1 },
            WeightedEdge { from: 3, to: 9, weight: 1 },
            WeightedEdge { from: 9, to: 7, weight: 1 },
        ];
        let mut order = min_fas_order(&nodes, &edges);
        order.sort();
        let mut expected = nodes.clone();
        expected.sort();
        assert_eq!(order, expected);
    }
}
