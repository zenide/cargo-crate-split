//! Clap derive CLI + dispatch, including cargo-subcommand argv handling.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::analyze::analyze;
use crate::discover::discover;
use crate::graph::build_graph;
use crate::report::{json, text};
use crate::respect_order::parse_respect_order;
use crate::scaffold::scaffold;

/// Shared `--help` text documenting the `--respect-order` SPEC format.
const RESPECT_ORDER_HELP: &str = "\
Pinned module order from foundation to top, separated by `,` or `<` (e.g. \
\"models,queries,db,ai,handlers,app\" or \"models < queries < handlers\"). \
When set, back-edges are the edges that VIOLATE this intended order (a module \
depending \"upward\"), instead of the computed ELS feedback-arc-set. Modules \
not named keep their computed position, after the pinned ones they depend on.";

#[derive(Parser, Debug)]
#[command(
    name = "cargo-crate-split",
    bin_name = "cargo-crate-split",
    about = "Analyze a Rust crate's module graph and propose a provably-acyclic split.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Analyze a crate and print a split proposal.
    Analyze {
        /// Path to the crate to analyze (the dir containing Cargo.toml).
        path: PathBuf,
        /// Module granularity: 1 = top-level modules, 2 = two levels deep, etc.
        #[arg(long, default_value_t = 1)]
        granularity: usize,
        /// Append a Mermaid `graph TD` block to the text report.
        #[arg(long)]
        mermaid: bool,
        /// Emit the analysis as JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Pinned module order from foundation to top (`,` or `<` separated).
        #[arg(long, value_name = "SPEC", long_help = RESPECT_ORDER_HELP)]
        respect_order: Option<String>,
    },
    /// Generate an empty workspace skeleton for the proposed split.
    Scaffold {
        /// Path to the crate to analyze.
        path: PathBuf,
        /// Output directory for the generated workspace skeleton.
        #[arg(long)]
        out: PathBuf,
        /// Module granularity.
        #[arg(long, default_value_t = 1)]
        granularity: usize,
        /// Pinned module order from foundation to top (`,` or `<` separated).
        #[arg(long, value_name = "SPEC", long_help = RESPECT_ORDER_HELP)]
        respect_order: Option<String>,
    },
}

/// Parse args (handling the `cargo crate-split ...` invocation) and dispatch.
pub fn run() -> Result<()> {
    let cli = Cli::parse_from(normalized_args());
    match cli.command {
        Command::Analyze {
            path,
            granularity,
            mermaid,
            json: as_json,
            respect_order,
        } => {
            let pinned = respect_order.as_deref().map(parse_respect_order).transpose()?;
            let disc = discover(&path)?;
            let mg = build_graph(&disc, granularity)?;
            let analysis = analyze(&mg, &disc.info.package_name, pinned.as_deref());
            if as_json {
                println!("{}", json::render(&analysis)?);
            } else {
                print!("{}", text::render(&analysis, mermaid));
            }
        }
        Command::Scaffold {
            path,
            out,
            granularity,
            respect_order,
        } => {
            let pinned = respect_order.as_deref().map(parse_respect_order).transpose()?;
            let disc = discover(&path)?;
            let mg = build_graph(&disc, granularity)?;
            let analysis = analyze(&mg, &disc.info.package_name, pinned.as_deref());
            let created = scaffold(&analysis, &out)?;
            println!(
                "Scaffolded workspace at {} ({} crate(s)):",
                out.display(),
                analysis.proposals.len()
            );
            for path in &created {
                println!("  created {path}");
            }
            println!(
                "\nNOTE: no source was moved. Move modules into each crate's src/ per the lib.rs headers."
            );
        }
    }
    Ok(())
}

/// When invoked as `cargo crate-split ...`, argv[1] is `crate-split`.
/// Strip it so clap sees a normal argument vector.
fn normalized_args() -> Vec<String> {
    let mut args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "crate-split" {
        args.remove(1);
    }
    args
}
