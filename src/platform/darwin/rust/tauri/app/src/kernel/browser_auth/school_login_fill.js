(username, password, gatewayOrigin) => {
  try {
    // 一次同步采样；尚未挂载时由认证编排器的同一 tick 重试。
    if (location.origin !== 'https://sso.ecnu.edu.cn' || location.pathname !== '/login') {
      return 'not_ready';
    }
    const service = new URLSearchParams(location.search).get('service');
    if (!service) return 'service_mismatch';
    let serviceOrigin;
    try {
      serviceOrigin = new URL(service).origin;
    } catch {
      return 'service_mismatch';
    }
    if (serviceOrigin !== new URL(gatewayOrigin).origin) return 'service_mismatch';

    const visible = (element) => !!element && element.offsetParent !== null;
    const challengeVisible =
      Array.from(document.querySelectorAll('[id*="captcha" i], [class*="captcha" i]'))
        .some((element) => element.tagName !== 'INPUT' && visible(element)) ||
      Array.from(document.querySelectorAll('iframe'))
        .some((frame) => visible(frame) && /captcha/i.test(frame.src || ''));
    if (challengeVisible) return 'challenge_visible';

    const form = document.querySelector('form#normalLoginForm') ||
      document.querySelector('form[action*="/login"]');
    const user = form && form.querySelector('input[name="username"]');
    const pass = form && form.querySelector('input[type="password"]');
    const button = form && (form.querySelector('button#submitBtn') ||
      form.querySelector('button[type="submit"]') || form.querySelector('button'));
    if (!visible(user) || !visible(pass) || !visible(button) ||
        user.disabled || pass.disabled || button.disabled) return 'not_ready';

    const setFieldValue = (element, value) => {
      element.focus();
      element.value = value;
      element.dispatchEvent(new Event('input', { bubbles: true }));
      element.dispatchEvent(new Event('change', { bubbles: true }));
      element.blur();
    };
    setFieldValue(user, username);
    setFieldValue(pass, password);
    // 唯一一次点击；跨页面和跨通道的提交预算由编排器独占。
    button.click();
    return 'submitted';
  } catch {
    return 'failed';
  }
}
