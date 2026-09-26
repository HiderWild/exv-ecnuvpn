// 极简连接表单的凭据决策（纯函数，无 IO）。
//
// 「记住」= 设置页的「记住密码」（core key `remember_password`）。三条语义分支
// （用户逐字描述，见决策记录 2026-09-12-appearance-instant-commit.md）：
//   1. 勾选 + 密码非空 → 把设置中的密码更新成这个密码（persist=true）。
//   2. 未勾选 + 密码非空 → 本次用这个密码连接（persist=false），并在**连接成功后**
//      清除设置中保存的密码（清除时机与失败保留见下方说明）。
//   3. 密码框未动（留空）→ 不动设置，由 host 回落到已保存密文。
//
// 为什么清除不能靠 persist=false：core 的 `persist_ui_credentials` 明确
// `persist=false` 不改磁盘用户名、已有密文或 remember 标记。清除必须走既有
// 受支持的 `ConfigSet` 路径（`password: ""` + `remember_password: "false"`），
// `config_set` 在 `!remember_password` 时会清零并清空密码——设置页与快速入门已在使用。
//
// 判断项（可翻转）：清除只在连接成功之后执行。理由是清除不可逆，失败时保留已保存
// 密码用户还能重试；在失败路径上破坏凭据会让重试更难。用户未就此提问。

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
  /** 主按钮是否禁用（空密码且无已保存密码）。 */
  blocked: boolean;
  /** 禁用/提示文案；未禁用时为空字符串。 */
  blockedHint: string;
  /** 传给 runtime.connect 的凭据；null = 不传（host 回落已保存密文）。 */
  credentials: ConnectCredentials | null;
  /** 连接成功后是否清除设置中保存的密码（分支 2）。 */
  clearStoredAfterSuccess: boolean;
}

/** 分支 1/2/3 的唯一决策源；视图只消费结果，不自行猜测。 */
export function planMinimalCredentials(
  draft: MinimalCredentialDraft,
  inputs: MinimalCredentialInputs,
): MinimalCredentialPlan {
  const username = draft.username.trim();
  const password = draft.password;
  if (!username) return { blocked: true, blockedHint: "请输入账户", credentials: null, clearStoredAfterSuccess: false };

  // 分支 2：未勾选且填了本次密码 → 本次使用，成功后清除已保存密码。
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

  // 分支 3：密码框未动 → 不动设置，由 host 回落已保存密文。
  if (inputs.hasStoredPassword && (inputs.storedUsername === undefined || username === inputs.storedUsername.trim())) {
    return { blocked: false, blockedHint: "", credentials: null, clearStoredAfterSuccess: false };
  }

  // 空密码且没有已保存密码：禁用主按钮并提示，避免必然失败的操作。
  return {
    blocked: true,
    blockedHint: "请输入密码",
    credentials: null,
    clearStoredAfterSuccess: false,
  };
}

/** 分支 2 的清除载荷：core `config_set` 在该组合下清零并清空密码。 */
export function clearedCredentialConfig(): ReadonlyArray<{ key: "password" | "remember_password"; value: string }> {
  return [
    { key: "password", value: "" },
    { key: "remember_password", value: "false" },
  ];
}
