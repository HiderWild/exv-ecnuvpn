import { readonly, ref, type InjectionKey, type Ref } from "vue";

import type { ServiceControlAction, ServiceControlReply } from "../lib/ipc";

export type ServiceOperationAction = ServiceControlAction | "repair";

export interface ServiceOperationCoordinator {
  readonly busy: Readonly<Ref<boolean>>;
  readonly label: Readonly<Ref<string>>;
  run(action: ServiceOperationAction): Promise<ServiceControlReply>;
}

export const SERVICE_OPERATION_KEY: InjectionKey<ServiceOperationCoordinator> = Symbol("service-operations");

const OPERATION_LABEL: Record<Exclude<ServiceOperationAction, "query">, string> = {
  install: "正在安装服务…",
  uninstall: "正在卸载服务…",
  start: "正在启动服务…",
  rotate_key: "正在轮换服务密钥…",
  repair: "正在修复服务…",
};

export function createServiceOperationCoordinator(
  control: (action: ServiceControlAction) => Promise<ServiceControlReply>,
): ServiceOperationCoordinator {
  const busy = ref(false);
  const label = ref("");
  const pending: Array<{
    action: ServiceControlAction;
    resolve: (reply: ServiceControlReply) => void;
    reject: (error: unknown) => void;
  }> = [];
  let running = false;

  const drain = (): void => {
    if (running) return;
    const next = pending.shift();
    if (!next) return;
    running = true;
    let operation: Promise<ServiceControlReply>;
    try {
      operation = control(next.action);
    } catch (error) {
      running = false;
      next.reject(error);
      drain();
      return;
    }
    void operation.then(
      (reply) => {
        next.resolve(reply);
        running = false;
        drain();
      },
      (error) => {
        next.reject(error);
        running = false;
        drain();
      },
    );
  };

  const enqueue = (action: ServiceControlAction): Promise<ServiceControlReply> => new Promise(
    (resolve, reject) => {
      pending.push({ action, resolve, reject });
      drain();
    },
  );

  return {
    busy: readonly(busy),
    label: readonly(label),
    async run(action: ServiceOperationAction): Promise<ServiceControlReply> {
      if (action === "query") return enqueue("query");
      if (busy.value) {
        throw { kind: "service_operation_busy", message: "另一项服务操作正在进行。" };
      }
      busy.value = true;
      label.value = OPERATION_LABEL[action];
      try {
        return await enqueue(action === "repair" ? "install" : action);
      } finally {
        busy.value = false;
        label.value = "";
      }
    },
  };
}
