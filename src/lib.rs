use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::str::FromStr;
use std::time::Duration;
use zip::ZipArchive;

mod target;

pub use target::{LLVM_VERSION, SUPPORTED_TARGETS, TargetStatus};

pub const PROJECT_TOOLCHAIN_FILE: &str = "riddle-toolchain.toml";
const GITHUB_REPOSITORY: &str = "riddle-lang/riddle";
const HTTP_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
    Nightly,
    Canary,
}

impl FromStr for ReleaseChannel {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "stable" => Ok(Self::Stable),
            "nightly" => Ok(Self::Nightly),
            "canary" => Ok(Self::Canary),
            _ => bail!("unknown release channel `{value}`; expected stable, nightly, or canary"),
        }
    }
}

impl ReleaseChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
            Self::Canary => "canary",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseResponse {
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct DistributionManifest {
    schema: u32,
    channel: String,
    version: String,
    llvm_version: String,
    #[serde(default)]
    targets: Vec<DistributionAsset>,
}

#[derive(Debug, Deserialize)]
struct DistributionAsset {
    triple: String,
    asset: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct CommitResponse {
    sha: String,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub digest: Option<String>,
}

pub fn install_toolchain(home: &Path, channel: ReleaseChannel) -> anyhow::Result<PathBuf> {
    match channel {
        ReleaseChannel::Canary => install_canary(home),
        ReleaseChannel::Stable | ReleaseChannel::Nightly => install_release(home, channel),
    }
}

fn install_release(home: &Path, channel: ReleaseChannel) -> anyhow::Result<PathBuf> {
    let agent = github_agent();
    let release = fetch_release(&agent, channel)?;
    let suffix = release_asset_suffix(std::env::consts::OS, std::env::consts::ARCH)?;
    let asset = select_release_asset(&release.assets, &suffix)?;
    let bytes = download_bytes(
        &agent,
        &asset.browser_download_url,
        &format!("release asset `{}`", asset.name),
        128 * 1024 * 1024,
    )?;
    if let Some(digest) = &asset.digest {
        verify_digest(&bytes, digest)?;
    } else {
        bail!("release asset `{}` has no SHA-256 digest", asset.name);
    }
    let install_name = host_toolchain_name(channel)?;
    let install_root = home.join("toolchains").join(&install_name);
    let temp_root = temporary_install_root(home, &install_name);
    prepare_temp_root(&temp_root)?;
    extract_archive(&bytes, &temp_root)?;
    validate_toolchain_root(&temp_root)?;
    preserve_installed_targets(&install_root, &temp_root)?;
    refresh_target_runtimes(&temp_root, &temp_root.join("runtime.c"))?;
    replace_install_root(&temp_root, &install_root)?;
    link_toolchain(home, &install_name, &install_root)?;
    link_toolchain(home, channel.as_str(), &install_root)
}

fn fetch_release(agent: &ureq::Agent, channel: ReleaseChannel) -> anyhow::Result<ReleaseResponse> {
    let release_url = match channel {
        ReleaseChannel::Stable => {
            format!("https://api.github.com/repos/{GITHUB_REPOSITORY}/releases/latest")
        }
        ReleaseChannel::Nightly => {
            format!("https://api.github.com/repos/{GITHUB_REPOSITORY}/releases/tags/nightly")
        }
        ReleaseChannel::Canary => unreachable!(),
    };
    agent
        .get(&release_url)
        .header("User-Agent", "ridup")
        .header("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| format!("failed to query {channel:?} release"))?
        .body_mut()
        .read_json()
        .context("invalid GitHub release response")
}

fn github_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

fn install_canary(home: &Path) -> anyhow::Result<PathBuf> {
    let agent = github_agent();
    let commit_url = format!("https://api.github.com/repos/{GITHUB_REPOSITORY}/commits/main");
    let commit: CommitResponse = agent
        .get(&commit_url)
        .header("User-Agent", "ridup")
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("failed to query the latest canary commit")?
        .body_mut()
        .read_json()
        .context("invalid GitHub commit response")?;
    let archive_url = format!(
        "https://github.com/{GITHUB_REPOSITORY}/archive/{}.zip",
        commit.sha
    );
    let bytes = download_bytes(
        &agent,
        &archive_url,
        "canary source archive",
        128 * 1024 * 1024,
    )?;

    let sources = home.join("sources");
    fs::create_dir_all(&sources)?;
    let download_root = sources.join(format!(".riddle.download-{}", std::process::id()));
    prepare_temp_root(&download_root)?;
    extract_archive(&bytes, &download_root)?;
    let archive_root = single_directory(&download_root)?;
    let source = home.join("sources").join("riddle");
    replace_install_root(&archive_root, &source)?;
    fs::remove_dir(download_root)?;

    let cargo_target = home.join("build").join("canary");
    let status = Command::new("cargo")
        .current_dir(&source)
        .args(["build", "--workspace", "--release"])
        .env("CARGO_TARGET_DIR", &cargo_target)
        .status()
        .context("failed to run `cargo`")?;
    if !status.success() {
        bail!("canary build failed with status {status}");
    }
    let build_root = cargo_target.join("release");
    let install_name = host_toolchain_name(ReleaseChannel::Canary)?;
    let install_root = home.join("toolchains").join(&install_name);
    let temp_root = temporary_install_root(home, &install_name);
    prepare_temp_root(&temp_root)?;
    for component in ["clue", "riddlec", "riddle-lsp"] {
        let source_path = component_path(&build_root, component)?;
        let target_name = source_path
            .file_name()
            .context("component has no file name")?;
        fs::copy(&source_path, temp_root.join(target_name))
            .with_context(|| format!("failed to install `{component}`"))?;
    }
    let runtime = source.join("crates/gc/src/runtime.c");
    fs::copy(&runtime, temp_root.join("runtime.c"))?;
    preserve_installed_targets(&install_root, &temp_root)?;
    refresh_target_runtimes(&temp_root, &runtime)?;
    replace_install_root(&temp_root, &install_root)?;
    link_toolchain(home, &install_name, &install_root)?;
    link_toolchain(home, "canary", &install_root)
}

fn host_toolchain_name(channel: ReleaseChannel) -> anyhow::Result<String> {
    Ok(format!("{}-{}", channel.as_str(), target::host()?))
}

fn release_channel_for_toolchain(name: &str) -> anyhow::Result<ReleaseChannel> {
    if let Ok(channel) = name.parse() {
        return Ok(channel);
    }
    for channel in [
        ReleaseChannel::Stable,
        ReleaseChannel::Nightly,
        ReleaseChannel::Canary,
    ] {
        if name == host_toolchain_name(channel)? {
            return Ok(channel);
        }
    }
    bail!("linked toolchain `{name}` has no target distribution; use stable, nightly, or canary")
}

fn preserve_installed_targets(existing_root: &Path, next_root: &Path) -> anyhow::Result<()> {
    let existing = existing_root.join("targets");
    if existing.is_dir() {
        copy_directory(&existing, &next_root.join("targets"))?;
    }
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let output = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory(&entry.path(), &output)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), output)?;
        } else {
            bail!(
                "unsupported entry in target component: `{}`",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn refresh_target_runtimes(toolchain_root: &Path, runtime: &Path) -> anyhow::Result<()> {
    if !runtime.is_file() {
        return Ok(());
    }
    for triple in SUPPORTED_TARGETS {
        let path = target::component_root(toolchain_root, triple).join("runtime.c");
        if path.is_file() {
            fs::copy(runtime, path)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct InstalledTarget {
    pub triple: &'static str,
    pub runtime: Option<PathBuf>,
    pub llvm_version: String,
    pub release_version: String,
    pub native: bool,
}

#[derive(Debug)]
pub struct InstalledCToolchain {
    pub compiler: PathBuf,
    pub config_path: PathBuf,
    pub status: TargetStatus,
}

pub fn install_target(
    home: &Path,
    active: &ActiveToolchain,
    triple: &str,
) -> anyhow::Result<InstalledTarget> {
    let triple = target::validate(triple)?;
    if triple == target::host()? {
        return Ok(InstalledTarget {
            triple,
            runtime: None,
            llvm_version: LLVM_VERSION.to_owned(),
            release_version: active.name.clone(),
            native: true,
        });
    }
    let channel = release_channel_for_toolchain(&active.name)?;
    if channel == ReleaseChannel::Canary {
        return install_canary_target(home, active, triple);
    }
    let agent = github_agent();
    let release = fetch_release(&agent, channel)?;
    let manifest = fetch_distribution_manifest(&agent, &release, channel)?;
    let entry = manifest
        .targets
        .iter()
        .find(|entry| entry.triple == triple)
        .with_context(|| {
            format!(
                "release {} does not support target `{triple}`",
                manifest.version
            )
        })?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == entry.asset)
        .with_context(|| format!("release is missing target asset `{}`", entry.asset))?;
    let bytes = download_bytes(
        &agent,
        &asset.browser_download_url,
        &format!("target component `{triple}`"),
        32 * 1024 * 1024,
    )?;
    verify_sha256(&bytes, &entry.sha256)?;
    let component = activate_target_archive(&active.root, triple, &bytes)?;
    Ok(InstalledTarget {
        triple,
        runtime: Some(target::component_root(&active.root, triple).join(component.runtime)),
        llvm_version: component
            .llvm_version
            .unwrap_or_else(|| manifest.llvm_version.clone()),
        release_version: manifest.version,
        native: false,
    })
}

fn fetch_distribution_manifest(
    agent: &ureq::Agent,
    release: &ReleaseResponse,
    channel: ReleaseChannel,
) -> anyhow::Result<DistributionManifest> {
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == "riddle-dist.toml")
        .context("release has no `riddle-dist.toml`; install a newer Riddle toolchain")?;
    let bytes = download_bytes(
        agent,
        &asset.browser_download_url,
        "Riddle distribution manifest",
        1024 * 1024,
    )?;
    if let Some(digest) = &asset.digest {
        verify_digest(&bytes, digest)?;
    }
    let text = std::str::from_utf8(&bytes).context("distribution manifest is not UTF-8")?;
    let manifest: DistributionManifest =
        toml::from_str(text).context("invalid Riddle distribution manifest")?;
    if manifest.schema != 1 {
        bail!("unsupported Riddle distribution schema {}", manifest.schema);
    }
    if manifest.channel != channel.as_str() {
        bail!(
            "distribution manifest channel `{}` does not match `{}`",
            manifest.channel,
            channel.as_str()
        );
    }
    Ok(manifest)
}

fn install_canary_target(
    home: &Path,
    active: &ActiveToolchain,
    triple: &'static str,
) -> anyhow::Result<InstalledTarget> {
    let runtime = home.join("sources/riddle/crates/gc/src/runtime.c");
    if !runtime.is_file() {
        bail!(
            "Canary source runtime is missing from `{}`; reinstall the canary toolchain",
            runtime.display()
        );
    }
    let targets = active.root.join("targets");
    fs::create_dir_all(&targets)?;
    let temp_root = targets.join(format!(".{triple}.install-{}", std::process::id()));
    prepare_temp_root(&temp_root)?;
    fs::copy(&runtime, temp_root.join("runtime.c"))?;
    let manifest_text = format!(
        "schema = 1\ntriple = \"{triple}\"\nruntime = \"runtime.c\"\nllvm_version = \"{LLVM_VERSION}\"\n"
    );
    fs::write(temp_root.join("target.toml"), &manifest_text)?;
    target::validate_component_root(&temp_root, triple, &manifest_text)?;
    preserve_c_toolchain(&active.root, triple, &temp_root)?;
    let install_root = target::component_root(&active.root, triple);
    replace_install_root(&temp_root, &install_root)?;
    Ok(InstalledTarget {
        triple,
        runtime: Some(install_root.join("runtime.c")),
        llvm_version: LLVM_VERSION.to_owned(),
        release_version: "canary".to_owned(),
        native: false,
    })
}

fn activate_target_archive(
    toolchain_root: &Path,
    triple: &str,
    bytes: &[u8],
) -> anyhow::Result<target::ComponentManifest> {
    let targets = toolchain_root.join("targets");
    fs::create_dir_all(&targets)?;
    let temp_root = targets.join(format!(".{triple}.install-{}", std::process::id()));
    prepare_temp_root(&temp_root)?;
    extract_archive(bytes, &temp_root)?;
    let manifest_path = temp_root.join("target.toml");
    let manifest_text = fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "target archive is incomplete; missing `{}`",
            manifest_path.display()
        )
    })?;
    let manifest = target::validate_component_root(&temp_root, triple, &manifest_text)?;
    preserve_c_toolchain(toolchain_root, triple, &temp_root)?;
    replace_install_root(&temp_root, &target::component_root(toolchain_root, triple))?;
    Ok(manifest)
}

fn preserve_c_toolchain(
    toolchain_root: &Path,
    triple: &str,
    temp_root: &Path,
) -> anyhow::Result<()> {
    let existing = target::component_root(toolchain_root, triple).join("c-toolchain.toml");
    if existing.is_file() {
        fs::copy(existing, temp_root.join("c-toolchain.toml"))?;
    }
    Ok(())
}

pub fn remove_target(active: &ActiveToolchain, triple: &str) -> anyhow::Result<()> {
    let triple = target::validate(triple)?;
    if triple == target::host()? {
        bail!("target `{triple}` is provided by the host toolchain and cannot be removed");
    }
    let root = target::component_root(&active.root, triple);
    if !root.is_dir() {
        bail!(
            "target `{triple}` is not installed for toolchain `{}`",
            active.name
        );
    }
    fs::remove_dir_all(&root)
        .with_context(|| format!("failed to remove target `{}`", root.display()))
}

pub fn list_targets(active: &ActiveToolchain) -> anyhow::Result<Vec<TargetStatus>> {
    SUPPORTED_TARGETS
        .into_iter()
        .map(|triple| target::status(&active.root, triple))
        .collect()
}

pub fn configure_target(
    active: &ActiveToolchain,
    triple: &str,
    compiler: Option<PathBuf>,
    sysroot: Option<PathBuf>,
    windows_sdk: Option<PathBuf>,
    msvc: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let triple = target::validate(triple)?;
    if target::load_component(&active.root, triple)?.is_none() && triple != target::host()? {
        bail!("target `{triple}` is not installed; run `ridup target add {triple}` first");
    }
    if compiler.is_none() && sysroot.is_none() && windows_sdk.is_none() && msvc.is_none() {
        bail!("no C toolchain setting was provided");
    }
    let mut config = target::load_c_toolchain(&active.root, triple)?;
    if let Some(compiler) = compiler {
        config.compiler = Some(resolve_compiler(&compiler)?);
    }
    if let Some(sysroot) = sysroot {
        config.sysroot = Some(resolve_directory("sysroot", &sysroot)?);
    }
    if let Some(windows_sdk) = windows_sdk {
        config.windows_sdk = Some(resolve_directory("Windows SDK", &windows_sdk)?);
    }
    if let Some(msvc) = msvc {
        config.msvc = Some(resolve_directory("MSVC", &msvc)?);
    }
    target::save_c_toolchain(&active.root, triple, &config)
}

pub fn install_c_toolchain(
    active: &ActiveToolchain,
    triple: &str,
) -> anyhow::Result<InstalledCToolchain> {
    let triple = target::validate(triple)?;
    if target::load_component(&active.root, triple)?.is_none() && triple != target::host()? {
        bail!("target `{triple}` is not installed; run `ridup target add {triple}` first");
    }
    let mut compiler = target::find_clang(triple);
    if compiler.is_none() {
        let (program, args) = target::package_manager_command().with_context(|| {
            format!(
                "no Clang was found and no supported package manager is available; install LLVM {LLVM_VERSION}, then run `ridup target configure {triple} --compiler <clang>`"
            )
        })?;
        let status = Command::new(program)
            .args(args)
            .status()
            .with_context(|| format!("failed to run `{program}`"))?;
        if !status.success() {
            bail!(
                "LLVM {LLVM_VERSION} installation failed with status {status}; the target component remains installed, so install Clang manually and run `ridup target configure {triple} --compiler <clang>`"
            );
        }
        compiler = target::find_clang(triple);
    }
    let compiler = compiler.with_context(|| {
        format!(
            "LLVM {LLVM_VERSION} was installed, but Clang was not found; run `ridup target configure {triple} --compiler <clang>`"
        )
    })?;
    let mut config = target::load_c_toolchain(&active.root, triple)?;
    config.compiler = Some(compiler.clone());
    if config.sysroot.is_none() {
        config.sysroot = discover_linux_sysroot(triple);
    }
    if triple.ends_with("-pc-windows-msvc") {
        if config.windows_sdk.is_none() {
            config.windows_sdk = discover_windows_sdk();
        }
        if config.msvc.is_none() {
            config.msvc = discover_msvc();
        }
    }
    let config_path = target::save_c_toolchain(&active.root, triple, &config)?;
    let status = target::status(&active.root, triple)?;
    Ok(InstalledCToolchain {
        compiler,
        config_path,
        status,
    })
}

fn resolve_compiler(path: &Path) -> anyhow::Result<PathBuf> {
    if path.components().count() == 1 {
        return target::locate_program(&path.to_string_lossy())
            .with_context(|| format!("C compiler `{}` was not found on PATH", path.display()));
    }
    fs::canonicalize(path)
        .with_context(|| format!("failed to resolve C compiler `{}`", path.display()))
        .and_then(|path| {
            if path.is_file() {
                Ok(path)
            } else {
                bail!("C compiler `{}` is not a file", path.display())
            }
        })
}

fn resolve_directory(kind: &str, path: &Path) -> anyhow::Result<PathBuf> {
    fs::canonicalize(path)
        .with_context(|| format!("failed to resolve {kind} `{}`", path.display()))
        .and_then(|path| {
            if path.is_dir() {
                Ok(path)
            } else {
                bail!("{kind} `{}` is not a directory", path.display())
            }
        })
}

fn discover_linux_sysroot(triple: &str) -> Option<PathBuf> {
    let gnu = match triple {
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "i686-unknown-linux-gnu" => "i386-linux-gnu",
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        _ => return None,
    };
    [
        PathBuf::from(format!("/usr/{gnu}")),
        PathBuf::from(format!("/usr/{triple}")),
    ]
    .into_iter()
    .find(|path| path.is_dir())
}

fn discover_windows_sdk() -> Option<PathBuf> {
    env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| {
            env::var_os("ProgramFiles(x86)")
                .map(PathBuf::from)
                .map(|root| root.join("Windows Kits/10"))
                .filter(|path| path.join("Include").is_dir() && path.join("Lib").is_dir())
        })
}

