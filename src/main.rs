use anyhow::Result;
use clap::Parser;

use agentstack::cli::{Cli, Command};
use agentstack::commands;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let dir = cli.manifest_dir.as_deref();
    match &cli.command {
        Command::Init(args) => commands::init::run(args, dir),
        Command::Add(args) => commands::add::run(args, dir),
        Command::Install(args) => commands::install::run(args, dir),
        Command::Update(args) => commands::install::run_update(args, dir),
        Command::Remove(args) => commands::remove::run(args, dir),
        Command::Upgrade(args) => commands::upgrade::run(args, dir),
        Command::Bootstrap(args) => commands::bootstrap::run(args, dir),
        Command::Apply(args) => commands::apply::run(args, dir),
        Command::Diff(args) => commands::diff::run(args, dir),
        Command::Explain(args) => commands::explain::run(args, dir),
        Command::Use(args) => commands::use_profile::run(args, dir),
        Command::Session(args) => commands::session::run(args, dir),
        Command::Instructions(args) => commands::instructions::run(args, dir),
        Command::Adopt(args) => commands::adopt::run(args, dir),
        Command::Consolidate(args) => commands::consolidate::run(args, dir),
        Command::Restore(args) => commands::restore::run(args, dir),
        Command::Doctor(args) => commands::doctor::run(args, dir),
        Command::Search(args) => commands::search::run(args, dir),
        Command::Stats => commands::stats::run(dir),
        Command::Adapters(args) => commands::adapters::run(args),
        Command::Plugins(args) => commands::plugins::run(args, dir),
        Command::Secret(args) => commands::secret::run(args, dir),
        Command::Export(args) => commands::bundle::run_export(args, dir),
        Command::Import(args) => commands::bundle::run_import(args, dir),
        Command::Dashboard(args) => agentstack::dashboard::serve(args, dir),
        Command::Mcp => agentstack::mcp_server::serve(dir),
        Command::Codemode(args) => commands::codemode::run(args, dir),
        Command::Hook(args) => commands::hook::run(args),
        Command::Run(args) => commands::runs::run(args, dir),
        Command::Runs(args) => commands::runs::list(args),
        Command::Kill(args) => commands::runs::kill(args),
    }
}
