use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const LLVM_VERSION: &str = "22.1.3";
pub const SUPPORTED_TARGETS: [&str; 7] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "i686-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "i686-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "aarch64-apple-darwin",
];

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CToolchainConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysroot: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_sdk: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msvc: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComponentManifest {
    pub schema: u32,
    pub triple: String,
    pub runtime: PathBuf,
    #[serde(default)]
    pub llvm_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetStatus {
    pub triple: &'static str,
    pub installed: bool,
    pub c_toolchain_ready: bool,
    pub reason: String,
}

pub fn validate(triple: &str) -> anyhow::Result<&'static str> {
    SUPPORTED_TARGETS
        .into_iter()
        .find(|candidate| *candidate == triple)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unsupported target `{triple}`; supported targets: {}",
                SUPPORTED_TARGETS.join(", ")
            )
        })
}

pub fn host() -> anyhow::Result<&'static str> {
    let triple = match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86") => "i686-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", "x86") => "i686-pc-windows-msvc",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => {
            bail!(
                "unsupported host platform `{}/{}`; supported hosts: {}",
                env::consts::OS,
                env::consts::ARCH,
                SUPPORTED_TARGETS.join(", ")
            )
        }
    };
    Ok(triple)
}

pub fn component_root(toolchain_root: &Path, triple: &str) -> PathBuf {
    toolchain_root.join("targets").join(triple)
}

pub fn component_manifest(toolchain_root: &Path, triple: &str) -> PathBuf {
    component_root(toolchain_root, triple).join("target.toml")
}

pub fn load_component(
    toolchain_root: &Path,
    triple: &str,
) -> anyhow::Result<Option<ComponentManifest>> {
    let path = component_manifest(toolchain_root, triple);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read `{}`", path.display()));
        }
    };
    validate_component_root(&component_root(toolchain_root, triple), triple, &text).map(Some)
}

pub fn validate_component_root(
    root: &Path,
    triple: &str,
    manifest_text: &str,
) -> anyhow::Result<ComponentManifest> {
    let path = root.join("target.toml");
    let manifest: ComponentManifest = toml::from_str(manifest_text)
        .with_context(|| format!("invalid target component `{}`", path.display()))?;
    if manifest.schema != 1 {
        bail!(
            "unsupported target component schema {} in `{}`",
            manifest.schema,
            path.display()
        );
    }
    if manifest.triple != triple {
        bail!(
            "target component `{}` describes `{}` instead of `{triple}`",
            path.display(),
            manifest.triple
        );
    }
    if !safe_relative(&manifest.runtime) {
        bail!(
            "target component runtime path `{}` is not a safe relative path",
            manifest.runtime.display()
        );
    }
    let runtime = root.join(&manifest.runtime);
    if !runtime.is_file() {
        bail!(
            "target component `{triple}` is incomplete; missing runtime `{}`",
            runtime.display()
        );
    }
    Ok(manifest)
}

pub fn load_c_toolchain(toolchain_root: &Path, triple: &str) -> anyhow::Result<CToolchainConfig> {
    let path = component_root(toolchain_root, triple).join("c-toolchain.toml");
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid C toolchain config `{}`", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CToolchainConfig::default())
        }
        Err(error) => Err(error).with_context(|| format!("failed to read `{}`", path.display())),
    }
}

pub fn save_c_toolchain(
    toolchain_root: &Path,
    triple: &str,
    config: &CToolchainConfig,
) -> anyhow::Result<PathBuf> {
    validate(triple)?;
    let root = component_root(toolchain_root, triple);
    fs::create_dir_all(&root)?;
    let path = root.join("c-toolchain.toml");
    fs::write(&path, toml::to_string_pretty(config)?)
        .with_context(|| format!("failed to write `{}`", path.display()))?;
    Ok(path)
}

pub fn status(toolchain_root: &Path, triple: &'static str) -> anyhow::Result<TargetStatus> {
    let installed = load_component(toolchain_root, triple)?.is_some();
    let config = load_c_toolchain(toolchain_root, triple)?;
    let host = host()?;
    let compiler = config
        .compiler
        .clone()
        .filter(|path| compiler_available(path))
        .or_else(|| (triple == host).then(find_host_c_compiler).flatten());
    let c_toolchain_ready = compiler.is_some()
        && (!requires_sysroot(triple, host) || config.sysroot.as_deref().is_some_and(Path::is_dir))
        && (!requires_windows_sdk(triple, host)
            || (config.windows_sdk.as_deref().is_some_and(Path::is_dir)
                && config.msvc.as_deref().is_some_and(Path::is_dir)));
    let reason = if triple == host && compiler.is_some() {
        "native host target; Clue detects the C compiler automatically".to_owned()
    } else if !installed && triple != host {
        format!("not installed; run `ridup target add {triple}`")
    } else if compiler.is_none() {
        format!("C compiler missing; run `ridup c-toolchain install {triple}`")
    } else if requires_sysroot(triple, host) && !config.sysroot.as_deref().is_some_and(Path::is_dir)
    {
        format!("sysroot missing; run `ridup target configure {triple} --sysroot <path>`")
    } else if requires_windows_sdk(triple, host)
        && (!config.windows_sdk.as_deref().is_some_and(Path::is_dir)
            || !config.msvc.as_deref().is_some_and(Path::is_dir))
    {
        format!(
            "Windows SDK/MSVC paths missing; run `ridup target configure {triple} --windows-sdk <path> --msvc <path>`"
        )
    } else {
        "ready".to_owned()
    };
    Ok(TargetStatus {
        triple,
        installed: installed || triple == host,
        c_toolchain_ready,
        reason,
    })
}

