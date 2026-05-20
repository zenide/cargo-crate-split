//! Parse a user-supplied `--respect-order` SPEC into an ordered module list.
//!
//! SPEC is a module order from foundation to top, separated by `,` or `<`
//! (whitespace around separators is ignored), e.g.:
//!
//! ```text
//! models,queries,db,ai,handlers,app
//! models < queries < db < ai < handlers < app
//! ```
//!
//! Earlier modules are more foundational; later modules sit "higher" and may
//! depend downward on earlier ones. An edge `u -> v` *violates* the pinned
//! order when `v` is later than `u` (a module depending upward).

use anyhow::{bail, Result};

/// Parse the SPEC string into an ordered list of module labels (foundation
/// first). Empty entries are dropped; duplicates are rejected.
pub fn parse_respect_order(spec: &str) -> Result<Vec<String>> {
    let modules: Vec<String> = spec
        .split([',', '<'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    if modules.is_empty() {
        bail!("--respect-order SPEC is empty; expected e.g. \"models,queries,handlers\"");
    }

    let mut seen = std::collections::HashSet::new();
    for m in &modules {
        if !seen.insert(m.clone()) {
            bail!("--respect-order lists module `{m}` more than once");
        }
    }

    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comma_separated() {
        let got = parse_respect_order("models,queries,handlers").unwrap();
        assert_eq!(got, vec!["models", "queries", "handlers"]);
    }

    #[test]
    fn angle_separated_with_spaces() {
        let got = parse_respect_order("models < queries < handlers").unwrap();
        assert_eq!(got, vec!["models", "queries", "handlers"]);
    }

    #[test]
    fn mixed_and_trimmed() {
        let got = parse_respect_order(" models, queries < db ,, ai ").unwrap();
        assert_eq!(got, vec!["models", "queries", "db", "ai"]);
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_respect_order("   ").is_err());
        assert!(parse_respect_order(",,").is_err());
    }

    #[test]
    fn rejects_duplicates() {
        assert!(parse_respect_order("models,queries,models").is_err());
    }
}
