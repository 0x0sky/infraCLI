mod cli;
mod config;
mod runtime;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, ServiceAction, ServiceArgs};
use config::{ConfigPath, ProjectConfig, Wizard};
use runtime::{DockerRuntime, Runtime};
use std::io::{self, Write};

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/n]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime = DockerRuntime;

    match cli.command {
        None => runtime.status(&ConfigPath::default())?,
        Some(Command::Conf(args)) => {
            let path = ConfigPath::resolve(args.path.as_deref())?;
            let config = Wizard::stdio().run(&path)?;
            config.write(&path)?;
            println!("configuration written to {}", path.display());
            if args.apply {
                runtime.apply(&config)?;
            }
        }
        Some(Command::Apply(args)) => {
            let path = ConfigPath::resolve(args.path.as_deref())?;
            runtime.apply(&ProjectConfig::read(&path)?)?;
        }
        Some(Command::Stop(args)) => {
            let path = ConfigPath::resolve(args.path.as_deref())?;
            runtime.stop_project(&ProjectConfig::read(&path)?)?;
        }
        Some(Command::Rm(args)) => {
            let path = ConfigPath::resolve(args.path.as_deref())?;
            let config = ProjectConfig::read(&path)?;
            if confirm(&format!("remove project {} runtime objects?", config.project))? {
                runtime.remove_project(&config)?;
            } else {
                println!("remove cancelled");
            }
        }
        Some(Command::Service(parts)) => {
            let args = ServiceArgs::try_from(parts)?;
            let config = ProjectConfig::read(&ConfigPath::default())?;
            match args.action {
                None => runtime.service_status(&config, &args.name)?,
                Some(ServiceAction::Logs) => runtime.logs(&config, &args.name)?,
                Some(ServiceAction::Restart) => runtime.restart(&config, &args.name)?,
                Some(ServiceAction::Stop) => runtime.stop_service(&config, &args.name)?,
                Some(ServiceAction::Rm) => {
                    if confirm(&format!("remove service {} runtime object?", args.name))? {
                        runtime.remove_service(&config, &args.name)?;
                    } else {
                        println!("remove cancelled");
                    }
                }
            }
        }
    }

    Ok(())
}