fn discover_msvc() -> Option<PathBuf> {
    if let Some(path) = env::var_os("VCToolsInstallDir")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
    {
        return Some(path);
    }
    let program_files = env::var_os("ProgramFiles(x86)").map(PathBuf::from)?;
    let vswhere = program_files.join("Microsoft Visual Studio/Installer/vswhere.exe");
    if vswhere.is_file()
        && let Ok(output) = Command::new(vswhere)
            .args([
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ])
            .output()
        && output.status.success()
    {
        let installation = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !installation.is_empty()
            && let Some(path) = latest_directory(&PathBuf::from(installation).join("VC/Tools/MSVC"))
        {
            return Some(path);
        }
    }

    let root = program_files.join("Microsoft Visual Studio");
    let mut versions = fs::read_dir(root)
        .ok()?
        .flatten()
        .flat_map(|year| fs::read_dir(year.path()).into_iter().flatten().flatten())
        .filter_map(|edition| latest_directory(&edition.path().join("VC/Tools/MSVC")))
        .collect::<Vec<_>>();
    versions.sort();
    versions.pop()
}

fn latest_directory(root: &Path) -> Option<PathBuf> {
    let mut directories = fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    directories.sort();
    directories.pop()
}

fn verify_sha256(bytes: &[u8], expected: &str) -> anyhow::Result<()> {
    if expected.starts_with("sha256:") {
        verify_digest(bytes, expected)
    } else {
        verify_digest(bytes, &format!("sha256:{expected}"))
    }
}

