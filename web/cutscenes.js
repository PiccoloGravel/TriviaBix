// Cutscenes: short full-screen bits that play on the TV at big moments. The server tags each host
// line with a cue (intro, round, streak, winner...); web/cutscenes/index.json says which scenes a
// cue can play and which face Bix pulls, and each scene is a JSON file of timed layers. See
// web/cutscenes/README.md for the format, and /cutscenes.html to preview every scene.

// The nullish default (a, else b), spelled out for older TV browsers (Samsung's on 2019-2020 sets is Chromium 63-69).
const nz = (v, d) => (v === undefined || v === null ? d : v);

const Cutscenes = {
  base: "/cutscenes/",
  manifest: { cues: {} },
  scenes: {},
  current: null,
  sound: true,

  /** Generated art (assets/gen/manifest.json, made by art/generate.py): path -> {kind, tags, rating}. */
  art: { files: {}, categories: {} },
  /** Generated files fetched ahead of time, path -> object URL, so a scene never waits on Wi-Fi. */
  preloaded: {},

  /** Loads the manifest and every scene it names, so nothing waits on the network mid-game. */
  async load() {
    try {
      this.manifest = await (await fetch(this.base + "index.json")).json();
    } catch (_) {
      return;
    }
    await this.loadArt();
    const names = new Set(Object.values(this.manifest.cues).flatMap((c) => c.scenes || []));
    await Promise.all([...names].map(async (name) => {
      try {
        this.scenes[name] = await (await fetch(`${this.base}${name}.json`)).json();
      } catch (_) {
        console.warn(`cutscene ${name} failed to load`);
      }
    }));
  },

  /** Reads which generated art exists and starts fetching it in the background. Fine if there's none. */
  async loadArt() {
    try {
      const res = await fetch(this.base + "assets/gen/manifest.json");
      if (!res.ok) return;
      const m = await res.json();
      this.art = { files: m.files || {}, categories: m.categories || {} };
    } catch (_) {
      return;
    }
    this.preload();
  },

  /** Fetches the art one file at a time, smallest first, so the stickers are ready long before the clips. */
  async preload() {
    const paths = Object.keys(this.art.files).sort((a, b) => (this.art.files[a].bytes || 0) - (this.art.files[b].bytes || 0));
    for (const path of paths) {
      try {
        const blob = await (await fetch(this.base + "assets/gen/" + path)).blob();
        this.preloaded[path] = URL.createObjectURL(blob);
      } catch (_) { /* it'll load when it plays */ }
    }
  },

  /**
   * Picks a generated file for a scene. `want` is "kind:id" (one file), or {kind, tags, any} where
   * kind can be a list in order of preference (["clip", "backdrop"]), tags must all match (or any of
   * them, with `any: true`), and kind "category" finds the card for the current category. Only art at
   * or below the room's rating is picked. Returns a URL, or null when nothing fits.
   */
  pickArt(want, ctx = {}) {
    if (!want) return null;
    const files = this.art.files;
    const allowed = (f) => RATINGS.indexOf(f.rating || "PG") <= RATINGS.indexOf(ctx.rating || "NC-17");
    const url = (path) => this.preloaded[path] || this.base + "assets/gen/" + path;
    if (typeof want === "string") {
      const [kind, id] = want.split(":");
      const path = Object.keys(files).find((p) => files[p].kind === kind && files[p].id === id);
      return path && allowed(files[path]) ? url(path) : null;
    }
    for (const kind of [].concat(want.kind || [])) {
      if (kind === "category") {
        const path = this.art.categories[slug((ctx.vars || {}).category || "")];
        if (path && files[path] && allowed(files[path])) return url(path);
        continue;
      }
      const tags = [].concat(want.tags || []);
      const match = Object.keys(files).filter((p) => {
        const f = files[p];
        const has = (t) => (f.tags || []).includes(t);
        return f.kind === kind && allowed(f) && (!tags.length || (want.any ? tags.some(has) : tags.every(has)));
      });
      if (match.length) return url(match[Math.floor(Math.random() * match.length)]);
    }
    return null;
  },

  /** Bix's face for a cue, or null to leave it as it is. */
  moodFor(cue) {
    const c = cue && this.manifest.cues[cue];
    return (c && c.mood) || null;
  },

  /** Plays one of the cue's scenes (if it has any and wins its `chance` roll). Returns whether one started. */
  cue(cue, ctx) {
    const c = cue && this.manifest.cues[cue];
    const names = c && (c.scenes || []).filter((n) => this.scenes[n]);
    if (!names || !names.length || Math.random() >= (nz(c.chance, 1))) return false;
    this.play(names[Math.floor(Math.random() * names.length)], ctx);
    return true;
  },

  get playing() { return !!this.current; },

  /** Cuts the current scene short. */
  stop() {
    if (!this.current) return;
    this.current.end();
  },

  /**
   * Plays scene `name`. `ctx` has `players` (everyone), `subject` (the player the moment is about,
   * or null) and `vars` for {placeholders} in text. Resolves when the scene is gone.
   */
  play(name, ctx = {}) {
    const scene = typeof name === "string" ? this.scenes[name] : name;
    if (!scene) return Promise.resolve();
    this.stop();
    const vars = ctx.vars || {};
    const fill = (text) => String(nz(text, "")).replace(/\{(\w+)\}/g, (_, k) => nz(vars[k], ""));
    const root = div("cutscene");
    const timers = [];
    const later = (ms, fn) => timers.push(setTimeout(fn, ms));

    if (scene.backdrop) root.append(backdrop(scene.backdrop, ctx));
    for (let layer of scene.layers || []) {
      // Generated art that isn't there (not made yet, or above the room's rating) falls back.
      if (layer.art && !this.pickArt(layer.art, ctx)) layer = layer.fallback ? { ...layer, ...layer.fallback, art: null } : null;
      const node = layer && Cutscenes.layers[layer.type] && Cutscenes.layers[layer.type](layer, ctx, fill);
      if (node) root.append(place(node, layer, later));
    }
    if (this.sound) for (const s of scene.sounds || []) later(s.at || 0, () => Cutscenes.sfx.play(s));
    document.body.append(root);
    document.body.classList.add("cutscene-on");
    root.querySelectorAll(".cs-text-title, .cs-text-stamp").forEach(fit);

    const duration = scene.duration || 3000;
    return new Promise((resolve) => {
      const me = {
        end: () => {
          timers.forEach(clearTimeout);
          if (this.current === me) {
            this.current = null;
            document.body.classList.remove("cutscene-on");
          }
          root.classList.add("leaving");
          setTimeout(() => root.remove(), 350);
          resolve();
        },
      };
      this.current = me;
      later(duration, me.end);
    });
  },
};

