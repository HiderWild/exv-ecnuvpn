async (path, probeCookie) => {
  const hex = value => Array.from(new TextEncoder().encode(value))
    .map(byte => byte.toString(16).padStart(2, '0')).join('');
  try {
    const response = await fetch(path, {credentials: 'include'});
    const body = await response.json();
    const state = [body?.code === 0 ? '1' : '0',
      (body?.data?.isLogin === 1 || body?.data?.isLogin === true) ? '1' : '0',
      hex(body?.data?.firstAuth?.[0] || '')].join('-');
    document.cookie = `${probeCookie}=${state}; Path=/; Secure; SameSite=Lax`;
  } catch {}
}