fn download_bytes(
    agent: &ureq::Agent,
    url: &str,
    description: &str,
    limit: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut response = agent
        .get(url)
        .header("User-Agent", "ridup")
        .call()
        .with_context(|| format!("failed to download {description}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {description}"))?;
    if bytes.len() > limit {
        bail!(
            "{description} exceeds the {} MiB limit",
            limit / 1024 / 1024
        );
    }
    Ok(bytes)
}

fn single_directory(root: &Path) -> anyhow::Result<PathBuf> {
    let mut entries = fs::read_dir(root)?;
    let entry = entries
        .next()
        .transpose()?
        .context("source archive is empty")?;
    if entries.next().transpose()?.is_some() || !entry.path().is_dir() {
        bail!("source archive must contain exactly one top-level directory");
    }
    Ok(entry.path())
}

fn temporary_install_root(home: &Path, channel: &str) -> PathBuf {
    home.join("toolchains")
        .join(format!(".{channel}.install-{}", std::process::id()))
}

fn prepare_temp_root(temp_root: &Path) -> anyhow::Result<()> {
    if temp_root.exists() {
        fs::remove_dir_all(temp_root)?;
    }
    fs::create_dir_all(temp_root).map_err(Into::into)
}

fn replace_install_root(temp_root: &Path, install_root: &Path) -> anyhow::Result<()> {
    if let Some(parent) = install_root.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = install_root
        .file_name()
        .and_then(OsStr::to_str)
        .context("install root has no valid directory name")?;
    let backup_root = install_root.with_file_name(format!(".{name}.backup-{}", std::process::id()));
    if backup_root.exists() {
        fs::remove_dir_all(&backup_root)?;
    }
    let had_existing = install_root.exists();
    if had_existing {
        fs::rename(install_root, &backup_root)
            .with_context(|| format!("failed to replace `{}`", install_root.display()))?;
    }
    if let Err(error) = fs::rename(temp_root, install_root) {
        if had_existing {
            fs::rename(&backup_root, install_root).with_context(|| {
                format!(
                    "failed to restore previous toolchain from `{}`",
                    backup_root.display()
                )
            })?;
        }
        return Err(error)
            .with_context(|| format!("failed to activate `{}`", install_root.display()));
    }
    if had_existing {
        fs::remove_dir_all(backup_root)?;
    }
    Ok(())
}

