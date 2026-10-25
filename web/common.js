// Shared helpers for the TV and phone pages.

const TBX = {
  SHAPES: ["▲", "◆", "●", "■"],

  $(sel) { return document.querySelector(sel); },

  el(tag, attrs = {}, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") node.className = v;
      else if (k === "style") node.style.cssText = v;
      else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
      else node.setAttribute(k, v);
    }
    for (const c of children.flat()) {
      if (c == null || c === false) continue;
      node.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return node;
  },

  show(id) {
    document.querySelectorAll("[data-screen]").forEach((s) => s.classList.toggle("hidden", s.id !== id));
  },

  avatar(player, size) {
    return TBX.el("span", {
      class: "avatar",
      style: `background:${player.color};width:${size};height:${size};font-size:calc(${size} * 0.5)`,
    }, (player.name[0] || "?").toUpperCase());
  },

  load(key) {
    try { return JSON.parse(localStorage.getItem(key)); } catch { return null; }
  },
  save(key, value) {
    try {
      if (value == null) localStorage.removeItem(key);
      else localStorage.setItem(key, JSON.stringify(value));
    } catch { /* private mode; we just won't remember */ }
  },

  /**
   * Opens a game socket and keeps it open. `params()` is called on every
   * (re)connect so it can include a token learned from the welcome message.
   * A fatal server error stops reconnecting and calls onFatal.
   */
  connect(path, params, { onMessage, onFatal, onStatus }) {
    let ws = null;
    let stopped = false;
    let retry = 0;
    let pingTimer = null;

    const open = () => {
      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      const qs = new URLSearchParams(params()).toString();
      ws = new WebSocket(`${proto}//${location.host}${path}?${qs}`);
      ws.onopen = () => {
        retry = 0;
        onStatus && onStatus(true);
        pingTimer = setInterval(() => send({ type: "ping" }), 25000);
      };
      ws.onmessage = (ev) => {
        const msg = JSON.parse(ev.data);
        if (msg.type === "error" && msg.fatal) {
          stopped = true;
          onFatal(msg.message);
          return;
        }
        onMessage(msg);
      };
      ws.onclose = () => {
        clearInterval(pingTimer);
        if (stopped) return;
        onStatus && onStatus(false);
        retry = Math.min(retry + 1, 6);
        setTimeout(open, 500 * 2 ** (retry - 1));
      };
    };

    const send = (msg) => {
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
    };

    open();
    return {
      send,
      close() { stopped = true; ws && ws.close(); },
    };
  },

  /** Animates a timer bar from the time left in the current phase. */
  runTimer(fill, msLeft, phaseMs) {
    if (msLeft == null || !phaseMs) {
      fill.style.transition = "none";
      fill.style.transform = "scaleX(0)";
      return;
    }
    fill.style.transition = "none";
    fill.style.transform = `scaleX(${msLeft / phaseMs})`;
    void fill.offsetWidth; // restart the transition from the new width
    fill.style.transition = `transform ${msLeft}ms linear`;
    fill.style.transform = "scaleX(0)";
  },
};
