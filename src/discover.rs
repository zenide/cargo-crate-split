//! Walk the target crate's `src/` and map each `.rs` file to its module path.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::module_path::{module_path_for_file, DiscoveredFile, ModuleKind, ModulePath};

/// Metadata about the analyzed crate.
#[derive(Clone, Debug)]
pub struct CrateInfo {
    /// Cargo package name, used to derive crate-name prefixes.
    pub package_name: String,
}

/// Result of discovery: the crate metadata + every discovered file.
#[derive(Clone, Debug)]
pub struct Discovery {
    pub info: CrateInfo,
    pub files: Vec<DiscoveredFile>,
}

fn count_loc(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// Parse the package name out of a crate's `Cargo.toml` without a TOML dep.
fn read_package_name(crate_root: &Path) -> Option<String> {
    let manifest = crate_root.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("name") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let val = rest.trim().trim_matches('"').trim_matches('\'');
                    if !val.is_empty() {
                        return Some(val.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Discover every source file in the target crate.
pub fn discover(crate_root: &Path) -> Result<Discovery> {
    let crate_root = crate_root
        .canonicalize()
        .with_context(|| format!("crate path does not exist: {}", crate_root.display()))?;

    let src_dir = crate_root.join("src");
    anyhow::ensure!(
        src_dir.is_dir(),
        "no src/ directory found at {}",
        src_dir.display()
    );

    let package_name = read_package_name(&crate_root)
        .unwrap_or_else(|| {
            crate_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "crate".to_string())
        });

    let bin_dir = src_dir.join("bin");
    let mut files: Vec<DiscoveredFile> = Vec::new();

    for entry in WalkDir::new(&src_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }

        let loc = count_loc(path);

        // Binary roots: src/bin/<name>.rs (and nested files under src/bin/).
        if path.starts_with(&bin_dir) {
            let rel = path.strip_prefix(&bin_dir).unwrap();
            // Top-level src/bin/<name>.rs is its own bin root.
            let bin_name = rel
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_else(|| "bin".to_string());
            let bin_name = bin_name.trim_end_matches(".rs").to_string();

            // The module path inside a bin file tree, relative to src/bin.
            let module = module_path_for_file(&bin_dir, path)
                .unwrap_or_else(ModulePath::crate_root);
            files.push(DiscoveredFile {
                file: path.to_path_buf(),
                module,
                kind: ModuleKind::Bin(bin_name),
                loc,
            });
            continue;
        }

        let Some(module) = module_path_for_file(&src_dir, path) else {
            continue;
        };

        files.push(DiscoveredFile {
            file: path.to_path_buf(),
            module,
            kind: ModuleKind::Lib,
            loc,
        });
    }

    Ok(Discovery {
        info: CrateInfo { package_name },
        files,
    })
}