fn extract_archive(bytes: &[u8], destination: &Path) -> anyhow::Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).context("invalid toolchain archive")?;
    let mut extracted_size = 0_u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        #[cfg(unix)]
        let unix_mode = file.unix_mode();
        extracted_size = extracted_size
            .checked_add(file.size())
            .context("toolchain archive size overflow")?;
        if extracted_size > 256 * 1024 * 1024 {
            bail!("toolchain archive expands beyond 256 MiB");
        }
        let relative = file
            .enclosed_name()
            .context("toolchain archive contains an unsafe path")?
            .to_owned();
        let output = destination.join(relative);
        if file.is_dir() {
            fs::create_dir_all(&output)?;
        } else {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            fs::write(&output, bytes)?;
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&output, fs::Permissions::from_mode(mode))?;
            }
        }
    }
    Ok(())
}

fn validate_toolchain_root(root: &Path) -> anyhow::Result<()> {
    for component in ["clue", "riddlec", "riddle-lsp"] {
        component_path(root, component)?;
    }
    Ok(())
}

fn release_asset_suffix(os: &str, arch: &str) -> anyhow::Result<String> {
    let platform = match os {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => bail!("unsupported host operating system `{os}`"),
    };
    let architecture = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "x86" => "i686",
        _ => bail!("unsupported host architecture `{arch}`"),
    };
    if platform == "macos" && architecture != "aarch64" {
        bail!("no macOS {architecture} release is available");
    }
    Ok(format!("{platform}-{architecture}.zip"))
}

