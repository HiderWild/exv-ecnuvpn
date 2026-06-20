import { normalizeError } from '../stores/vpn'

export const PROTECTED_IMPORT_ERROR =
  '无法解密或读取受保护的配置文件。可能原因包括：导入口令不正确、文件已损坏、文件内容被修改，或配置格式不兼容。'
export const INVALID_IMPORT_ERROR =
  '导入文件不是有效的 EXV 配置文件。可能原因包括：文件已损坏、内容不完整，或配置格式不兼容。'

export type ImportEnvelope = {
  format: 'protected' | 'unprotected'
  data: string
}

function parseImportObject(text: string): Record<string, unknown> {
  const parsed = JSON.parse(text) as unknown
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(INVALID_IMPORT_ERROR)
  }
  return parsed as Record<string, unknown>
}

function isProtectedEnvelope(value: Record<string, unknown>) {
  return value.format === 'protected' &&
    typeof value.protected_format === 'string' &&
    typeof value.payload === 'string'
}

export function detectImportEnvelope(text: string): ImportEnvelope {
  let parsed: Record<string, unknown>
  try {
    parsed = parseImportObject(text)
  } catch {
    throw new Error(INVALID_IMPORT_ERROR)
  }

  if (isProtectedEnvelope(parsed)) {
    return { format: 'protected', data: text }
  }

  if (parsed.format === 'protected' && typeof parsed.data === 'string') {
    try {
      const inner = parseImportObject(parsed.data)
      if (isProtectedEnvelope(inner)) {
        return { format: 'protected', data: parsed.data }
      }
    } catch {
      throw new Error(PROTECTED_IMPORT_ERROR)
    }
  }

  if (parsed.format === 'unprotected' && typeof parsed.data === 'string') {
    return { format: 'unprotected', data: parsed.data }
  }

  return { format: 'unprotected', data: text }
}

export function importEnvelopeToPayload(envelope: ImportEnvelope, password?: string) {
  return {
    format: envelope.format,
    data: envelope.data,
    password: envelope.format === 'protected' ? password : undefined,
  }
}

export function friendlyImportConfigError(error: unknown) {
  const code = error && typeof error === 'object' && 'code' in error ? String((error as { code?: unknown }).code) : ''
  const raw = error instanceof Error ? error.message : String(error || '')
  const normalized = normalizeError(error).message
  const text = `${code} ${raw} ${normalized}`.toLowerCase()
  if (
    code === 'config_import_tampered_or_wrong_password' ||
    text.includes('protected config') ||
    (text.includes('protected') && text.includes('decrypt')) ||
    text.includes('tampered') ||
    text.includes('parse error') ||
    raw === PROTECTED_IMPORT_ERROR
  ) {
    return PROTECTED_IMPORT_ERROR
  }
  if (code === 'invalid_config' || normalized === INVALID_IMPORT_ERROR || raw === INVALID_IMPORT_ERROR) {
    return INVALID_IMPORT_ERROR
  }
  return normalized
}
