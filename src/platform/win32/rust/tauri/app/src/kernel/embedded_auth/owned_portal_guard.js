// 自有WebView doc-start / all frames：只阻断R1及门户SDK实证的官方本机API。
(() => {
  const ports = new Set(['54630', '54641', '54652', '54631', '54642', '54653']);
  const paths = new Set([
    '/v1/detect', '/v1/service/init', '/v1/service/start', '/v1/service/online',
    '/v1/service/offline', '/v1/service/logout', '/v1/service/status', '/v1/service/events',
    '/v1/service/checkMITMAttack', '/v1/service/reportEnv', '/v1/service/reportEnvBeforeLogin',
    '/v1/service/PopupPhishingRiskConfirm', '/v1/update/check', '/v1/update/status', '/v1/update/start',
    '/v1/base/setinfo', '/v1/base/getter', '/v1/base/setter', '/v1/base/printLog'
  ]);
  const official = input => {
    try {
      const url = new URL(typeof input === 'string' ? input : input.url, location.href);
      return url.protocol === 'https:' &&
        (url.hostname === '127.0.0.1' || url.hostname === 'localhost.sangfor.com.cn') &&
        ports.has(url.port) && paths.has(url.pathname);
    } catch (_) { return false; }
  };
  const localApiFailures = new WeakSet();
  window.__exvIsOwnedLocalApiFailure = error => localApiFailures.has(error);
  const fetchOriginal = window.fetch;
  if (fetchOriginal) window.fetch = function(input, ...args) {
    if (official(input)) {
      const error = new TypeError('EXV owned authentication handles this local API');
      localApiFailures.add(error);
      return Promise.reject(error);
    }
    return Reflect.apply(fetchOriginal, this, [input, ...args]);
  };
  const prototype = XMLHttpRequest.prototype;
  const openOriginal = prototype.open;
  const sendOriginal = prototype.send;
  const abortOriginal = prototype.abort;
  const requests = new WeakMap();
  const fields = ['readyState', 'status', 'statusText', 'response', 'responseText', 'responseXML', 'responseURL'];
  const emit = (xhr, type) => xhr.dispatchEvent(new ProgressEvent(type));
  prototype.open = function(method, url, ...args) {
    const previous = requests.get(this);
    if (previous) {
      previous.sent = false;
      requests.delete(this);
      for (const field of fields) delete this[field];
    }
    const blocked = official(url);
    const state = { readyState: 1, sent: false, async: args[0] !== false };
    if (blocked) requests.set(this, state);
    let result;
    try { result = Reflect.apply(openOriginal, this, [method, url, ...args]); }
    catch (error) { requests.delete(this); throw error; }
    if (blocked && requests.get(this) === state) {
      const values = { status: 0, statusText: '', response: '', responseText: '', responseXML: null, responseURL: '' };
      for (const field of fields) Object.defineProperty(this, field, {
        configurable: true,
        get: () => field === 'readyState' ? state.readyState : values[field],
      });
    }
    return result;
  };
  prototype.send = function(...args) {
    const state = requests.get(this);
    if (!state) return Reflect.apply(sendOriginal, this, args);
    if (state.readyState !== 1 || state.sent) throw new DOMException('The request is not opened', 'InvalidStateError');
    if (!state.async) {
      state.readyState = 4;
      throw new DOMException('Network request failed', 'NetworkError');
    }
    state.sent = true;
    setTimeout(() => {
      if (requests.get(this) !== state || !state.sent) return;
      state.sent = false;
      state.readyState = 4;
      emit(this, 'readystatechange');
      // SDK 的失败处理会在 DONE 回调内 abort；仍完成本轮 error/loadend。
      if (requests.get(this) !== state) return;
      emit(this, 'error');
      if (requests.get(this) === state) emit(this, 'loadend');
    }, 0);
  };
  prototype.abort = function(...args) {
    const state = requests.get(this);
    if (!state) return Reflect.apply(abortOriginal, this, args);
    const pending = state.sent;
    state.sent = false;
    Reflect.apply(abortOriginal, this, args);
    if (pending) {
      state.readyState = 4;
      emit(this, 'readystatechange');
      if (requests.get(this) !== state) return;
      emit(this, 'abort');
      if (requests.get(this) !== state) return;
      emit(this, 'loadend');
    }
    if (requests.get(this) === state) state.readyState = 0;
  };
})();
