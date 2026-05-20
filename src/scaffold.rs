//! Generate an empty workspace skeleton implementing the proposed split.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::analyze::Analysis;

/// Generate the workspace skeleton at `out_dir`. Returns a list of created
/// paths (for printing). Does NOT move any source code.
pub fn scaffold(a: &Analysis, out_dir: &Path) -> Result<Vec<String>> {
    let mut created: Vec<String> = Vec::new();

    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating {}", out_dir.display()))?;

    let crates_dir = out_dir.join("crates");
    fs::create_dir_all(&crates_dir)?;

    // Root workspace Cargo.toml.
    let mut members: Vec<String> = a
        .proposals
        .iter()
        .map(|p| format!("crates/{}", p.crate_name))
        .collect();
    members.sort();

    let mut root = String::from("[workspace]\nresolver = \"2\"\nmembers = [\n");
    for m in &members {
        root.push_str(&format!("    \"{m}\",\n"));
    }
    root.push_str("]\n");

    let root_path = out_dir.join("Cargo.toml");
    fs::write(&root_path, root)?;
    created.push(root_path.display().to_string());

    // Per-crate dirs.
    for p in &a.proposals {
        let dir = crates_dir.join(&p.crate_name);
        let src = dir.join("src");
        fs::create_dir_all(&src)?;

        // Stub Cargo.toml with inter-crate dependencies.
        let mut toml = String::new();
        toml.push_str("[package]\n");
        toml.push_str(&format!("name = \"{}\"\n", p.crate_name));
        toml.push_str("version = \"0.1.0\"\n");
        toml.push_str("edition = \"2021\"\n\n");
        toml.push_str("[dependencies]\n");
        for dep in &p.depends_on {
            // Path dependency to the sibling crate.
            toml.push_str(&format!(
                "{dep} = {{ path = \"../{dep}\" }}\n"
            ));
        }

        let toml_path = dir.join("Cargo.toml");
        fs::write(&toml_path, toml)?;
        created.push(toml_path.display().to_string());

        // Empty lib.rs with a header comment listing the modules to move here.
        let mut lib = String::new();
        lib.push_str(&format!(
            "// crate `{}` — proposed by cargo-crate-split\n",
            p.crate_name
        ));
        lib.push_str("// Move the following modules from the original crate here:\n");
        for m in &p.modules {
            lib.push_str(&format!("//   - {m}\n"));
        }
        lib.push('\n');

        let lib_path = src.join("lib.rs");
        fs::write(&lib_path, lib)?;
        created.push(lib_path.display().to_string());
    }

    Ok(created)
}
