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

## 项目选择

可以通过 `riddle-toolchain.toml` 固定项目使用的工具链：

```toml
[toolchain]
channel = "0.1.1-x86_64-pc-windows-msvc"
```

选择优先级如下：

1. 代理参数，例如 `clue +dev build`；
2. `RIDUP_TOOLCHAIN` 环境变量；
3. 最近的 `ridup override set <toolchain>` 目录覆盖；
4. 最近的 `riddle-toolchain.toml`；
5. 默认工具链。

当把 ridup 可执行文件安装为 `clue`、`riddlec` 或 `riddle-lsp` 时，它会作为代理，从选中的工具链中执行对应组件。发行打包或安装器需要创建这些代理副本或硬链接。

远程下载和 C 编译器安装目前尚未实现。`toolchain link` 先提供版本选择契约，等发行归档清单和下载校验格式确定后再加入远程安装。
