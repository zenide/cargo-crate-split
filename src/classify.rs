//! Classify a recommended cut and suggest the refactor mechanic to break it.
//!
//! The feedback-arc-set picks *which* edges to cut; this module answers the next
//! question an engineer (or coding agent) actually asks: **how do I break this
//! one, and how hard is it?** Without name resolution we lean on Rust naming
//! conventions — `UpperCamelCase` leaf = a type/trait, `snake_case` leaf = a
//! value/fn/module, `*` = a glob re-export — to classify each cut and emit a
//! concrete, imperative instruction plus a difficulty estimate.
//!
//! Occurrence count alone is a poor effort proxy (issue #4): three references to
//! one type in one file is a trivial move; three scattered call sites needing
//! dependency inversion is not. We weight by distinct files, distinct symbols,
//! and the cut kind instead.

use serde::Serialize;

use crate::analyze::OccurrenceOut;

/// What the cut edge is made of, inferred from the referenced leaf symbols.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CutKind {
    /// Every referenced leaf is a type/trait/enum (`UpperCamelCase`). The classic
    /// cheap cut: move the shared type down into a foundation crate.
    MoveType,
    /// Referenced leaves are values — functions, consts, or bare modules. The
    /// foundation module is *calling up* into a higher-level one; usually needs
    /// dependency inversion.
    FunctionCall,
    /// At least one glob re-export (`use crate::x::*`). Opaque to a path analyzer
    /// — the real coupling must be read by hand.
    Glob,
    /// A mix of types and values.
    Mixed,
}

impl CutKind {
    pub fn label(self) -> &'static str {
        match self {
            CutKind::MoveType => "move-type-to-foundation",
            CutKind::FunctionCall => "dependency-inversion",
            CutKind::Glob => "inspect-glob-reexport",
            CutKind::Mixed => "extract-shared",
        }
    }
}

/// Rough refactor effort, derived from kind + distinct files + distinct symbols.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Difficulty {
    /// One small mechanical move (e.g. relocate one type, one file touched).
    Trivial,
    /// A handful of edits, possibly across a few files.
    Moderate,
    /// Many sites, a glob, or behavioural inversion — read the code first.
    Hard,
}

impl Difficulty {
    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Trivial => "trivial",
            Difficulty::Moderate => "moderate",
            Difficulty::Hard => "hard",
        }
    }
}

/// The classification of a single cut, attached to each `BackEdge`.
#[derive(Clone, Debug, Serialize)]
pub struct CutClassification {
    pub kind: CutKind,
    pub difficulty: Difficulty,
    /// Number of distinct source files the cut's references span.
    pub distinct_files: usize,
    /// Distinct leaf symbols referenced (sorted, deduped).
    pub distinct_symbols: Vec<String>,
    /// Concrete, imperative instruction for breaking the cut.
    pub suggestion: String,
}

/// Is a leaf symbol type-like (`UpperCamelCase`): starts uppercase AND contains a
/// lowercase letter? This separates `CriticOutcome` (a type) from `MAX_LEN` (a
/// SCREAMING_SNAKE const, which is value-like).
fn is_type_like(sym: &str) -> bool {
    let mut chars = sym.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => sym.chars().any(|c| c.is_ascii_lowercase()),
        _ => false,
    }
}

/// The trailing `::` segment of a reference path, e.g.
/// `crate::ai::coach::CriticOutcome` -> `CriticOutcome`.
fn leaf_symbol(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path).trim()
}

/// Render up to `n` symbols as a backtick list, with a "+k more" tail.
fn symbol_list(symbols: &[String], n: usize) -> String {
    if symbols.is_empty() {
        return "the referenced item(s)".to_string();
    }
    let shown: Vec<String> = symbols.iter().take(n).map(|s| format!("`{s}`")).collect();
    let mut out = shown.join(", ");
    if symbols.len() > n {
        out.push_str(&format!(" (+{} more)", symbols.len() - n));
    }
    out
}