const RATINGS = ["PG", "PG-13", "MA", "NC-17"];

/** Same as slugify() in art/generate.py: "Science and Nature" -> "science-and-nature". */
function slug(text) {
  return String(text).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

function div(cls, style) {
  const d = document.createElement("div");
  if (cls) d.className = cls;
  if (style) d.style.cssText = style;
  return d;
}

/** Wraps a layer in its position, entrance, idle loop and exit. Positions are % of the screen, sizes vh. */
function place(node, layer, later) {
  const at = layer.at || 0;
  const inMs = nz(layer.inMs, 600);
  const outer = div("cs-layer", `left:${nz(layer.x, 50)}%;top:${nz(layer.y, 50)}%;z-index:${layer.z || 1};` +
    `--rot:${layer.rotate || 0}deg`);
  const enter = div(`cs-in cs-in-${layer.in || "pop"}`, `animation-duration:${inMs}ms;animation-delay:${at}ms`);
  const loop = div(layer.loop ? `cs-loop cs-loop-${layer.loop}` : "cs-loop", `animation-delay:${at + inMs}ms`);
  loop.append(node);
  enter.append(loop);
  outer.append(enter);
  if (layer.out != null) {
    later(layer.out, () => {
      enter.style.animation = "none";
      outer.classList.add(`cs-out-${layer.leave || "fade"}`);
    });
  }
  return outer;
}

/** Shrinks one-line text that would run off the screen (long names, big words). */
function fit(t) {
  const max = innerWidth * (parseFloat(t.style.maxWidth) || 90) / 100;
  const w = t.scrollWidth;
  if (w > max) t.style.fontSize = `${parseFloat(t.style.fontSize) * max / w}vh`;
}

/**
 * The scene's background. With `art`, a generated picture or looping clip goes on top of the drawn
 * kind (which shows instead when the art isn't there), slowly drifting if `pan` is set, under a
 * `shade` (0 to 1) that keeps text readable.
 */
function backdrop(b, ctx) {
  const [c0 = "#e63973", c1 = "#2f80ed"] = b.colors || [];
  const d = div(`cs-backdrop cs-bg-${b.kind || "flash"} cs-enter-${b.enter || "wipe"}`, `--c0:${c0};--c1:${c1}`);
  if (b.kind === "curtains") {
    d.style.setProperty("--open", `${nz(b.open, 600)}ms`);
    d.append(div("cs-curtain l"), div("cs-curtain r"), div("cs-valance"));
  }
  if (b.kind === "sunburst" || b.kind === "spotlight") d.append(div("cs-rays"));
  const src = b.art && Cutscenes.pickArt(b.art, ctx);
  if (src) {
    const art = media(src, `cs-art${b.pan ? " cs-art-pan" : ""}`);
    const shade = div("cs-art-shade", `opacity:${nz(b.shade, 0.25)}`);
    if (b.kind === "curtains") d.querySelector(".cs-curtain").before(art, shade);
    else d.append(art, shade);
  }
  return d;
}

/** An <img>, or a muted looping <video> for .mp4/.webm (or a blob URL the preloader made from one). */
function media(src, cls) {
  const isVideo = /\.(mp4|webm)$/i.test(src) || Object.entries(Cutscenes.preloaded).some(([p, u]) => u === src && /\.(mp4|webm)$/i.test(p));
  const m = document.createElement(isVideo ? "video" : "img");
  m.className = cls;
  m.src = src;
  if (isVideo) {
    Object.assign(m, { muted: true, loop: true, autoplay: true, playsInline: true });
    m.play().catch(() => {});
  } else {
    m.alt = "";
  }
  return m;
}

/** The kinds of layer a scene can use. Add one here and document it in web/cutscenes/README.md. */
Cutscenes.layers = {
  text(l, ctx, fill) {
    const t = div(`cs-text cs-text-${l.style || "title"}`, `font-size:${l.size || 12}vh;` +
      (l.color ? `color:${l.color};` : "") + (l.bg ? `background:${l.bg};` : "") + `max-width:${l.width || 90}vw`);
    t.textContent = fill(l.text);
    return t.textContent.trim() ? t : null;
  },

  bix(l) {
    const box = div("cs-bix", `width:${l.size || 40}vh`);
    box.append(Bix.create(l.mood || "excited"));
    return box;
  },

  emoji(l) {
    const e = div("cs-emoji", `font-size:${l.size || 15}vh`);
    e.textContent = l.char || "⭐";
    return e;
  },

  /** One player: who is "subject" (the player the moment is about), "first", "last" or an index. */
  player(l, ctx) {
    const players = ctx.players || [];
    const p = l.who === "first" ? players[0] : l.who === "last" ? players[players.length - 1]
      : typeof l.who === "number" ? players[l.who] : ctx.subject;
    if (!p) return null;
    const box = div("cs-player");
    box.append(TBX.avatar(p, `${l.size || 24}vh`));
    if (l.name !== false) {
      const n = div("cs-name", `font-size:${Math.max(3, (l.size || 24) / 5)}vh`);
      n.textContent = p.name;
      box.append(n);
    }
    if (l.hat) {
      const hat = div("cs-hat", `font-size:${(l.size || 24) * 0.6}vh;animation-delay:${(l.at || 0) + (nz(l.hatAt, 700))}ms`);
      hat.textContent = l.hat;
      box.append(hat);
    }
    return box;
  },

  /** Everyone in a row, popping in one after another. */
  players(l, ctx) {
    const list = ctx.players || [];
    if (!list.length) return null;
    const size = l.size || Math.min(12, 80 / list.length);
    const row = div("cs-lineup", `gap:${size * 0.25}vh`);
    list.forEach((p, i) => {
      const cell = div("cs-cell", `animation-delay:${(l.at || 0) + i * (nz(l.stagger, 120))}ms`);
      cell.append(TBX.avatar(p, `${size}vh`));
      if (l.name !== false) {
        const n = div("cs-name", `font-size:${Math.max(2.2, size / 4)}vh`);
        n.textContent = p.name;
        cell.append(n);
      }
      row.append(cell);
    });
    return row;
  },

  /** Particles: rain (fall from the top), rise (float up), or burst (fly out from the layer's x/y). */
  effect(l) {
    const chars = l.chars || ["⭐"];
    const box = div(`cs-fx cs-fx-${l.kind || "rain"}`);
    const n = l.count || 24;
    const life = l.life || 1800;
    for (let i = 0; i < n; i++) {
      const p = div("cs-particle", `font-size:${(l.size || 6) * (0.6 + Math.random() * 0.8)}vh;` +
        `animation-duration:${life * (0.7 + Math.random() * 0.6)}ms;animation-delay:${(l.at || 0) + Math.random() * (nz(l.spread, 900))}ms;` +
        `--x:${Math.random() * 100}vw;--dx:${(Math.random() - 0.5) * 80}vw;--dy:${(Math.random() - 0.5) * 80}vh;--r:${(Math.random() - 0.5) * 720}deg`);
      p.textContent = chars[i % chars.length];
      box.append(p);
    }
    if (l.kind === "confetti" && typeof confetti === "function") setTimeout(confetti, l.at || 0);
    return box;
  },

  /**
   * Any picture the browser can show (GIF, WebP, PNG, SVG) or a looping clip (MP4, WebM). Either a
   * file in web/cutscenes/assets/ (`src`) or generated art (`art`, see Cutscenes.pickArt).
   */
  image(l, ctx) {
    const src = l.art ? Cutscenes.pickArt(l.art, ctx)
      : l.src.startsWith("/") || l.src.includes("://") ? l.src : Cutscenes.base + l.src;
    if (!src) return null;
    const m = media(src, `cs-image${l.frame ? " cs-image-frame" : ""}`);
    m.style.height = `${l.size || 30}vh`;
    return m;
  },

  /** A Lottie animation (After Effects export, or a free one from lottiefiles.com). The player loads on first use. */
  lottie(l) {
    const box = div("cs-lottie", `width:${l.size || 40}vh;height:${l.size || 40}vh`);
    const src = l.src.startsWith("/") || l.src.includes("://") ? l.src : Cutscenes.base + l.src;
    loadLottie().then((lottie) => {
      const anim = lottie.loadAnimation({ container: box, renderer: "svg", loop: !!l.loopAnim, autoplay: false, path: src });
      anim.addEventListener("DOMLoaded", () => setTimeout(() => anim.play(), l.at || 0));
    }).catch(() => console.warn("Lottie player failed to load"));
    return box;
  },
};

const LOTTIE_URL = "https://cdnjs.cloudflare.com/ajax/libs/lottie-web/5.12.2/lottie_light.min.js";
function loadLottie() {
  if (window.lottie) return Promise.resolve(window.lottie);
  loadLottie.p = loadLottie.p || new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = LOTTIE_URL;
    s.onload = () => resolve(window.lottie);
    s.onerror = reject;
    document.head.append(s);
  });
  return loadLottie.p;
}

