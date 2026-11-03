// 极简连接表单的凭据决策（纯函数，无 IO）。
//
// 「记住」= 设置页的「记住密码」（core key `remember_password`）。语义分支（D1/D3
// 沿用 2026-10-03 裁定；D2 按 2026-10-26 验收计划 S5a"已裁定项 2"与 darwin 统一）：
//   1. 勾选 + 密码非空 → 把设置中的密码更新成这个密码（persist=true）。
//   2. 未勾选 + 密码非空 → 本次用这个密码连接（persist=false），并在**连接成功后**
//      清除设置中保存的密码（成功后清除：尝试失败期间保留已存密码，失败重试更
//      友好；与 darwin `clearStoredAfterSuccess` 同构）。
//   3. 勾选 + 密码留空 + 有匹配的已保存密码 → 不动设置，由 host 回落已保存密文。
//   4. 其余留空（未勾选留空 / 无已保存密码 / 用户名与已保存账户不符）→ 拒绝连接。
//
// D1：「记住」勾选是纯草稿，只在点击连接时生效；取消保存 + 本次一次性凭据连接。
// D3：未勾选 + 密码留空 → 拒绝连接，不回落磁盘旧密码。
//
// 为什么清除不能靠 persist=false：core 的 `persist_ui_credentials` 明确
// `persist=false` 不改磁盘用户名、已有密文或 remember 标记。清除必须走既有
// 受支持的 `ConfigSet` 路径（`password: ""` + `remember_password: "false"`），
// `config_set` 在 `!remember_password` 时会清零并清空密码——设置页与快速入门已在使用。

import type { ConnectCredentials } from "../lib/ipc";

export interface MinimalCredentialDraft {
  username: string;
  password: string;
  remember: boolean;
}

export interface MinimalCredentialInputs {
  /** Core 验证后的凭据可用状态，与记住开关分离。 */
  hasStoredPassword: boolean;
  storedUsername?: string;
}

export interface MinimalCredentialPlan {
  /** 主按钮是否禁用（仅账户名为空时禁用；密码问题走「点击后拒绝并提示」）。 */
  blocked: boolean;
  /** 拒绝原因文案；未拒绝时为空字符串。 */
  blockedHint: string;
  /** 传给 runtime.connect 的凭据；null = 不传（host 回落已保存密文）。 */
  credentials: ConnectCredentials | null;
  /** 连接成功后是否清除设置中保存的密码（分支 2；失败期间保留便于重试）。 */
  clearStoredAfterSuccess: boolean;
}

/** 分支 1/2/3/4 的唯一决策源；视图只消费结果，不自行猜测。 */
export function planMinimalCredentials(
  draft: MinimalCredentialDraft,
  inputs: MinimalCredentialInputs,
): MinimalCredentialPlan {
  const username = draft.username.trim();
  const password = draft.password;
  // 账户名为空：按钮禁用，避免无意义的必然失败。
  if (!username) return { blocked: true, blockedHint: "请输入账户", credentials: null, clearStoredAfterSuccess: false };

  // 分支 2（D1）：未勾选且填了本次密码 → 本次使用，连接成功后清除已保存密码。
  if (!draft.remember && password) {
    return {
      blocked: false,
      blockedHint: "",
      credentials: { username, password, persist: false },
      clearStoredAfterSuccess: true,
    };
  }

  // 分支 1：勾选且填了密码 → 更新设置中的密码。
  if (draft.remember && password) {
    return {
      blocked: false,
      blockedHint: "",
      credentials: { username, password, persist: true },
      clearStoredAfterSuccess: false,
    };
  }

  // 分支 3：勾选、密码留空、用户名匹配已保存账户 → 由 host 回落已保存密文。
  if (draft.remember && inputs.hasStoredPassword
    && (inputs.storedUsername === undefined || username === inputs.storedUsername.trim())) {
    return { blocked: false, blockedHint: "", credentials: null, clearStoredAfterSuccess: false };
  }

  // 分支 4（D3）：未勾选留空 / 无已保存密码 / 用户名不匹配 → 拒绝连接，不回落旧密码。
  return {
    blocked: true,
    blockedHint: "未输入密码",
    credentials: null,
    clearStoredAfterSuccess: false,
  };
}

/** 分支 2 的清除载荷：core `config_set` 在该组合下清零并清空密码。 */
export function clearedCredentialConfig(): ReadonlyArray<{ key: "password" | "remember_password" | "auto_reconnect"; value: string }> {
  return [
    { key: "password", value: "" },
    { key: "remember_password", value: "false" },
    { key: "auto_reconnect", value: "false" },
  ];
}
