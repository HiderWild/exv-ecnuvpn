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
- 产品版本唯一来源是根 `release.json`。改版使用 `python scripts/release.py version set <版本>`，核对使用 `version check --notes`；两端版本与用户更新摘要一并维护。
- `release-notes.json` 面向用户记录功能、体验和修复；关于页、CHANGELOG 与 Release 使用同一版本内容。