// ---------- Sound effects ----------
// Synthesised in the browser, so there are no audio files to license. A scene can also play a file
// with { "src": "assets/honk.mp3" }.

Cutscenes.sfx = {
  ctx: null,
  volume: 0.35,

  play(s) {
    if (s.src) {
      const a = new Audio(s.src.startsWith("/") || s.src.includes("://") ? s.src : Cutscenes.base + s.src);
      a.volume = nz(s.volume, 0.8);
      a.play().catch(() => {});
      return;
    }
    const fn = this.sounds[s.sound];
    if (!fn) return;
    try {
      this.ctx = this.ctx || new (window.AudioContext || window.webkitAudioContext)();
      if (this.ctx.state === "suspended") this.ctx.resume();
      const out = this.ctx.createGain();
      out.gain.value = this.volume * (nz(s.volume, 1));
      out.connect(this.ctx.destination);
      fn(this.ctx, out, this.ctx.currentTime);
    } catch (_) { /* no Web Audio on this TV */ }
  },

  /** A note: `type` wave from `f0` Hz (sliding to `f1`), with a quick attack and a fade. */
  tone(ac, out, t, { type = "sine", f0, f1 = f0, dur = 0.3, gain = 1, vibrato = 0 }) {
    const o = ac.createOscillator();
    const g = ac.createGain();
    o.type = type;
    o.frequency.setValueAtTime(f0, t);
    if (f1 !== f0) o.frequency.exponentialRampToValueAtTime(f1, t + dur);
    if (vibrato) {
      const lfo = ac.createOscillator();
      const depth = ac.createGain();
      lfo.frequency.value = 6;
      depth.gain.value = vibrato;
      lfo.connect(depth).connect(o.frequency);
      lfo.start(t);
      lfo.stop(t + dur);
    }
    g.gain.setValueAtTime(0.0001, t);
    g.gain.exponentialRampToValueAtTime(gain, t + 0.02);
    g.gain.setValueAtTime(gain, t + dur * 0.7);
    g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    o.connect(g).connect(out);
    o.start(t);
    o.stop(t + dur + 0.05);
  },

  noise(ac, out, t, { dur = 0.4, from = 400, to = 4000, gain = 1, type = "bandpass" }) {
    const buf = ac.createBuffer(1, Math.ceil(ac.sampleRate * dur), ac.sampleRate);
    const data = buf.getChannelData(0);
    for (let i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1;
    const src = ac.createBufferSource();
    src.buffer = buf;
    const f = ac.createBiquadFilter();
    f.type = type;
    f.frequency.setValueAtTime(from, t);
    f.frequency.exponentialRampToValueAtTime(to, t + dur);
    const g = ac.createGain();
    g.gain.setValueAtTime(0.0001, t);
    g.gain.exponentialRampToValueAtTime(gain, t + dur * 0.3);
    g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    src.connect(f).connect(g).connect(out);
    src.start(t);
  },

  sounds: {
    whoosh(ac, out, t) { Cutscenes.sfx.noise(ac, out, t, { dur: 0.5, from: 300, to: 3000, gain: 1.4 }); },
    boing(ac, out, t) { Cutscenes.sfx.tone(ac, out, t, { f0: 180, f1: 520, dur: 0.45, gain: 0.8, vibrato: 40 }); },
    ding(ac, out, t) {
      Cutscenes.sfx.tone(ac, out, t, { f0: 1320, dur: 0.8, gain: 0.6 });
      Cutscenes.sfx.tone(ac, out, t, { f0: 1980, dur: 0.5, gain: 0.25 });
    },
    pop(ac, out, t) { Cutscenes.sfx.tone(ac, out, t, { f0: 600, f1: 1400, dur: 0.12, gain: 0.7 }); },
    tada(ac, out, t) {
      const chord = (at, dur) => [523, 659, 784].forEach((f) =>
        Cutscenes.sfx.tone(ac, out, t + at, { type: "sawtooth", f0: f, dur, gain: 0.18 }));
      chord(0, 0.18);
      chord(0.2, 0.9);
    },
    drumroll(ac, out, t) {
      for (let i = 0; i < 22; i++) Cutscenes.sfx.noise(ac, out, t + i * 0.055, { dur: 0.06, from: 900, to: 600, gain: 0.4 + i * 0.03 });
      Cutscenes.sfx.noise(ac, out, t + 1.25, { dur: 1.2, from: 6000, to: 2500, gain: 1, type: "highpass" });
    },
    sad_trombone(ac, out, t) {
      [[293.7, 0.4], [277.2, 0.4], [261.6, 0.4], [246.9, 1.2]].reduce((at, [f, dur], i) => {
        Cutscenes.sfx.tone(ac, out, t + at, { type: "sawtooth", f0: f, f1: i === 3 ? f * 0.97 : f, dur, gain: 0.22, vibrato: i === 3 ? 6 : 0 });
        return at + dur + 0.05;
      }, 0);
    },
    airhorn(ac, out, t) {
      [0, 0.28, 0.56].forEach((at, i) => [440, 554, 659].forEach((f) =>
        Cutscenes.sfx.tone(ac, out, t + at, { type: "sawtooth", f0: f, f1: f * 0.98, dur: i === 2 ? 0.7 : 0.22, gain: 0.12 })));
    },
    crickets(ac, out, t) {
      for (let i = 0; i < 6; i++) Cutscenes.sfx.tone(ac, out, t + i * 0.32 + (i % 2) * 0.06, { f0: 4200, dur: 0.07, gain: 0.25 });
    },
    slam(ac, out, t) {
      Cutscenes.sfx.tone(ac, out, t, { f0: 140, f1: 50, dur: 0.35, gain: 1 });
      Cutscenes.sfx.noise(ac, out, t, { dur: 0.25, from: 2000, to: 300, gain: 0.6, type: "lowpass" });
    },
  },
};
