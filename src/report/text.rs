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
    // The proposal is the decomposition ACHIEVABLE after applying the cheap
    // cuts below — i.e. the SCCs of `G − applied_cuts`, not raw G.
    let _ = writeln!(
        s,
        "PROPOSED SPLIT — achievable after {} cut(s) ({} reference(s))",
        a.applied_cut_count, a.applied_refs_cut
    );
    if a.applied_cut_count < a.summary.recommended_cut_count {
        let _ = writeln!(
            s,
            "  (budget-limited: {} of {} recommended cuts applied)",
            a.applied_cut_count, a.summary.recommended_cut_count
        );
    }
    let _ = writeln!(s, "  (foundation first; ★ = unlocked by the applied cuts)");
    let _ = writeln!(s, "{}", "-".repeat(64));
    let freed: std::collections::HashSet<&str> =
        a.freed_modules.iter().map(|s| s.as_str()).collect();
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
            let cyc = if p.is_cycle { "  (CYCLE REMAINS)" } else { "" };
            let star = if p.modules.len() == 1 && freed.contains(p.modules[0].as_str()) {
                " ★"
            } else {
                ""
            };
            let _ = writeln!(
                s,
                "    • {}{}  —  {} LOC, {} file(s){}",
                p.crate_name, star, p.loc, p.file_count, cyc
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

    // Required cuts — the prerequisites (the price) of the proposed split.
    let _ = writeln!(s, "REQUIRED CUTS (the price of the proposed split)");
    let _ = writeln!(s, "{}", "-".repeat(64));
    if a.back_edges.is_empty() {
        let _ = writeln!(
            s,
            "  No multi-module cycles. The raw graph already splits as a DAG."
        );
    } else {
        let _ = writeln!(
            s,
            "  {} recommended cut(s), cheapest first ({} applied for the proposal above):",
            a.back_edges.len(),
            a.applied_cut_count
        );
        for (i, be) in a.back_edges.iter().enumerate() {
            let mark = if i < a.applied_cut_count { "[applied]" } else { "[skipped]" };
            let _ = writeln!(s);
            let _ = writeln!(
                s,
                "  ✂ {mark} cut  {} -> {}  ({} occurrence(s)) [SCC #{}]",
                be.source_module, be.target_module, be.occurrence_count, be.scc
            );
            for occ in &be.occurrences {
                let _ = writeln!(s, "        {}:{}  {}", occ.file, occ.line, occ.path);
            }
        }
    }
    let _ = writeln!(s);

    // Effort/payoff frontier.
    let _ = writeln!(s, "EFFORT / PAYOFF FRONTIER");
    let _ = writeln!(s, "{}", "-".repeat(64));
    let _ = writeln!(
        s,
        "  Each row = decomposition after the k cheapest cuts. Largest-crate LOC"
    );
    let _ = writeln!(
        s,
        "  drops in STEPS at unlock points (a key edge removed), not smoothly."
    );
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "  {:>5}  {:>9}  {:>7}  {:>12}   modules newly freed",
        "cuts", "refs cut", "#crates", "largest LOC"
    );
    let _ = writeln!(s, "  {}", "-".repeat(60));
    for step in &a.frontier {
        let freed = if step.freed_modules.is_empty() {
            "—".to_string()
        } else {
            step.freed_modules.join(", ")
        };
        let _ = writeln!(
            s,
            "  {:>5}  {:>9}  {:>7}  {:>12}   {}",
            step.cuts_applied, step.refs_cut, step.crate_count, step.largest_loc, freed
        );
    }
    let _ = writeln!(s);

    // Summary.
    let _ = writeln!(s, "SUMMARY");
    let _ = writeln!(s, "{}", "-".repeat(64));
    let _ = writeln!(
        s,
        "  {} module(s) -> {} proposed crate(s) (after {} of {} recommended cuts)",
        a.summary.module_count,
        a.summary.proposed_crate_count,
        a.applied_cut_count,
        a.summary.recommended_cut_count
    );
    let _ = writeln!(s, "  largest crate: {} LOC", a.summary.largest_crate_loc);
    let _ = writeln!(s, "  cycles remaining (after applied cuts): {}", a.summary.cycle_count);
    if !a.freed_modules.is_empty() {
        let _ = writeln!(
            s,
            "  modules unlocked into their own crate: {}",
            a.freed_modules.join(", ")
        );
    }

    if include_mermaid {
        let _ = writeln!(s);
        let _ = writeln!(s, "MERMAID");
        let _ = writeln!(s, "{}", "-".repeat(64));
        s.push_str(&mermaid::render(a));
    }

    s
}
