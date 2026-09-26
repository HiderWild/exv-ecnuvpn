import type { ComputedRef, InjectionKey, Ref } from "vue";
import type { ProductConnectionAction } from "./connection-action";
import type { ProductUiState } from "./types";

/** 应用级交互上下文；只保存界面操作，不取代运行时连接策略。 */
export interface ConnectionInteraction {
  state: ComputedRef<ProductUiState | null>;
  action: ComputedRef<ProductConnectionAction>;
  busy: ComputedRef<boolean>;
  autoInstallService: Ref<boolean>;
  runAction: () => Promise<void>;
}

export const CONNECTION_INTERACTION_KEY: InjectionKey<ConnectionInteraction> = Symbol("exv.connection.interaction");
