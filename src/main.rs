use clap::{Parser, Subcommand};
use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
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
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },
    CToolchain {
        #[command(subcommand)]
        command: CToolchainCommand,
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

#[derive(Subcommand)]
enum TargetCommand {
    /// Install a target runtime component.
    Add {
        triple: String,
        #[arg(long)]
        toolchain: Option<String>,
        /// Install the matching C compiler without prompting.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Remove an installed target component and its C configuration.
    Remove {
        triple: String,
        #[arg(long)]
        toolchain: Option<String>,
    },
    /// List target-component and C-toolchain readiness separately.
    List {
        #[arg(long)]
        toolchain: Option<String>,
    },
    /// Configure C compiler, sysroot, or platform SDK paths.
    Configure {
        triple: String,
        #[arg(long)]
        toolchain: Option<String>,
        #[arg(long)]
        compiler: Option<PathBuf>,
        #[arg(long)]
        sysroot: Option<PathBuf>,
        #[arg(long)]
        windows_sdk: Option<PathBuf>,
        #[arg(long)]
        msvc: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum CToolchainCommand {
    /// Detect or install the matching Clang/LLD toolchain.
    Install {
        triple: String,
        #[arg(long)]
        toolchain: Option<String>,
    },
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
        Commands::Target { command } => match command {
            TargetCommand::Add {
                triple,
                toolchain,
                yes,
            } => {
                let active = active_toolchain(&home, toolchain.as_deref())?;
                let installed = ridup::install_target(&home, &active, &triple)?;
                if installed.native {
                    println!(
                        "ridup: target `{}` is included in the host toolchain",
                        installed.triple
                    );
                    let status = status_for(&active, installed.triple)?;
                    if !status.c_toolchain_ready {
                        println!("  C toolchain: {}", status.reason);
                        if yes || confirm_c_install(&installed.llvm_version)? {
                            install_c_toolchain(&active, installed.triple)?;
                        }
                    }
                } else {
                    println!("ridup: installed target `{}`", installed.triple);
                    println!("  release: {}", installed.release_version);
                    println!("  runtime: {}", installed.runtime.unwrap().display());
                    println!("  LLVM/Clang baseline: {}", installed.llvm_version);
                    let status = status_for(&active, installed.triple)?;
                    if !status.c_toolchain_ready {
                        println!("  C toolchain: {}", status.reason);
                        let install = yes || confirm_c_install(&installed.llvm_version)?;
                        if install {
                            install_c_toolchain(&active, installed.triple)?;
                        } else {
                            println!(
                                "ridup: target component installed; configure C later with `ridup c-toolchain install {}`",
                                installed.triple
                            );
                        }
                    }
                }
            }
            TargetCommand::Remove { triple, toolchain } => {
                let active = active_toolchain(&home, toolchain.as_deref())?;
                ridup::remove_target(&active, &triple)?;
                println!("ridup: removed target `{triple}` from `{}`", active.name);
            }
            TargetCommand::List { toolchain } => {
                let active = active_toolchain(&home, toolchain.as_deref())?;
                for status in ridup::list_targets(&active)? {
                    println!(
                        "{}  component={}  c-toolchain={}  {}",
                        status.triple,
                        if status.installed {
                            "installed"
                        } else {
                            "missing"
                        },
                        if status.c_toolchain_ready {
                            "ready"
                        } else {
                            "missing"
                        },
                        status.reason
                    );
                }
            }
            TargetCommand::Configure {
                triple,
                toolchain,
                compiler,
                sysroot,
                windows_sdk,
                msvc,
            } => {
                let active = active_toolchain(&home, toolchain.as_deref())?;
                let path = ridup::configure_target(
                    &active,
                    &triple,
                    compiler,
                    sysroot,
                    windows_sdk,
                    msvc,
                )?;
                println!("ridup: wrote `{}`", path.display());
                print_target_status(&status_for(&active, &triple)?);
            }
        },
        Commands::CToolchain { command } => match command {
            CToolchainCommand::Install { triple, toolchain } => {
                let active = active_toolchain(&home, toolchain.as_deref())?;
                install_c_toolchain(&active, &triple)?;
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

fn active_toolchain(
    home: &std::path::Path,
    explicit: Option<&str>,
) -> anyhow::Result<ridup::ActiveToolchain> {
    ridup::resolve_toolchain(home, &env::current_dir()?, explicit)
}

fn status_for(
    active: &ridup::ActiveToolchain,
    triple: &str,
) -> anyhow::Result<ridup::TargetStatus> {
    ridup::list_targets(active)?
        .into_iter()
        .find(|status| status.triple == triple)
        .ok_or_else(|| anyhow::anyhow!("unsupported target `{triple}`"))
}

fn install_c_toolchain(active: &ridup::ActiveToolchain, triple: &str) -> anyhow::Result<()> {
    println!(
        "ridup: installing or detecting LLVM {}...",
        ridup::LLVM_VERSION
    );
    let installed = ridup::install_c_toolchain(active, triple)?;
    println!("ridup: C compiler `{}`", installed.compiler.display());
    println!("  config: {}", installed.config_path.display());
    print_target_status(&installed.status);
    Ok(())
}

fn print_target_status(status: &ridup::TargetStatus) {
    println!(
        "  target {}: component={}, C toolchain={} ({})",
        status.triple,
        if status.installed {
            "installed"
        } else {
            "missing"
        },
        if status.c_toolchain_ready {
            "ready"
        } else {
            "incomplete"
        },
        status.reason
    );
}

fn confirm_c_install(version: &str) -> anyhow::Result<bool> {
    if !io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("Install the matching LLVM/Clang {version} C toolchain now? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
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
