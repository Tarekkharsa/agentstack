#![deny(unsafe_code)]

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
    // Bare `agentstack`: a short orientation, not the 30-command help dump.
    let Some(command) = &cli.command else {
        return commands::overview::run(dir);
    };
    match command {
        Command::Init(args) => commands::init::run(args, dir),
        Command::Add(args) => commands::add::run(args, dir),
        Command::Install(args) => commands::install::run(args, dir),
        Command::Update(args) => commands::install::run_update(args, dir),
        Command::Lock(args) => commands::lock::run(args, dir),
        Command::Remove(args) => commands::remove::run(args, dir),
        Command::Upgrade(args) => commands::upgrade::run(args, dir),
        Command::Bootstrap(args) => commands::bootstrap::run(args, dir),
        Command::Setup(args) => commands::setup::run(args, dir),
        Command::Apply(args) => commands::apply::run(args, dir),
        Command::Diff(args) => commands::diff::run(args, dir),
        Command::Explain(args) => commands::explain::run(args, dir),
        Command::Use(args) => commands::use_profile::run(args, dir),
        Command::Session(args) => commands::session::run(args, dir),
        Command::Instructions(args) => commands::instructions::run(args, dir),
        Command::Adopt(args) => commands::adopt::run(args, dir),
        Command::Consolidate(args) => commands::consolidate::run(args, dir),
        Command::Lib(args) => commands::lib::run(args, dir),
        Command::Restore(args) => commands::restore::run(args, dir),
        Command::Doctor(args) => commands::doctor::run(args, dir),
        Command::Audit(args) => commands::audit::run(args, dir),
        Command::Search(args) => commands::search::run(args, dir),
        Command::Stats(args) => commands::stats::run(args, dir),
        Command::Analyze(args) => commands::analyze::run(args),
        Command::Proxy(cmd) => commands::proxy::run(cmd, dir),
        Command::Optimize(args) => commands::optimize::run(args, dir),
        Command::Adapters(args) => commands::adapters::run(args),
        Command::Pack(cmd) => commands::pack::run(cmd),
        Command::Plugins(args) => commands::plugins::run(args, dir),
        Command::Secret(args) => commands::secret::run(args, dir),
        Command::Settings(args) => commands::settings::run(args, dir),
        Command::Export(args) => commands::bundle::run_export(args, dir),
        Command::Import(args) => commands::bundle::run_import(args, dir),
        Command::Dashboard(args) => agentstack::dashboard::serve(args, dir),
        Command::Mcp(args) => {
            agentstack::mcp_server::serve(dir, args.auto_project, args.transparent)
        }
        Command::Connect(args) => commands::connect::run_connect(args),
        Command::Disconnect(args) => commands::connect::run_disconnect(args),
        Command::Trust(args) => commands::trust::run(args),
        Command::Codemode(args) => commands::codemode::run(args, dir),
        Command::Hook(args) => commands::hook::run(args),
        Command::Guard(args) => commands::guard::run(args),
        Command::SelfCmd(args) => commands::self_cmd::run(args),
        Command::Run(args) => commands::runs::run(args, dir),
        Command::Runs(args) => commands::runs::list(args),
        Command::Kill(args) => commands::runs::kill(args),
        Command::Report(args) => commands::report::run(args),
        Command::Sign(args) => commands::verify_cmd::sign(args, dir),
        Command::Verify(args) => commands::verify_cmd::verify(args, dir),
    }
}
