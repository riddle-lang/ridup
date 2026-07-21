# ridup

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

## Project selection

Pin a project with `riddle-toolchain.toml`:

```toml
[toolchain]
channel = "0.1.1-x86_64-pc-windows-msvc"
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

Remote download and C compiler installation are intentionally not implemented
yet. `toolchain link` provides the version-selection contract while the release
archive manifest and download verification format are still being defined.
