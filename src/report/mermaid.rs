//! Mermaid `graph TD` renderer for the crate dependency DAG.

use std::collections::HashMap;
use std::fmt::Write;

use crate::analyze::Analysis;

/// Render a Mermaid `graph TD` block (no surrounding fences).
pub fn render(a: &Analysis) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "graph TD");

    // Stable node ids.
    let mut id_of: HashMap<String, String> = HashMap::new();
    for (i, p) in a.proposals.iter().enumerate() {
        id_of.insert(p.crate_name.clone(), format!("n{i}"));
    }

    // Declare nodes with labels (name + LOC).
    for p in &a.proposals {
        let id = &id_of[&p.crate_name];
        let _ = writeln!(s, "    {}[\"{}<br/>{} LOC\"]", id, p.crate_name, p.loc);
    }

    // Edges.
    for p in &a.proposals {
        let from = &id_of[&p.crate_name];
        for dep in &p.depends_on {
            if let Some(to) = id_of.get(dep) {
                let _ = writeln!(s, "    {from} --> {to}");
            }
        }
    }

    s
}
