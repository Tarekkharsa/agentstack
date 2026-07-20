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
        Command::Status(_) => commands::overview::run_status(dir),
        Command::Add(args) => commands::add::run(args, dir),
        Command::Install(args) => commands::install::run(args, dir),
        Command::Lock(args) => commands::lock::dispatch(args, dir),
        Command::Remove(args) => commands::remove::run(args, dir),
        Command::Setup(args) => commands::setup::run(args, dir),
        Command::Apply(args) => commands::apply::run(args, dir),
        Command::Diff(args) => commands::diff::run(args, dir),
        Command::Explain(args) => commands::explain::run(args, dir),
        Command::Use(args) => commands::use_profile::run(args, dir),
        Command::Session(args) => commands::session::run(args, dir),
        Command::Instructions(args) => commands::instructions::run(args, dir),
        Command::Adopt(args) => commands::adopt::run(args, dir),
        Command::Lib(args) => commands::lib::run(args, dir),
        Command::Restore(args) => commands::restore::run(args, dir),
        Command::Doctor(args) => commands::doctor::run(args, dir),
        Command::Search(args) => commands::search::run(args, dir),
        Command::Proxy(args) => commands::proxy::run(args, dir),
        Command::Optimize(args) => commands::optimize::run(args, dir),
        Command::Adapters(args) => commands::adapters::run(args),
        Command::Secret(args) => commands::secret::run(args, dir),
        Command::Settings(args) => commands::settings::run(args, dir),
        Command::Export(args) => commands::bundle::run_export(args, dir),
        Command::Import(args) => commands::bundle::run_import(args, dir),
        Command::Dashboard(args) => agentstack::dashboard::serve(args, dir),
        Command::Mcp(args) => agentstack::mcp_server::serve(
            dir,
            args.auto_project,
            args.transparent,
            args.grant.as_deref(),
        ),
        Command::Gateway(cmd) => match cmd {
            agentstack::cli::GatewayCmd::Connect(args) => commands::connect::run_connect(args),
            agentstack::cli::GatewayCmd::Disconnect(args) => {
                commands::connect::run_disconnect(args)
            }
        },
        Command::Trust(args) => commands::trust::run(args),
        Command::Guard(args) => commands::guard::run(args),
        Command::SelfCmd(args) => commands::self_cmd::run(args),
        Command::Run(args) => commands::runs::run(args, dir),
        Command::Kill(args) => commands::runs::kill(args),
        Command::Report(cmd) => match cmd {
            agentstack::cli::ReportCmd::Run(args) => commands::report::run(args),
            agentstack::cli::ReportCmd::Runs(args) => commands::runs::list(args),
            agentstack::cli::ReportCmd::Usage(args) => commands::stats::run(args, dir),
            agentstack::cli::ReportCmd::Calls(args) => commands::analyze::run(args),
            agentstack::cli::ReportCmd::Wire(args) => commands::report::wire(args),
        },
        Command::Sign(args) => commands::verify_cmd::sign(args, dir),
        Command::Verify(args) => commands::verify_cmd::verify(args, dir),
    }
}