pub fn compiler_available(path: &Path) -> bool {
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file();
    }
    let Some(search_path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&search_path).any(|directory| {
        let direct = directory.join(path);
        direct.is_file()
            || (cfg!(windows) && directory.join(format!("{}.exe", path.display())).is_file())
    })
}

pub fn find_clang(triple: &str) -> Option<PathBuf> {
    let candidates = if cfg!(windows) && triple.ends_with("-pc-windows-msvc") {
        ["clang-cl-22", "clang-22", "clang-cl", "clang"]
    } else if cfg!(windows) {
        ["clang-22", "clang", "clang-cl-22", "clang-cl"]
    } else {
        ["clang-22", "clang", "clang-21", "clang-20"]
    };
    candidates.into_iter().find_map(locate_program).or_else(|| {
        if cfg!(windows) {
            env::var_os("ProgramFiles")
                .map(PathBuf::from)
                .map(|root| {
                    root.join(if triple.ends_with("-pc-windows-msvc") {
                        "LLVM/bin/clang-cl.exe"
                    } else {
                        "LLVM/bin/clang.exe"
                    })
                })
                .filter(|path| path.is_file())
        } else {
            None
        }
    })
}

fn find_host_c_compiler() -> Option<PathBuf> {
    if let Some(program) = env::var_os("CC") {
        return locate_program(&program.to_string_lossy());
    }
    let candidates = if cfg!(windows) {
        ["cl", "clang-cl", "clang", "gcc"]
    } else {
        ["cc", "clang", "gcc", "clang-22"]
    };
    candidates.into_iter().find_map(locate_program)
}

pub fn locate_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() && path.is_file() {
        return Some(path.to_path_buf());
    }
    let search_path = env::var_os("PATH")?;
    env::split_paths(&search_path).find_map(|directory| {
        let direct = directory.join(path);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) {
            let executable = directory.join(format!("{program}.exe"));
            if executable.is_file() {
                return Some(executable);
            }
        }
        None
    })
}

pub fn package_manager_command() -> Option<(&'static str, &'static [&'static str])> {
    if cfg!(windows) && locate_program("winget").is_some() {
        return Some((
            "winget",
            &[
                "install",
                "--id",
                "LLVM.LLVM",
                "--version",
                LLVM_VERSION,
                "--exact",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ],
        ));
    }
    if cfg!(target_os = "linux") && locate_program("apt-get").is_some() {
        return if locate_program("sudo").is_some() {
            Some(("sudo", &["apt-get", "install", "-y", "clang-22", "lld-22"]))
        } else {
            Some(("apt-get", &["install", "-y", "clang-22", "lld-22"]))
        };
    }
    if cfg!(target_os = "macos") && locate_program("brew").is_some() {
        return Some(("brew", &["install", "llvm@22", "lld"]));
    }
    None
}

pub fn requires_sysroot(triple: &str, host: &str) -> bool {
    triple != host && (triple.ends_with("-unknown-linux-gnu") || triple == "aarch64-apple-darwin")
}

pub fn requires_windows_sdk(triple: &str, host: &str) -> bool {
    triple.ends_with("-pc-windows-msvc") && triple != host
}

pub fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_exactly_the_supported_targets() {
        for triple in SUPPORTED_TARGETS {
            assert_eq!(validate(triple).unwrap(), triple);
        }
        assert!(validate("x86_64-unknown-linux-musl").is_err());
    }

    #[test]
    fn detects_cross_target_requirements() {
        assert!(requires_sysroot(
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu"
        ));
        assert!(!requires_sysroot(
            "x86_64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu"
        ));
        assert!(requires_windows_sdk(
            "x86_64-pc-windows-msvc",
            "x86_64-unknown-linux-gnu"
        ));
        assert!(requires_windows_sdk(
            "i686-pc-windows-msvc",
            "x86_64-pc-windows-msvc"
        ));
    }

    #[test]
    fn rejects_unsafe_runtime_paths() {
        assert!(safe_relative(Path::new("runtime.c")));
        assert!(!safe_relative(Path::new("../runtime.c")));
        assert!(!safe_relative(Path::new("/runtime.c")));
    }
}
