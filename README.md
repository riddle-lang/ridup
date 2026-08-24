<h1 align="center">ridup</h1>

<h3 align="center">
    <a href="README-en.md">English</a> | <a href="README.md">中文</a>
</h3>

`ridup` 用于选择并运行已安装的 Riddle 工具链，负责管理 Riddle 版本、目标组件以及交叉编译所需的 C 工具链配置。

## 本地工具链

可以链接已解压的发行版或本地构建目录，然后将它设为默认工具链：

```powershell
ridup toolchain link dev D:\Code\riddle\target\debug
ridup default dev
ridup show
ridup run dev clue --version
```

`ridup toolchain list` 会列出已链接的工具链。工具链目录可以像 Cargo 的 `target/debug` 一样直接包含组件，也可以把组件放在 `bin/` 下。

## 发布通道

Riddle 使用三个发布通道：

| 通道 | 来源 | 适用场景 |
| --- | --- | --- |
| `stable` | 最新正式 [GitHub Release](https://github.com/riddle-lang/riddle/releases/latest) | 日常使用，经过完整验证 |
| `nightly` | 每日 [Nightly Release](https://github.com/riddle-lang/riddle/releases/tag/nightly) | 提前试用当天汇总的最新改动 |
| `canary` | 用户本地编译的最新源码 | 最早验证最新提交，可能随时出现问题 |

直接安装所需通道：

```powershell
ridup toolchain install stable
ridup toolchain install nightly
ridup toolchain install canary
ridup default stable
```

重复执行安装命令即可更新对应通道。`stable` 和 `nightly` 会自动选择当前系统的发布归档，验证 GitHub 提供的 SHA-256 后再替换旧工具链。`canary` 会下载 `main` 最新提交的源码，在本机执行 `cargo build --workspace --release`，然后安装 `clue`、`riddlec`、`riddle` 和 `riddle-lsp`；因此安装 `canary` 需要本机已有 Rust 和 Cargo，不需要 Git。

工具链实际目录使用完整宿主 triple，例如 `stable-x86_64-pc-windows-msvc`；`stable`、`nightly` 和 `canary` 仍是指向对应宿主工具链的便捷名称。

其中 `riddle` 提供统一工具入口，当前可用 `riddle fmt` 格式化 Riddle 源码或检查格式。

下载和 Canary 构建都会使用标准代理环境变量：

```powershell
$env:HTTPS_PROXY = "http://127.0.0.1:7890"
ridup toolchain install stable
```

## 目标组件与交叉编译

给当前工具链安装、查看或删除目标组件：

```powershell
ridup target add aarch64-unknown-linux-gnu
ridup target list
ridup target remove aarch64-unknown-linux-gnu
```

`target add` 会显示安装的 Riddle runtime、发行版本和 LLVM/Clang 基线，然后询问是否安装匹配的 C 编译器。使用 `--yes` 可以在非交互环境中自动确认；不确认也会保留已经安装的目标组件。

首版只支持以下 7 个 triple，其他值会直接报错：

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `i686-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`
- `i686-pc-windows-msvc`
- `aarch64-pc-windows-msvc`
- `aarch64-apple-darwin`

目标组件和 C 工具链是两个独立状态。`ridup target list` 会分别显示 `component=installed|missing` 和 `c-toolchain=ready|missing`。可以单独安装或配置 C 工具链：

```powershell
ridup c-toolchain install aarch64-unknown-linux-gnu
ridup target configure aarch64-unknown-linux-gnu --compiler C:\LLVM\bin\clang.exe --sysroot D:\sysroots\aarch64-linux-gnu
```

ridup 优先复用系统已有的 Clang。缺失时会尝试安装发行清单指定的 LLVM 22.1.3：Windows 使用 `winget`，Debian/Ubuntu 使用 `apt-get`，macOS 使用 Homebrew；包管理器不可用或没有该版本时会保留目标组件，并给出手动配置命令。

Clang/LLD 本身不包含所有目标系统库。Linux 交叉目标还需要 sysroot；从非 Windows 宿主生成 MSVC 程序需要 Windows SDK 和 MSVC 库；生成 macOS 程序需要 Apple SDK。ridup 不会分发专有 SDK，也不会把缺少 SDK/sysroot 的目标标为就绪。所有目标命令都可用 `--toolchain <name>` 指定工具链。

## 项目选择

可以通过 `riddle-toolchain.toml` 固定项目使用的工具链：

```toml
[toolchain]
channel = "canary"
```

选择优先级如下：

1. 代理参数，例如 `clue +dev build`；
2. `RIDUP_TOOLCHAIN` 环境变量；
3. 最近的 `ridup override set <toolchain>` 目录覆盖；
4. 最近的 `riddle-toolchain.toml`；
5. 默认工具链。

当把 ridup 可执行文件安装为 `clue`、`riddlec`、`riddle` 或 `riddle-lsp` 时，它会作为代理，从选中的工具链中执行对应组件，并把工具链根目录传给 Clue，使其读取已安装的目标和 C 配置。发行打包或安装器需要创建这些代理副本或硬链接。
