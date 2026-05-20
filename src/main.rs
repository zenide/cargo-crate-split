mod analyze;
mod classify;
mod cli;
mod discover;
mod fas;
mod frontier;
mod graph;
mod module_path;
mod parse;
mod report;
mod respect_order;
mod scaffold;

fn main() -> anyhow::Result<()> {
    cli::run()
}