fn select_release_asset<'a>(
    assets: &'a [ReleaseAsset],
    suffix: &str,
) -> anyhow::Result<&'a ReleaseAsset> {
    assets
        .iter()
        .find(|asset| asset.name.ends_with(suffix))
        .ok_or_else(|| anyhow::anyhow!("release does not contain a `{suffix}` asset"))
}

fn verify_digest(bytes: &[u8], digest: &str) -> anyhow::Result<()> {
    let expected = digest
        .strip_prefix("sha256:")
        .context("release asset digest is not SHA-256")?;
    let mut actual = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(actual, "{byte:02x}").unwrap();
    }
    if actual == expected {
        Ok(())
    } else {
        bail!("release asset SHA-256 mismatch: expected {expected}, got {actual}")
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub default_toolchain: Option<String>,
    #[serde(default)]
    pub toolchains: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub overrides: BTreeMap<PathBuf, String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ActiveToolchain {
    pub name: String,
    pub root: PathBuf,
    pub reason: String,
}

#[derive(Deserialize)]
struct ToolchainFile {
    toolchain: ToolchainSelection,
}

#[derive(Deserialize)]
struct ToolchainSelection {
    channel: String,
}

pub fn home() -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os("RIDUP_HOME") {
        return Ok(PathBuf::from(path));
    }
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .context("cannot determine ridup home; set RIDUP_HOME")
        .map(|path| path.join(".ridup"))
}

pub fn load_config(home: &Path) -> anyhow::Result<Config> {
    let path = home.join("config.toml");
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid ridup config `{}`", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(error) => Err(error).with_context(|| format!("failed to read `{}`", path.display())),
    }
}

pub fn save_config(home: &Path, config: &Config) -> anyhow::Result<()> {
    fs::create_dir_all(home)?;
    let path = home.join("config.toml");
    let text = toml::to_string_pretty(config)?;
    fs::write(&path, text).with_context(|| format!("failed to write `{}`", path.display()))
}

pub fn link_toolchain(home: &Path, name: &str, path: &Path) -> anyhow::Result<PathBuf> {
    validate_name(name)?;
    let root = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve toolchain path `{}`", path.display()))?;
    if !root.is_dir() {
        bail!("toolchain path `{}` is not a directory", root.display());
    }
    let mut config = load_config(home)?;
    config.toolchains.insert(name.to_owned(), root.clone());
    save_config(home, &config)?;
    Ok(root)
}

