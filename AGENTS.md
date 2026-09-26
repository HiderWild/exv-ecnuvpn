# 贡献与维护须知

## 构建

- 一键发布构建：`scripts/build-release.ps1`（前置环境见 `README.md`）。
- 构建脚本只消费已提交内容；提交前请确认本地构建通过。

## 代码约定

- Rust：遵守仓库 `rustfmt.toml`；不引入 `unsafe`（平台 crate 除外，需附
  SAFETY 说明）；注释与文档以中文为主，标识符与协议字面量保持原文。
- 前端：Vue 3 + TypeScript，构建以 `vite` 为准。

## 流程约定

- 不使用 CI 强制门禁；提交者自行保证构建与基本回归。
- 发布版本以 `CHANGELOG.md` 为准，版本号来源为
  `src/platform/win32/rust/tauri/app/tauri.conf.json` 的 `version`。
