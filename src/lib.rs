use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

pub const PROJECT_TOOLCHAIN_FILE: &str = "riddle-toolchain.toml";

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
            if let Some(name) = entry.file_name().to_str() {
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
}
