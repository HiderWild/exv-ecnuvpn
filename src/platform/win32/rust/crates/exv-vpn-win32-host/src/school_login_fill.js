async (username, password, gatewayOrigin) => {
  const SCHOOL_ORIGIN = 'https://sso.ecnu.edu.cn';
  const SCHOOL_PATH = '/login';
  if (location.origin !== SCHOOL_ORIGIN || location.pathname !== SCHOOL_PATH) {
    return 'not_school_page';
  }
  const service = new URLSearchParams(location.search).get('service');
  if (!service) return 'service_mismatch';
  try {
    if (new URL(service).origin !== gatewayOrigin) return 'service_mismatch';
  } catch {
    return 'service_mismatch';
  }
  // 学校页面脚本动态挂载普通表单（Angular）。杠杆 4（提速研究 2026-10-07 §二
  // 杠杆 4）：挂载等待由旧版四分之一秒定长轮询改为 MutationObserver 事件驱动
  // ——DOM 变更即时探测，控制可见（offsetParent）判定、超时预算（8 秒有界）与
  // 全部结果分类语义逐字保留。就绪判定与轮询版完全同构：form + 三控件齐备且可见。
  const findForm = () => {
    const form = document.querySelector('form#normalLoginForm') ||
      document.querySelector('form[action*="/login"]');
    const user = form && form.querySelector('input[name="username"]');
    const pass = form && form.querySelector('input[type="password"]');
    const button = form && (form.querySelector('button#submitBtn') ||
      form.querySelector('button[type="submit"]') || form.querySelector('button'));
    const visible = (el) => !!el && el.offsetParent !== null;
    return (form && user && pass && button && visible(user) && visible(pass) && visible(button))
      ? { user, pass, button }
      : null;
  };
  const controls = await new Promise((resolve) => {
    let settled = false;
    const finish = (found) => {
      if (settled) return;
      settled = true;
      if (observer) observer.disconnect();
      clearTimeout(timer);
      resolve(found);
    };
    // Angular 反复重挂（移除再插入节点）时 observer 会连续触发，多次探测幂等；
    // attributes:true 覆盖「节点已插入、稍后才变可见」的 class/style 切换。
    let observer = null;
    if (typeof MutationObserver === 'function' && document.documentElement) {
      observer = new MutationObserver(() => {
        const found = findForm();
        if (found) finish(found);
      });
      observer.observe(document.documentElement, {
        childList: true,
        subtree: true,
        attributes: true,
      });
    }
    // 8 秒有界上限兜底（与轮询版一致）：到期做最后一次判定，就绪则通过，
    // 否则由调用方按 form_not_ready 分类。
    const timer = setTimeout(() => finish(findForm()), 8000);
    // eval 时表单可能已挂载完毕（外层在到达学校页后才注入）：先探测一次。
    const immediate = findForm();
    if (immediate) finish(immediate);
  });
  if (!controls) return 'form_not_ready';
  // 已可见的验证码挑战：不填不点，整页交还人工。
  const challengeVisible =
    Array.from(document.querySelectorAll('[id*="captcha" i], [class*="captcha" i]'))
      .some((el) => el.tagName !== 'INPUT' && el.offsetParent !== null) ||
    Array.from(document.querySelectorAll('iframe'))
      .some((frame) => frame.offsetParent !== null && /captcha/i.test(frame.src || ''));
  if (challengeVisible) return 'challenge_visible';
  const setFieldValue = (element, value) => {
    element.focus();
    element.value = value;
    element.dispatchEvent(new Event('input', { bubbles: true }));
    element.dispatchEvent(new Event('change', { bubbles: true }));
    element.blur();
  };
  setFieldValue(controls.user, username);
  setFieldValue(controls.pass, password);
  // 预算内唯一一次提交：由学校页面自身的监听器与校验脚本接管后续。
  controls.button.click();
  return 'submitted';
}