pub fn set_default(home: &Path, name: &str) -> anyhow::Result<()> {
    let mut config = load_config(home)?;
    toolchain_root(home, &config, name)?;
    config.default_toolchain = Some(name.to_owned());
    save_config(home, &config)
}

pub fn set_override(home: &Path, path: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let mut config = load_config(home)?;
    toolchain_root(home, &config, name)?;
    let path = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve override path `{}`", path.display()))?;
    config.overrides.insert(path.clone(), name.to_owned());
    save_config(home, &config)?;
    Ok(path)
}

pub fn unset_override(home: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let mut config = load_config(home)?;
    let path = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve override path `{}`", path.display()))?;
    if config.overrides.remove(&path).is_none() {
        bail!("no toolchain override is set for `{}`", path.display());
    }
    save_config(home, &config)?;
    Ok(path)
}

pub fn list_toolchains(home: &Path) -> anyhow::Result<Vec<String>> {
    let config = load_config(home)?;
    let mut names = config.toolchains.keys().cloned().collect::<BTreeSet<_>>();
    if let Ok(entries) = fs::read_dir(home.join("toolchains")) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            if let Some(name) = entry.file_name().to_str()
                && !name.starts_with('.')
            {
                names.insert(name.to_owned());
            }
        }
    }
    Ok(names.into_iter().collect())
}

pub fn resolve_toolchain(
    home: &Path,
    cwd: &Path,
    explicit: Option<&str>,
) -> anyhow::Result<ActiveToolchain> {
    let environment = env::var("RIDUP_TOOLCHAIN").ok();
    resolve_toolchain_with(home, cwd, explicit, environment.as_deref())
}

fn resolve_toolchain_with(
    home: &Path,
    cwd: &Path,
    explicit: Option<&str>,
    environment: Option<&str>,
) -> anyhow::Result<ActiveToolchain> {
    let config = load_config(home)?;
    let cwd = fs::canonicalize(cwd)
        .with_context(|| format!("failed to resolve current directory `{}`", cwd.display()))?;

    let selected = if let Some(name) = explicit {
        (name.to_owned(), "command-line override".to_owned())
    } else if let Some(name) = environment {
        (name.to_owned(), "RIDUP_TOOLCHAIN".to_owned())
    } else if let Some((path, name)) = cwd
        .ancestors()
        .find_map(|path| config.overrides.get(path).map(|name| (path, name)))
    {
        (
            name.clone(),
            format!("directory override for {}", path.display()),
        )
    } else if let Some((path, name)) = find_project_toolchain(&cwd)? {
        (name, path.display().to_string())
    } else if let Some(name) = &config.default_toolchain {
        (name.clone(), "default toolchain".to_owned())
    } else {
        bail!(
            "no active Riddle toolchain; run `ridup toolchain link <name> <path>` and `ridup default <name>`"
        );
    };

    validate_name(&selected.0)?;
    Ok(ActiveToolchain {
        root: toolchain_root(home, &config, &selected.0)?,
        name: selected.0,
        reason: selected.1,
    })
}

pub fn component_path(root: &Path, component: &str) -> anyhow::Result<PathBuf> {
    validate_name(component)?;
    let executable = if cfg!(windows) {
        format!("{component}.exe")
    } else {
        component.to_owned()
    };
    [root.join("bin").join(&executable), root.join(&executable)]
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "toolchain component `{component}` is missing from `{}`",
                root.display()
            )
        })
}

pub fn run_component(
    active: &ActiveToolchain,
    component: &str,
    args: &[OsString],
) -> anyhow::Result<ExitStatus> {
    let program = component_path(&active.root, component)?;
    Command::new(&program)
        .args(args)
        .env("RIDUP_TOOLCHAIN", &active.name)
        .env("RIDUP_TOOLCHAIN_ROOT", &active.root)
        .status()
        .with_context(|| format!("failed to run `{}`", program.display()))
}

pub fn take_toolchain_override(args: &mut Vec<OsString>) -> anyhow::Result<Option<String>> {
    let Some(first) = args.first().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let Some(name) = first.strip_prefix('+') else {
        return Ok(None);
    };
    validate_name(name)?;
    let name = name.to_owned();
    args.remove(0);
    Ok(Some(name))
}

