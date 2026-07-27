<h1 align="center">ridup</h1>

<h3 align="center">
    <a href="README-en.md">English</a> | <a href="README.md">中文</a>
</h3>

Ridup selects and runs installed Riddle toolchains. It manages Riddle versions,
target components, and C-toolchain configuration for cross-compilation.

## Local toolchains

Link an unpacked release or a local build directory, then make it the default:

```powershell
ridup toolchain link dev D:\Code\riddle\target\debug
ridup default dev
ridup show
ridup run dev clue --version
```

`ridup toolchain list` lists linked toolchains. A linked directory may contain
components directly, as Cargo's `target/debug` does, or under `bin/`.

## Release channels

Riddle has three release channels:

| Channel | Source | Intended use |
| --- | --- | --- |
| `stable` | Latest formal [GitHub Release](https://github.com/riddle-lang/riddle/releases/latest) | Daily use with full validation |
| `nightly` | Daily [Nightly Release](https://github.com/riddle-lang/riddle/releases/tag/nightly) | Trying the latest changes collected that day |
| `canary` | Latest source compiled locally | Earliest validation of new commits; may break at any time |

Install the desired channels directly:

```powershell
ridup toolchain install stable
ridup toolchain install nightly
ridup toolchain install canary
ridup default stable
```

Running an install command again updates that channel. For `stable` and
`nightly`, ridup selects the host release archive and verifies GitHub's SHA-256
digest before replacing the previous toolchain. For `canary`, ridup downloads
the latest `main` commit, runs `cargo build --workspace --release` locally, and
installs `clue`, `riddlec`, and `riddle-lsp`. Installing `canary` therefore
requires Rust and Cargo, but not Git.

The actual toolchain directory uses the full host triple, for example
`stable-x86_64-pc-windows-msvc`. `stable`, `nightly`, and `canary` remain
convenient aliases for the corresponding host toolchain.

Downloads and Canary builds honor standard proxy environment variables:

```powershell
$env:HTTPS_PROXY = "http://127.0.0.1:7890"
ridup toolchain install stable
```

## Targets and cross-compilation

Install, inspect, or remove a target component for the active toolchain:

```powershell
ridup target add aarch64-unknown-linux-gnu
ridup target list
ridup target remove aarch64-unknown-linux-gnu
```

`target add` reports the installed Riddle runtime, release version, and
LLVM/Clang baseline, then asks whether to install the matching C compiler. Use
`--yes` to confirm automatically in a non-interactive environment. Declining
does not remove the target component.

The first release supports only these seven triples. Every other value is
rejected:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `i686-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`
- `i686-pc-windows-msvc`
- `aarch64-pc-windows-msvc`
- `aarch64-apple-darwin`

The target component and C toolchain are separate states. `ridup target list`
reports `component=installed|missing` and `c-toolchain=ready|missing`
independently. Install or configure the C toolchain separately:

```powershell
ridup c-toolchain install aarch64-unknown-linux-gnu
ridup target configure aarch64-unknown-linux-gnu --compiler C:\LLVM\bin\clang.exe --sysroot D:\sysroots\aarch64-linux-gnu
```

Ridup reuses an existing Clang first. If none is found, it attempts to install
the distribution's LLVM 22.1.3 baseline with `winget` on Windows, `apt-get` on
Debian/Ubuntu, or Homebrew on macOS. If that package/version is unavailable,
the target component remains installed and ridup prints the manual
configuration command.

Clang/LLD does not include every target system library. Linux cross targets
also need a sysroot; producing MSVC programs from a non-Windows host needs the
Windows SDK and MSVC libraries; producing macOS programs needs an Apple SDK.
Ridup does not redistribute proprietary SDKs and does not mark a target ready
while its SDK/sysroot is missing. Every target command accepts
`--toolchain <name>`.

## Project selection

Pin a project with `riddle-toolchain.toml`:

```toml
[toolchain]
channel = "canary"
```

Selection precedence is:

1. A proxy argument such as `clue +dev build`.
2. `RIDUP_TOOLCHAIN`.
3. The nearest `ridup override set <toolchain>` directory override.
4. The nearest `riddle-toolchain.toml`.
5. The default toolchain.

When the ridup executable is installed under the names `clue`, `riddlec`, and
`riddle-lsp`, it acts as a proxy, executes that component from the selected
toolchain, and passes the toolchain root to Clue so it can load installed targets
and C configuration. Release packaging or an installer should create those
proxy copies or hard links.