/// Classify a cut from its occurrences and the two module labels.
///
/// `source` depends "upward" on `target` (target sits higher in the intended
/// foundation→top order). Breaking the cut means severing that upward edge.
pub fn classify(source: &str, target: &str, occs: &[OccurrenceOut]) -> CutClassification {
    let mut files: Vec<&str> = occs.iter().map(|o| o.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let distinct_files = files.len();

    let mut symbols: Vec<String> = occs.iter().map(|o| leaf_symbol(&o.path).to_string()).collect();
    symbols.sort();
    symbols.dedup();

    let has_glob = symbols.iter().any(|s| s == "*");
    let type_syms: Vec<&String> = symbols.iter().filter(|s| is_type_like(s)).collect();
    let value_syms: Vec<&String> = symbols
        .iter()
        .filter(|s| !is_type_like(s) && s.as_str() != "*")
        .collect();

    let kind = if has_glob {
        CutKind::Glob
    } else if !type_syms.is_empty() && value_syms.is_empty() {
        CutKind::MoveType
    } else if type_syms.is_empty() && !value_syms.is_empty() {
        CutKind::FunctionCall
    } else {
        CutKind::Mixed
    };

    let penalty = match kind {
        CutKind::MoveType => 0,
        CutKind::FunctionCall | CutKind::Mixed => 2,
        CutKind::Glob => 4,
    };
    let score = distinct_files + symbols.len() + penalty;
    let difficulty = if kind == CutKind::Glob {
        Difficulty::Hard
    } else if kind == CutKind::MoveType && distinct_files <= 2 && symbols.len() <= 3 {
        Difficulty::Trivial
    } else if score >= 8 {
        Difficulty::Hard
    } else {
        Difficulty::Moderate
    };

    let suggestion = match kind {
        CutKind::MoveType => format!(
            "Move {} out of `{target}` into a shared foundation crate (e.g. a `*-types` \
             crate that both `{source}` and `{target}` depend on). The upward edge then \
             disappears — both sides import the type from below.",
            symbol_list(&type_syms.iter().map(|s| (*s).clone()).collect::<Vec<_>>(), 4)
        ),
        CutKind::FunctionCall => format!(
            "Invert the dependency: `{source}` is calling up into `{target}` via {}. Define a \
             trait (or fn pointer / callback) in `{source}` or a foundation crate that captures \
             what it needs, have `{target}` implement and inject it. `{source}` must not name \
             `{target}` directly.",
            symbol_list(&value_syms.iter().map(|s| (*s).clone()).collect::<Vec<_>>(), 4)
        ),
        CutKind::Glob => format!(
            "`{source}` glob-imports from `{target}` (`use crate::{target}::*`). Replace the glob \
             with explicit imports first to see the true coupling, then move shared types down or \
             invert the call edge as appropriate.",
        ),
        CutKind::Mixed => format!(
            "Mixed coupling — types {} and values {}. Move the shared types into a foundation \
             crate; for the remaining calls, invert via a trait in the lower crate.",
            symbol_list(&type_syms.iter().map(|s| (*s).clone()).collect::<Vec<_>>(), 3),
            symbol_list(&value_syms.iter().map(|s| (*s).clone()).collect::<Vec<_>>(), 3),
        ),
    };

    CutClassification {
        kind,
        difficulty,
        distinct_files,
        distinct_symbols: symbols,
        suggestion,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occ(file: &str, path: &str) -> OccurrenceOut {
        OccurrenceOut {
            file: file.to_string(),
            line: 1,
            path: path.to_string(),
        }
    }

    #[test]
    fn type_only_single_file_is_trivial_move() {
        let occs = vec![
            occ("a.rs", "crate::ai::critic::CriticOutcome"),
            occ("a.rs", "crate::ai::critic::Verdict"),
        ];
        let c = classify("queries", "ai", &occs);
        assert_eq!(c.kind, CutKind::MoveType);
        assert_eq!(c.difficulty, Difficulty::Trivial);
        assert_eq!(c.distinct_files, 1);
        assert_eq!(c.distinct_symbols, vec!["CriticOutcome", "Verdict"]);
        assert!(c.suggestion.contains("foundation crate"));
    }

    #[test]
    fn lowercase_leaf_is_function_call_inversion() {
        let occs = vec![occ("a.rs", "crate::ai::assistant::run")];
        let c = classify("queries", "ai", &occs);
        assert_eq!(c.kind, CutKind::FunctionCall);
        assert!(c.suggestion.contains("Invert"));
    }

    #[test]
    fn screaming_snake_const_is_value_not_type() {
        assert!(!is_type_like("MAX_LEN"));
        assert!(is_type_like("CriticOutcome"));
        assert!(!is_type_like("run_query"));
    }

    #[test]
    fn glob_is_hard() {
        let occs = vec![occ("a.rs", "crate::ai::*")];
        let c = classify("queries", "ai", &occs);
        assert_eq!(c.kind, CutKind::Glob);
        assert_eq!(c.difficulty, Difficulty::Hard);
    }

    #[test]
    fn many_scattered_types_escalate_past_trivial() {
        let occs: Vec<OccurrenceOut> = (0..6)
            .map(|i| occ(&format!("f{i}.rs"), "crate::ai::critic::CriticOutcome"))
            .collect();
        let c = classify("queries", "ai", &occs);
        assert_eq!(c.kind, CutKind::MoveType);
        assert_ne!(c.difficulty, Difficulty::Trivial, "6 files is not trivial");
    }
}
