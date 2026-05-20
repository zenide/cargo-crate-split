//! Parse a single `.rs` file with `syn` and emit `crate::A::...` references as edges.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use syn::visit::{self, Visit};

use crate::module_path::ModulePath;

/// A single observed reference to `crate::<target>::...` in a source file.
#[derive(Clone, Debug)]
pub struct Reference {
    /// The module that contains the reference (the enclosing module path,
    /// accounting for inline `mod x {}` nesting).
    pub source: ModulePath,
    /// The first segment after `crate::` — the referenced top-level module.
    pub target_top: String,
    /// File the reference appears in.
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The full path string as written, e.g. `crate::ai::assistant::run`.
    pub path: String,
}

/// Parse one file and return all `crate::...` references found in it.
///
/// `base_module` is the module path of the file itself (from discovery).
/// Inline `mod x { ... }` blocks extend the enclosing module path.
pub fn references_in_file(file: &Path, base_module: &ModulePath) -> Result<Vec<Reference>> {
    let src = std::fs::read_to_string(file)
        .with_context(|| format!("reading {}", file.display()))?;
    let ast = syn::parse_file(&src)
        .with_context(|| format!("parsing {}", file.display()))?;

    let mut visitor = RefVisitor {
        file: file.to_path_buf(),
        module_stack: vec![base_module.clone()],
        refs: Vec::new(),
    };
    visitor.visit_file(&ast);
    Ok(visitor.refs)
}

struct RefVisitor {
    file: PathBuf,
    /// Stack of module paths; top is the current enclosing module.
    module_stack: Vec<ModulePath>,
    refs: Vec<Reference>,
}

impl RefVisitor {
    fn current_module(&self) -> &ModulePath {
        self.module_stack.last().expect("non-empty module stack")
    }

    /// Record a reference if `path` begins with `crate::<seg>`.
    fn record_path(&mut self, path: &syn::Path) {
        let mut segs = path.segments.iter();
        let Some(first) = segs.next() else {
            return;
        };
        if first.ident != "crate" {
            return;
        }
        let Some(second) = segs.next() else {
            // Just `crate` with nothing after — no target module.
            return;
        };
        let target_top = second.ident.to_string();

        let line = path
            .segments
            .first()
            .map(|s| s.ident.span().start().line)
            .unwrap_or(0);

        let full = path_to_string(path);

        self.refs.push(Reference {
            source: self.current_module().clone(),
            target_top,
            file: self.file.clone(),
            line,
            path: full,
        });
    }

    /// Record the leading `crate::<seg>` of a `use` tree, expanding groups.
    fn record_use_tree(&mut self, tree: &syn::UseTree, prefix: Vec<String>) {
        match tree {
            syn::UseTree::Path(p) => {
                let mut new_prefix = prefix;
                new_prefix.push(p.ident.to_string());
                self.record_use_tree(&p.tree, new_prefix);
            }
            syn::UseTree::Name(n) => {
                let mut full = prefix.clone();
                full.push(n.ident.to_string());
                self.emit_use(&full, n.ident.span().start().line);
            }
            syn::UseTree::Rename(r) => {
                let mut full = prefix.clone();
                full.push(r.ident.to_string());
                self.emit_use(&full, r.ident.span().start().line);
            }
            syn::UseTree::Glob(g) => {
                let mut full = prefix.clone();
                full.push("*".to_string());
                self.emit_use(&full, g.star_token.spans[0].start().line);
            }
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.record_use_tree(item, prefix.clone());
                }
            }
        }
    }

    /// `segments` is the fully-expanded leading path of a `use` item.
    fn emit_use(&mut self, segments: &[String], line: usize) {
        if segments.first().map(|s| s.as_str()) != Some("crate") {
            return;
        }
        let Some(target_top) = segments.get(1) else {
            return;
        };
        let full = segments.join("::");
        self.refs.push(Reference {
            source: self.current_module().clone(),
            target_top: target_top.clone(),
            file: self.file.clone(),
            line,
            path: full,
        });
    }
}

impl<'ast> Visit<'ast> for RefVisitor {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        // Only inline `mod x { ... }` extends the path; `mod x;` has no content.
        if node.content.is_some() {
            let child = self.current_module().child(&node.ident.to_string());
            self.module_stack.push(child);
            visit::visit_item_mod(self, node);
            self.module_stack.pop();
        } else {
            visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.record_use_tree(&node.tree, Vec::new());
        // Do not descend further; use trees aren't `Path` nodes.
        visit::visit_item_use(self, node);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        self.record_path(node);
        visit::visit_path(self, node);
    }
}

fn path_to_string(path: &syn::Path) -> String {
    let segs: Vec<String> = path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    let mut out = segs.join("::");
    if path.leading_colon.is_some() {
        out = format!("::{out}");
    }
    out
}