fn find_project_toolchain(cwd: &Path) -> anyhow::Result<Option<(PathBuf, String)>> {
    for directory in cwd.ancestors() {
        let path = directory.join(PROJECT_TOOLCHAIN_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let file: ToolchainFile = toml::from_str(&text)
            .with_context(|| format!("invalid toolchain file `{}`", path.display()))?;
        validate_name(&file.toolchain.channel)?;
        return Ok(Some((path, file.toolchain.channel)));
    }
    Ok(None)
}

fn toolchain_root(home: &Path, config: &Config, name: &str) -> anyhow::Result<PathBuf> {
    validate_name(name)?;
    let path = config
        .toolchains
        .get(name)
        .cloned()
        .unwrap_or_else(|| home.join("toolchains").join(name));
    if path.is_dir() {
        Ok(path)
    } else {
        bail!("toolchain `{name}` is not installed")
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || matches!(name, "." | "..") || name.chars().any(std::path::is_separator) {
        bail!("invalid toolchain or component name `{name}`");
    }
    Ok(())
}

pub fn proxy_name(executable: &OsStr) -> Option<&str> {
    let stem = Path::new(executable).file_stem()?.to_str()?;
    matches!(stem, "clue" | "riddlec" | "riddle-lsp").then_some(stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_root(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "ridup-{name}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn fake_toolchain(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn resolves_command_environment_override_file_and_default_in_order() {
        let root = temp_root("precedence");
        let home = root.join("home");
        let project = root.join("project").join("nested");
        fs::create_dir_all(&project).unwrap();
        let mut config = Config::default();
        for name in ["default", "file", "directory", "environment", "explicit"] {
            config
                .toolchains
                .insert(name.into(), fake_toolchain(&root, name));
        }
        config.default_toolchain = Some("default".into());
        config.overrides.insert(
            fs::canonicalize(root.join("project")).unwrap(),
            "directory".into(),
        );
        save_config(&home, &config).unwrap();
        fs::write(
            root.join("project/riddle-toolchain.toml"),
            "[toolchain]\nchannel = \"file\"\n",
        )
        .unwrap();

        assert_eq!(
            resolve_toolchain_with(&home, &project, Some("explicit"), Some("environment"))
                .unwrap()
                .name,
            "explicit"
        );
        assert_eq!(
            resolve_toolchain_with(&home, &project, None, Some("environment"))
                .unwrap()
                .name,
            "environment"
        );
        assert_eq!(
            resolve_toolchain_with(&home, &project, None, None)
                .unwrap()
                .name,
            "directory"
        );
        config.overrides.clear();
        save_config(&home, &config).unwrap();
        assert_eq!(
            resolve_toolchain_with(&home, &project, None, None)
                .unwrap()
                .name,
            "file"
        );
        fs::remove_file(root.join("project/riddle-toolchain.toml")).unwrap();
        assert_eq!(
            resolve_toolchain_with(&home, &project, None, None)
                .unwrap()
                .name,
            "default"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_plus_toolchain_without_taking_normal_arguments() {
        let mut args = vec![OsString::from("+nightly"), OsString::from("build")];
        assert_eq!(
            take_toolchain_override(&mut args).unwrap().as_deref(),
            Some("nightly")
        );
        assert_eq!(args, [OsString::from("build")]);

        let mut args = vec![OsString::from("build")];
        assert_eq!(take_toolchain_override(&mut args).unwrap(), None);
        assert_eq!(args, [OsString::from("build")]);
    }

    #[test]
    fn parses_only_installable_release_channels() {
        assert_eq!(
            ReleaseChannel::from_str("stable").unwrap(),
            ReleaseChannel::Stable
        );
        assert_eq!(
            ReleaseChannel::from_str("nightly").unwrap(),
            ReleaseChannel::Nightly
        );
        assert_eq!(
            ReleaseChannel::from_str("canary").unwrap(),
            ReleaseChannel::Canary
        );
        assert!(ReleaseChannel::from_str("beta").is_err());
    }

    #[test]
    fn names_host_toolchains_with_the_full_triple_and_keeps_channel_aliases() {
        let full = host_toolchain_name(ReleaseChannel::Stable).unwrap();
        assert_eq!(full, format!("stable-{}", target::host().unwrap()));
        assert_eq!(
            release_channel_for_toolchain(&full).unwrap(),
            ReleaseChannel::Stable
        );
        assert_eq!(
            release_channel_for_toolchain("stable").unwrap(),
            ReleaseChannel::Stable
        );
    }

    #[test]
    fn maps_supported_hosts_to_release_asset_suffixes() {
        assert_eq!(
            release_asset_suffix("windows", "x86_64").unwrap(),
            "windows-x86_64.zip"
        );
        assert_eq!(
            release_asset_suffix("linux", "x86").unwrap(),
            "linux-i686.zip"
        );
        assert_eq!(
            release_asset_suffix("macos", "aarch64").unwrap(),
            "macos-aarch64.zip"
        );
        assert!(release_asset_suffix("macos", "x86_64").is_err());
    }

    #[test]
    fn selects_only_the_matching_platform_archive() {
        let assets = vec![
            ReleaseAsset {
                name: "riddle-v0.1.1-windows-x86_64.zip".into(),
                browser_download_url: "https://example.invalid/windows".into(),
                digest: Some("sha256:windows".into()),
            },
            ReleaseAsset {
                name: "riddle-v0.1.1-linux-x86_64.zip".into(),
                browser_download_url: "https://example.invalid/linux".into(),
                digest: Some("sha256:linux".into()),
            },
        ];

        assert_eq!(
            select_release_asset(&assets, "windows-x86_64.zip")
                .unwrap()
                .browser_download_url,
            "https://example.invalid/windows"
        );
        assert!(select_release_asset(&assets, "windows-aarch64.zip").is_err());
    }

    #[test]
    fn verifies_github_sha256_digests() {
        const EMPTY_SHA256: &str =
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(verify_digest(&[], EMPTY_SHA256).is_ok());
        assert!(verify_digest(b"changed", EMPTY_SHA256).is_err());
        assert!(verify_digest(&[], "md5:unsupported").is_err());
    }

    #[test]
    fn parses_the_distribution_manifest_emitted_by_release_workflows() {
        let manifest: DistributionManifest = toml::from_str(
            r#"
schema = 1
channel = "stable"
version = "0.1.1"
commit = "abc123"
llvm_version = "22.1.3"

[[hosts]]
triple = "x86_64-pc-windows-msvc"
asset = "riddle-v0.1.1-windows-x86_64.zip"
sha256 = "00"

[[targets]]
triple = "aarch64-unknown-linux-gnu"
asset = "riddle-v0.1.1-target-aarch64-unknown-linux-gnu.zip"
sha256 = "11"
"#,
        )
        .unwrap();
        assert_eq!(manifest.schema, 1);
        assert_eq!(manifest.channel, "stable");
        assert_eq!(manifest.targets.len(), 1);
        assert_eq!(manifest.targets[0].triple, "aarch64-unknown-linux-gnu");
    }

    #[test]
    fn keeps_existing_toolchain_when_activation_fails() {
        let root = temp_root("activation-rollback");
        let install_root = root.join("stable");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("old"), b"working").unwrap();

        assert!(replace_install_root(&root.join("missing"), &install_root).is_err());
        assert_eq!(fs::read(install_root.join("old")).unwrap(), b"working");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toolchain_updates_preserve_target_configuration_and_refresh_runtime() {
        let root = temp_root("target-update");
        let existing = root.join("existing");
        let next = root.join("next");
        let triple = SUPPORTED_TARGETS[0];
        let component = target::component_root(&existing, triple);
        fs::create_dir_all(&component).unwrap();
        fs::write(component.join("runtime.c"), b"old").unwrap();
        fs::write(
            component.join("c-toolchain.toml"),
            b"compiler = \"clang\"\n",
        )
        .unwrap();
        fs::create_dir_all(&next).unwrap();
        let runtime = next.join("runtime.c");
        fs::write(&runtime, b"new").unwrap();

        preserve_installed_targets(&existing, &next).unwrap();
        refresh_target_runtimes(&next, &runtime).unwrap();

        let updated = target::component_root(&next, triple);
        assert_eq!(fs::read(updated.join("runtime.c")).unwrap(), b"new");
        assert!(updated.join("c-toolchain.toml").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn requires_every_proxy_component_before_activation() {
        let root = temp_root("complete-toolchain");
        fs::create_dir_all(&root).unwrap();
        assert!(validate_toolchain_root(&root).is_err());
        for component in ["clue", "riddlec", "riddle-lsp"] {
            let name = if cfg!(windows) {
                format!("{component}.exe")
            } else {
                component.to_owned()
            };
            fs::write(root.join(name), b"binary").unwrap();
        }
        assert!(validate_toolchain_root(&root).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toolchain_list_ignores_interrupted_install_directories() {
        let root = temp_root("hidden-installs");
        fs::create_dir_all(root.join("toolchains/stable")).unwrap();
        fs::create_dir_all(root.join("toolchains/.stable.install-1")).unwrap();
        fs::create_dir_all(root.join("toolchains/.stable.backup-1")).unwrap();
        assert_eq!(list_toolchains(&root).unwrap(), ["stable"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn release_download_allows_slow_connections() {
        assert!(HTTP_TIMEOUT >= std::time::Duration::from_secs(300));
    }

    #[test]
    fn github_client_rejects_plaintext_redirects() {
        assert!(github_agent().config().https_only());
    }

    #[test]
    fn reports_target_component_and_c_toolchain_readiness_separately() {
        let root = temp_root("target-status");
        let toolchain = root.join("toolchain");
        fs::create_dir_all(&toolchain).unwrap();
        let active = ActiveToolchain {
            name: "dev".into(),
            root: toolchain.clone(),
            reason: "test".into(),
        };
        let triple = SUPPORTED_TARGETS
            .into_iter()
            .find(|triple| *triple != target::host().unwrap())
            .unwrap();
        let component = target::component_root(&toolchain, triple);
        fs::create_dir_all(&component).unwrap();
        fs::write(component.join("runtime.c"), b"void runtime(void) {}\n").unwrap();
        fs::write(
            component.join("target.toml"),
            format!("schema = 1\ntriple = \"{triple}\"\nruntime = \"runtime.c\"\n"),
        )
        .unwrap();

        let status = target::status(&toolchain, triple).unwrap();
        assert!(status.installed);
        assert!(!status.c_toolchain_ready);

        let compiler = root.join(if cfg!(windows) { "clang.exe" } else { "clang" });
        let sysroot = root.join("sysroot");
        let sdk = root.join("sdk");
        let msvc = root.join("msvc");
        fs::write(&compiler, b"test").unwrap();
        for directory in [&sysroot, &sdk, &msvc] {
            fs::create_dir_all(directory).unwrap();
        }
        configure_target(
            &active,
            triple,
            Some(compiler),
            Some(sysroot),
            Some(sdk),
            Some(msvc),
        )
        .unwrap();
        let status = target::status(&toolchain, triple).unwrap();
        assert!(status.installed);
        assert!(status.c_toolchain_ready, "{}", status.reason);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn locates_the_single_source_archive_root() {
        let root = temp_root("source-archive-root");
        let source = root.join("riddle-commit");
        fs::create_dir_all(&source).unwrap();
        assert_eq!(single_directory(&root).unwrap(), source);
        fs::create_dir_all(root.join("unexpected")).unwrap();
        assert!(single_directory(&root).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
