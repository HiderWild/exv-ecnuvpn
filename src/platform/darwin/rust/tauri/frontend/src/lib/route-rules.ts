/** 沿用设置的配置列表口径；不去重、合并或重新校验规则。 */
export function configuredRoutes(value: string | undefined): string[] {
  return (value ?? "").split(",").map(item => item.trim()).filter(Boolean);
}
