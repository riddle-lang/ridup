<h1 align="center">ridup</h1>

<h3 align="center">
    <a href="README-en.md">English</a> | <a href="README.md">中文</a>
</h3>

Ridup selects and runs installed Riddle toolchains. It manages Riddle versions;
the selected `clue` remains responsible for finding a host C compiler.

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

Downloads and Canary builds honor standard proxy environment variables:

```powershell
$env:HTTPS_PROXY = "http://127.0.0.1:7890"
ridup toolchain install stable
```

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
`riddle-lsp`, it acts as a proxy and executes that component from the selected
toolchain. Release packaging or an installer should create those proxy copies or
hard links.

C compiler installation is not implemented; the selected `clue` continues to
use a C compiler already available on the host.
