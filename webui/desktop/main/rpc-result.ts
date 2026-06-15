export function normalizeRpcSuccessResult(result: unknown): unknown {
  if (!result || typeof result !== 'object' || Array.isArray(result)) {
    return result
  }

  const obj = result as Record<string, unknown>
  if (obj.ok !== true) {
    return result
  }

  if (Object.prototype.hasOwnProperty.call(obj, 'data')) {
    return obj.data
  }

  if (Object.prototype.hasOwnProperty.call(obj, 'id')) {
    return result
  }

  const { ok: _ok, ...payload } = obj
  return payload
}
