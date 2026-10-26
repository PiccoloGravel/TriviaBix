// Shared helpers for the TV and phone pages.

// Older TV browsers (Samsung's on 2019-2020 sets is Chromium 63-69) lack a few newer calls.
if (!Element.prototype.replaceChildren) {
  Element.prototype.replaceChildren = function (...nodes) {
    while (this.firstChild) this.removeChild(this.firstChild);
    this.append(...nodes);
  };
}
if (!Object.fromEntries) {
  Object.fromEntries = (entries) => {
    const o = {};
    for (const [k, v] of entries) o[k] = v;
    return o;
  };
}
if (!Array.prototype.flat) {
  Object.defineProperty(Array.prototype, "flat", {
    configurable: true, writable: true,
    value(depth = 1) {
      return this.reduce((out, v) => out.concat(Array.isArray(v) && depth > 0 ? v.flat(depth - 1) : v), []);
    },
  });
  Object.defineProperty(Array.prototype, "flatMap", {
    configurable: true, writable: true,
    value(fn, self) { return this.map(fn, self).flat(); },
  });
}

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

  /** id -> { id, emoji, name }, filled by loadCharacters(). */
  characters: {},
  characterList: [],

  async loadCharacters() {
    try {
      TBX.characterList = await (await fetch("/api/characters")).json();
      TBX.characters = Object.fromEntries(TBX.characterList.map((c) => [c.id, c]));
    } catch (_) { /* avatars fall back to initials */ }
  },

  avatar(player, size) {
    const ch = TBX.characters[player.character];
    return TBX.el("span", {
      class: "avatar",
      title: ch ? `${player.name} the ${ch.name}` : player.name,
      style: `background:${player.color};width:${size};height:${size};font-size:calc(${size} * ${ch ? 0.62 : 0.5})`,
    }, ch ? ch.emoji : (player.name[0] || "?").toUpperCase());
  },

  /** Fills `box` with character buttons. Taken ones are disabled. */
  characterGrid(box, selected, taken, onPick) {
    box.replaceChildren(...TBX.characterList.map((c) => {
      const isTaken = taken.includes(c.id) && c.id !== selected;
      const b = TBX.el("button", {
        type: "button",
        class: "char" + (c.id === selected ? " sel" : ""),
        title: isTaken ? `${c.name} (taken)` : c.name,
        "aria-label": c.name,
        onclick: () => onPick(c.id),
      }, c.emoji);
      b.disabled = isTaken;
      return b;
    }));
  },

  load(key) {
    try { return JSON.parse(localStorage.getItem(key)); } catch (_) { return null; }
  },
  save(key, value) {
    try {
      if (value == null) localStorage.removeItem(key);
      else localStorage.setItem(key, JSON.stringify(value));
    } catch (_) { /* private mode; we just won't remember */ }
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
