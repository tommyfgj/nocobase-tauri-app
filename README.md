# NocoBase Tauri App

将 NocoBase 2.1.24 和只读 MySQL/Doris 数据源插件封装为 macOS Tauri
桌面应用。应用在本机固定端口 `14300` 启动 NocoBase，并安装
`~/.nocobase-desktop/bin/nb` 供外部 Agent 调用。

## 源码依赖

仓库通过 Git submodule 跟踪：

- `vendor/nocobase`：NocoBase 源码
- `vendor/plugin-data-source-readonly-mysql`：只读 SQL 插件源码

克隆时执行：

```bash
git clone --recurse-submodules https://github.com/tommyfgj/nocobase-tauri-app.git
```

## 构建 macOS DMG

需要 macOS、Rust、Node.js、Yarn 和 Tauri 构建依赖。

```bash
yarn install
./scripts/prepare-resources.sh
yarn tauri build
```

`prepare-resources.sh` 会下载固定版本的 Node.js arm64 可执行文件，安装
NocoBase 运行时依赖并生成 `src-tauri/resources/runtime.tar.gz`。这些大型生成
文件不会提交到 Git。

仓库不包含证书身份、entitlements、签名或 Apple 公证凭据。发布者应在本地或
私有 CI 中注入自己的签名配置。

## 数据目录

运行时、数据库配置和日志保存在 `~/.nocobase-desktop`。数据库密码不会写入
本仓库。
