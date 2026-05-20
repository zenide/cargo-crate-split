//! Human-readable text report.

use std::fmt::Write;

use crate::analyze::Analysis;
use crate::report::mermaid;

/// Render the full text report. If `include_mermaid`, append a Mermaid block.
pub fn render(a: &Analysis, include_mermaid: bool) -> String {
    let mut s = String::new();

    let _ = writeln!(
        s,
        "cargo-crate-split — analysis of `{}` (granularity {})",
        a.package_name, a.granularity
    );
    let _ = writeln!(s, "{}", "=".repeat(64));
    let _ = writeln!(s);

    // Proposed crates grouped by layer (foundation first = lowest level).
    let _ = writeln!(s, "PROPOSED CRATES (foundation first)");
    let _ = writeln!(s, "{}", "-".repeat(64));
    for layer in &a.layers {
        let tag = if layer.level == 0 {
            "layer 0 (foundation / leaf crates)".to_string()
        } else {
            format!("layer {}", layer.level)
        };
        let _ = writeln!(s, "\n  [{tag}]");
        for crate_name in &layer.crates {
            let Some(p) = a.proposals.iter().find(|p| &p.crate_name == crate_name) else {
                continue;
            };
            let cyc = if p.is_cycle { "  (CYCLE)" } else { "" };
            let _ = writeln!(
                s,
                "    • {}  —  {} LOC, {} file(s){}",
                p.crate_name, p.loc, p.file_count, cyc
            );
            let _ = writeln!(s, "        modules: {}", p.modules.join(", "));
            if !p.depends_on.is_empty() {
                let _ = writeln!(s, "        depends on: {}", p.depends_on.join(", "));
            }
        }
    }
    let _ = writeln!(s);

    // Crate dependency DAG (text adjacency).
    let _ = writeln!(s, "CRATE DEPENDENCY DAG");
    let _ = writeln!(s, "{}", "-".repeat(64));
    let mut any_edge = false;
    for p in &a.proposals {
        if p.depends_on.is_empty() {
            continue;
        }
        any_edge = true;
        let _ = writeln!(s, "  {} -> {}", p.crate_name, p.depends_on.join(", "));
    }
    if !any_edge {
        let _ = writeln!(s, "  (no inter-crate dependencies — all crates independent)");
    }
    let _ = writeln!(s);

    // Cycles & cycle-breaking edits.
    let _ = writeln!(s, "CYCLES & CYCLE-BREAKING EDITS");
    let _ = writeln!(s, "{}", "-".repeat(64));
    if a.back_edges.is_empty() {
        let _ = writeln!(
            s,
            "  No multi-module cycles. The proposed split is already a DAG."
        );
    } else {
        let _ = writeln!(
            s,
            "  {} back-edge(s) to remove (cheapest first):",
            a.back_edges.len()
        );
        for be in &a.back_edges {
            let _ = writeln!(s);
            let _ = writeln!(
                s,
                "  ✂ cut  {} -> {}  ({} occurrence(s)) [SCC #{}]",
                be.source_module, be.target_module, be.occurrence_count, be.scc
            );
            for occ in &be.occurrences {
                let _ = writeln!(s, "        {}:{}  {}", occ.file, occ.line, occ.path);
            }
        }
    }
    let _ = writeln!(s);

    // Summary.
    let _ = writeln!(s, "SUMMARY");
    let _ = writeln!(s, "{}", "-".repeat(64));
    let _ = writeln!(
        s,
        "  {} module(s) -> {} proposed crate(s)",
        a.summary.module_count, a.summary.proposed_crate_count
    );
    let _ = writeln!(s, "  largest crate: {} LOC", a.summary.largest_crate_loc);
    let _ = writeln!(s, "  cycles found: {}", a.summary.cycle_count);

    if include_mermaid {
        let _ = writeln!(s);
        let _ = writeln!(s, "MERMAID");
        let _ = writeln!(s, "{}", "-".repeat(64));
        s.push_str(&mermaid::render(a));
    }

    s
}
