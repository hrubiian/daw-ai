(() => {
  const updating = new Set();
  const pending = new Map();
  let status = null;
  const applied = new Map();
  function fitPanel() {
    const scale = Math.min(window.innerWidth / 800, window.innerHeight / 600, 2);
    document.documentElement.style.setProperty("--model-m-zoom", scale);
  }

  function presentError(message) {
    if (!status) {
      status = document.createElement("div");
      status.setAttribute("role", "status");
      Object.assign(status.style, {
        position: "fixed", right: "16px", bottom: "16px", zIndex: "999",
        padding: "8px 12px", borderRadius: "4px", background: "#5c320b",
        color: "#fff", font: "13px system-ui",
      });
      document.body.append(status);
    }
    status.hidden = !message;
    status.textContent = message;
  }

  function apply(snapshot) {
    window.__vstOnState?.({host: "daw-ai", api: "CLAP", track: {name: snapshot.track}});
    snapshot.values.forEach((value, index) => {
      if (!updating.has(index) && !pending.has(index) && applied.get(index) !== value && window.SPVFD) {
        window.SPVFD(index, value);
        applied.set(index, value);
      }
    });
    if (snapshot.scope) {
      window.__vstOnProcessorBuffers?.({scope: snapshot.scope});
    }
  }

  window.IPlugSendMsg = ({msg, paramIdx, value}) => {
    if (msg === "BPCFUI") updating.add(paramIdx);
    if (msg === "EPCFUI") updating.delete(paramIdx);
    if (msg === "SPVFUI") {
      applied.delete(paramIdx);
      pending.set(paramIdx, value);
    }
  };
  window.addEventListener("blur", () => updating.clear());
  window.addEventListener("pointerup", () => updating.clear());
  window.addEventListener("pointercancel", () => updating.clear());

  async function sync() {
    const changes = Object.fromEntries(pending);
    pending.clear();
    try {
      const writing = Object.keys(changes).length > 0;
      const response = await fetch("state", writing ? {
        method: "POST",
        headers: {"Content-Type": "application/json"},
        body: JSON.stringify(changes),
      } : undefined);
      const body = await response.json();
      if (!response.ok) throw new Error(body.error);
      apply(body);
      presentError("");
    } catch (error) {
      presentError(error.message || "daw-ai editor disconnected.");
    } finally {
      window.setTimeout(sync, pending.size > 0 ? 25 : 65);
    }
  }
  window.addEventListener("resize", fitPanel);
  window.addEventListener("DOMContentLoaded", () => {
    fitPanel();
    sync();
  }, {once: true});
})();
