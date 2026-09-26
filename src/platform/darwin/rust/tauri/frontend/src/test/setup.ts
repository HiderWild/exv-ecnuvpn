import { afterEach } from 'vitest';

// Darwin 宿主 Node ≥ 22 在 globalThis 上预注册了实验性 localStorage 槽（未提供
// --localstorage-file 时值为 undefined），vitest 的 happy-dom 环境会因「键已存在」
// 跳过窗口 localStorage 的注入。测试环境在此用 happy-dom 自带的 localStorage 实现
// 补齐同一全局（与浏览器同源实现；仅影响测试运行时，不影响产品代码）。
if (globalThis.localStorage === undefined) {
  const { Window } = await import('happy-dom');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: new Window().localStorage,
  });
}

afterEach(() => {
  document.body.innerHTML = '';
  localStorage.clear();
});
