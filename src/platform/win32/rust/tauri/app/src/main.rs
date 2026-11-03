#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // C6 观测加固（2026-10-09 区域 C 计划 §2.6 / 复审 M3-c）：入口 canary——不依赖
    // 任何插件与 logger（fern/tauri_plugin_log），直接写 stderr。覆盖两个 log canary
    // 结构性够不着的盲区：(1) 第二实例在插件初始化期早退（到不了 `.setup`，那边
    // 的 log canary 不执行）；(2) logger 自身初始化失败（与「进程没走到 setup」在
    // log canary 缺席上同构，本行可区分两层）。stderr 句柄已被实测残留行（core
    // 子进程 stderr=inherit 汇入）证明存活；release（windows 子系统）下无句柄时
    // std 按约定把写入当 no-op 静默丢弃，无副作用。判读：本行在=进程到达入口；
    // setup 的 `ui process setup reached` 在=setup 已到且 logger 存活；两者都在而
    // stdout 无输出=纯 stdout 句柄问题（H-A/H-C）。
    eprintln!(
        "exv.bootstrap: ui process entry pid={} profile={}",
        std::process::id(),
        if cfg!(debug_assertions) { "debug" } else { "release" }
    );
    exv_ui::run()
}
