// Injected before every plugin page. No dependencies or access to Tauri internals.
(() => {
  const channel = "cc-switch-plugin/v1";
  const pending = new Map();
  let sequence = 0;
  let context;
  const request = (method, value) =>
    new Promise((resolve, reject) => {
      const requestId = String(++sequence);
      const timeout = setTimeout(() => {
        pending.delete(requestId);
        reject(new Error("CC Switch plugin request timed out"));
      }, 10000);
      pending.set(requestId, { resolve, reject, timeout });
      parent.postMessage({ channel, requestId, method, value }, "*");
    });
  addEventListener("message", (event) => {
    if (event.source !== parent || event.data?.channel !== channel) return;
    const response = event.data;
    if (response.event === "healthCheck") {
      parent.postMessage(
        { channel, event: "healthReply", challenge: response.challenge },
        "*",
      );
      return;
    }
    if (response.event === "contextChanged") {
      context = response.value;
      dispatchEvent(
        new CustomEvent("cc-switch-context-changed", { detail: context }),
      );
      return;
    }
    const item = pending.get(response.requestId);
    if (!item) return;
    clearTimeout(item.timeout);
    pending.delete(response.requestId);
    if (response.error) item.reject(new Error(response.error));
    else item.resolve(response.value);
  });
  // Start after parsing so the host can attach its listener before the handshake.
  const ready = new Promise((resolve, reject) => {
    addEventListener(
      "DOMContentLoaded",
      () =>
        request("ready").then((value) => {
          context = value;
          resolve(value);
        }, reject),
      { once: true },
    );
  });
  // A page may not consume ready; avoid an unhandled rejection on load failure.
  ready.catch(() => {});
  Object.defineProperty(window, "CCSwitch", {
    value: Object.freeze({
      ready,
      getContext: () => request("getContext"),
      getStorage: () => request("getStorage"),
      setStorage: (value) => request("setStorage", value),
      providers: Object.freeze({
        list: (options = {}) => request("providers.list", options),
      }),
      usage: Object.freeze({
        getStats: (options) => request("usage.getStats", options),
      }),
    }),
  });
})();
