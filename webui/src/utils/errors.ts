export function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error
  if (error && typeof error === 'object') {
    const anyError = error as {
      message?: string
      response?: { data?: { error?: string; message?: string } }
    }
    const responseMessage = anyError.response?.data?.error || anyError.response?.data?.message
    if (responseMessage) return responseMessage
    if (anyError.message) return anyError.message
  }
  return 'Operation failed'
}

export function classifyErrorMessage(msg: string): string {
  const lower = msg.toLowerCase()
  if (lower.includes('administrator') || lower.includes('denied') || lower.includes('cancel') || lower.includes('not allowed') || lower.includes('elevation_denied')) {
    return 'elevation_denied'
  }
  if (lower.includes('openconnect') || lower.includes('runtime') || lower.includes('not found')) {
    return 'runtime_missing'
  }
  if (lower.includes('config') || lower.includes('no server') || lower.includes('no username')) {
    return 'config_missing'
  }
  if (lower.includes('helper') || lower.includes('socket') || lower.includes('launchd') || lower.includes('daemon')) {
    return 'helper_unavailable'
  }
  if (lower.includes('route') || lower.includes('cleanup')) {
    return 'cleanup_failed'
  }
  if (lower.includes('connect') || lower.includes('timeout')) {
    return 'connect_failed'
  }
  return 'unknown'
}