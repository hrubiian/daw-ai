(() => {
  if (window === window.parent) {
    window.addEventListener("DOMContentLoaded", () => {
      const frame = document.querySelector("iframe");
      const status = document.getElementById("status");
      const pending = new Map();

      window.addEventListener("message", ({ source, data }) => {
        if (source !== frame.contentWindow || data?.type !== "daw-param") return;
        const { index, value } = data;
        if (Number.isInteger(index) && index >= 0 && index < 256 &&
            Number.isFinite(value) && value >= 0 && value <= 1) {
          pending.set(index, value);
        }
      });

      async function sync() {
        const changes = Object.fromEntries(pending);
        pending.clear();
        try {
          const response = await fetch("state", Object.keys(changes).length ? {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(changes),
          } : undefined);
          const state = await response.json();
          if (!response.ok) throw new Error(state.error);
          frame.contentWindow.postMessage({ type: "daw-values", values: state.values }, "*");
          status.hidden = true;
        } catch (error) {
          status.textContent = error.message;
          status.hidden = false;
        } finally {
          window.setTimeout(sync, pending.size ? 25 : 100);
        }
      }

      sync();
    }, { once: true });
    return;
  }

  const editing = new Set();
  const applied = new Map();
  function fitPanel() {
    const panel = document.getElementById("root")?.firstElementChild;
    if (!panel?.offsetWidth || !panel.offsetHeight) return;
    const scale = Math.min(1, (window.innerWidth - 12) / panel.scrollWidth,
      (window.innerHeight - 12) / panel.scrollHeight);
    panel.style.zoom = Math.max(0.65, scale);
  }
  window.addEventListener("resize", fitPanel);
  window.addEventListener("DOMContentLoaded", () => {
    const root = document.getElementById("root");
    if (root) new MutationObserver(fitPanel).observe(root, { childList: true });
    fitPanel();
  }, { once: true });
  window.IPlugSendMsg = ({ msg, paramIdx, value }) => {
    if (msg === "BPCFUI") editing.add(paramIdx);
    if (msg === "EPCFUI") editing.delete(paramIdx);
    if (msg === "SPVFUI") {
      applied.delete(paramIdx);
      window.parent.postMessage({ type: "daw-param", index: paramIdx, value }, "*");
    }
  };
  for (const event of ["blur", "pointerup", "pointercancel"]) {
    window.addEventListener(event, () => editing.clear());
  }
  window.addEventListener("message", ({ source, data }) => {
    if (source !== window.parent || data?.type !== "daw-values" || !window.SPVFD) return;
    data.values.forEach((value, index) => {
      if (!editing.has(index) && applied.get(index) !== value) {
        window.SPVFD(index, value);
        applied.set(index, value);
      }
    });
  });
})();
