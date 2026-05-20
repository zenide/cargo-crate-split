//! File path <-> Rust module path logic, and granularity truncation.

use std::path::{Path, PathBuf};

/// A fully-qualified Rust module path, e.g. `["ai", "assistant", "tools"]`.
/// An empty segment list represents the crate root.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModulePath {
    pub segments: Vec<String>,
}

impl ModulePath {
    pub fn crate_root() -> Self {
        ModulePath { segments: Vec::new() }
    }

    /// Append a child module segment (e.g. for an inline `mod x {}`).
    pub fn child(&self, seg: &str) -> Self {
        let mut segments = self.segments.clone();
        segments.push(seg.to_string());
        ModulePath { segments }
    }

    /// Truncate this path to at most `granularity` leading segments.
    /// `granularity` 1 = top-level module, 2 = two levels deep, etc.
    /// The crate root truncates to the crate root.
    pub fn truncate(&self, granularity: usize) -> ModulePath {
        let g = granularity.max(1);
        let segments: Vec<String> = self.segments.iter().take(g).cloned().collect();
        ModulePath { segments }
    }

    /// Human-readable label.
    pub fn label(&self) -> String {
        if self.segments.is_empty() {
            "crate".to_string()
        } else {
            self.segments.join("::")
        }
    }
}

/// Where a module's source comes from: the lib tree, or a separate binary root.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    /// Part of the primary lib/main module tree.
    Lib,
    /// A binary root (src/bin/<name>.rs or a [[bin]] entry).
    Bin(String),
}

/// A discovered source file mapped to its module path.
#[derive(Clone, Debug)]
pub struct DiscoveredFile {
    pub file: PathBuf,
    pub module: ModulePath,
    pub kind: ModuleKind,
    pub loc: usize,
}

/// Compute the module path of a `.rs` file relative to the crate's `src/` dir.
///
/// Rules:
/// - `src/lib.rs` / `src/main.rs` -> crate root.
/// - `src/foo.rs` or `src/foo/mod.rs` -> `foo`.
/// - `src/foo/bar.rs` -> `foo::bar`.
///
/// Files under `src/bin/` are handled by the caller as separate roots.
pub fn module_path_for_file(src_dir: &Path, file: &Path) -> Option<ModulePath> {
    let rel = file.strip_prefix(src_dir).ok()?;
    let mut segments: Vec<String> = Vec::new();

    let components: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => Some(os.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();

    if components.is_empty() {
        return None;
    }

    let last = components.last().unwrap();
    if !last.ends_with(".rs") {
        return None;
    }
    let stem = last.trim_end_matches(".rs").to_string();

    // Directory components before the file name become module segments.
    for dir in &components[..components.len() - 1] {
        segments.push(dir.clone());
    }

    match stem.as_str() {
        "lib" | "main" if segments.is_empty() => {
            // crate root
        }
        "mod" => {
            // src/foo/mod.rs -> foo (segments already hold ["foo"])
        }
        other => {
            segments.push(other.to_string());
        }
    }

    Some(ModulePath { segments })
}
