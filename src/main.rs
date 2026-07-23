use clap::{Parser, Subcommand};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ridup", version, about = "The Riddle toolchain manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Show,
    Default {
        toolchain: String,
    },
    Override {
        #[command(subcommand)]
        command: OverrideCommand,
    },
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommand,
    },
    Run {
        toolchain: String,
        component: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

#[derive(Subcommand)]
enum OverrideCommand {
    Set {
        toolchain: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    Unset {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum ToolchainCommand {
    /// Link an existing local build or unpacked toolchain.
    Link { name: String, path: PathBuf },
    /// Download a release channel or build Canary from source.
    Install {
        #[arg(value_parser = ["stable", "nightly", "canary"])]
        channel: String,
    },
    /// List installed and linked toolchains.
    List,
}

fn main() -> anyhow::Result<()> {
    let executable = env::current_exe()?;
    if let Some(component) = ridup::proxy_name(executable.as_os_str()) {
        return run_proxy(component);
    }

    let home = ridup::home()?;
    match Cli::parse().command {
        Commands::Show => {
            let active = ridup::resolve_toolchain(&home, &env::current_dir()?, None)?;
            println!("active toolchain: {}", active.name);
            println!("active because: {}", active.reason);
            println!("toolchain root: {}", active.root.display());
        }
        Commands::Default { toolchain } => {
            ridup::set_default(&home, &toolchain)?;
            println!("ridup: default toolchain set to `{toolchain}`");
        }
        Commands::Override { command } => match command {
            OverrideCommand::Set { toolchain, path } => {
                let path = ridup::set_override(&home, &path, &toolchain)?;
                println!(
                    "ridup: override for `{}` set to `{toolchain}`",
                    path.display()
                );
            }
            OverrideCommand::Unset { path } => {
                let path = ridup::unset_override(&home, &path)?;
                println!("ridup: override removed for `{}`", path.display());
            }
        },
        Commands::Toolchain { command } => match command {
            ToolchainCommand::Link { name, path } => {
                let path = ridup::link_toolchain(&home, &name, &path)?;
                println!("ridup: linked `{name}` to `{}`", path.display());
            }
            ToolchainCommand::Install { channel } => {
                let channel = channel.parse::<ridup::ReleaseChannel>()?;
                println!("ridup: installing `{}`...", channel.as_str());
                let path = ridup::install_toolchain(&home, channel)?;
                println!(
                    "ridup: installed `{}` at `{}`",
                    channel.as_str(),
                    path.display()
                );
            }
            ToolchainCommand::List => {
                for name in ridup::list_toolchains(&home)? {
                    println!("{name}");
                }
            }
        },
        Commands::Run {
            toolchain,
            component,
            args,
        } => {
            let active = ridup::resolve_toolchain(&home, &env::current_dir()?, Some(&toolchain))?;
            exit_with(ridup::run_component(&active, &component, &args)?);
        }
    }
    Ok(())
}

fn run_proxy(component: &str) -> anyhow::Result<()> {
    let home = ridup::home()?;
    let mut args = env::args_os().skip(1).collect::<Vec<_>>();
    let explicit = ridup::take_toolchain_override(&mut args)?;
    let active = ridup::resolve_toolchain(&home, &env::current_dir()?, explicit.as_deref())?;
    exit_with(ridup::run_component(&active, component, &args)?)
}

fn exit_with(status: std::process::ExitStatus) -> ! {
    std::process::exit(status.code().unwrap_or(1))
}
