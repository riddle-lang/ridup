<h1 align="center">ridup</h1>

<h3 align="center">
    <a href="README-en.md">English</a> | <a href="README.md">中文</a>
</h3>

`ridup` 用于选择并运行已安装的 Riddle 工具链，负责管理 Riddle 版本；被选中的 `clue` 仍负责寻找当前机器上的 C 编译器。

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

重复执行安装命令即可更新对应通道。`stable` 和 `nightly` 会自动选择当前系统的发布归档，验证 GitHub 提供的 SHA-256 后再替换旧工具链。`canary` 会下载 `main` 最新提交的源码，在本机执行 `cargo build --workspace --release`，然后安装 `clue`、`riddlec` 和 `riddle-lsp`；因此安装 `canary` 需要本机已有 Rust 和 Cargo，不需要 Git。

下载和 Canary 构建都会使用标准代理环境变量：

```powershell
$env:HTTPS_PROXY = "http://127.0.0.1:7890"
ridup toolchain install stable
```

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

当把 ridup 可执行文件安装为 `clue`、`riddlec` 或 `riddle-lsp` 时，它会作为代理，从选中的工具链中执行对应组件。发行打包或安装器需要创建这些代理副本或硬链接。

C 编译器安装目前尚未实现；被选中的 `clue` 仍使用当前机器上已有的 C 编译器。
