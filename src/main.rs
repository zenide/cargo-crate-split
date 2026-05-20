mod analyze;
mod cli;
mod discover;
mod graph;
mod module_path;
mod parse;
mod report;
mod scaffold;

fn main() -> anyhow::Result<()> {
    cli::run()
}
