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
