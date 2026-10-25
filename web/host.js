// The TV screen. After "Start a room" it needs no mouse: the server moves the
// game along on timers, and Enter/Space (a remote's OK button) skips ahead.

const { $, el, show } = TBX;
const HOST_KEY = "tbx:host";

let settings = { rounds: 10, max_rating: "PG", mode: "trivia", clip_ms: 2000 };
let conn = null;
let state = null;
let prev = null;
let joinBase = location.origin;
let secsTimer = null;

// ---------- The host: voice, speech bubble and callout graphics ----------
// The server writes the script (see server/host-script.json) and sends the current line in
// `state.host`; this page says it out loud and draws the graphics that go with it.

const voice = {
  on: TBX.load("tbx:voice") !== false,
  pick: TBX.load("tbx:voice-name"),
  chosen: null,
  queue: 0,
  /** English voices, best first. Edge's "Natural" voices and Chrome's Google voices are free and sound far less robotic. */
  ranked() {
    if (!("speechSynthesis" in window)) return [];
    const score = (v) => [/natural|neural/i, /google uk english male/i, /google/i, /daniel|samantha|karen|moira/i]
      .findIndex((re) => re.test(v.name));
    return speechSynthesis.getVoices()
      .filter((v) => /^en/i.test(v.lang))
      .sort((a, b) => ((score(a) + 5) % 5) - ((score(b) + 5) % 5));
  },
  choose() {
    const list = this.ranked();
    this.chosen = list.find((v) => v.name === this.pick) || list[0] || null;
  },
  /** Cycles to the next voice; returns its name. */
  next() {
    const list = this.ranked();
    if (!list.length) return "the default voice";
    const i = list.findIndex((v) => v === this.chosen);
    this.chosen = list[(i + 1) % list.length];
    this.pick = this.chosen.name;
    TBX.save("tbx:voice-name", this.pick);
    return this.chosen.name;
  },
  say(text) {
    if (!this.on || !text || !("speechSynthesis" in window)) return;
    if (!this.chosen) this.choose();
    const u = new SpeechSynthesisUtterance(text);
    if (this.chosen) u.voice = this.chosen;
    u.rate = 1.05;
    u.pitch = 1.1;
    u.onstart = () => { this.queue++; hostBar.classList.add("talking"); };
    u.onend = u.onerror = () => { if (--this.queue <= 0) { this.queue = 0; hostBar.classList.remove("talking"); } };
    speechSynthesis.speak(u);
    return u;
  },
  /** Runs `fn` once the host has finished the line in state `s`, or soon if there's nothing to wait for. */
  afterLine(s, fn) {
    let done = false;
    const go = () => { if (!done) { done = true; fn(); } };
    const text = s.host ? s.host.text : "";
    if (!this.on || !text || !("speechSynthesis" in window)) return setTimeout(go, 600);
    this.pending = { id: s.host.id, go };
    // Some TV browsers never fire onend, so fall back on a rough speaking time.
    setTimeout(go, 1500 + text.length * 80);
  },
  /** Called when the last part of host line `id` has been spoken. */
  lineDone(id) {
    if (this.pending && this.pending.id === id) this.pending.go();
  },
  hush() {
    if ("speechSynthesis" in window) speechSynthesis.cancel();
    this.queue = 0;
    hostBar.classList.remove("talking");
  },
};
if ("speechSynthesis" in window) speechSynthesis.addEventListener("voiceschanged", () => voice.choose());

const hostBar = $("#host-bar");
let spokenId = null;
let typeTimer = null;

/** Speaks and shows a new host line, once per line. `extra` is read out after it (the question). */
function hostSays(s, extra) {
  const h = s.host;
  if (!h) {
    hostBar.classList.add("quiet");
    return;
  }
  if (h.id === spokenId) return;
  spokenId = h.id;
  $("#host-face").dataset.name = h.name;
  hostBar.classList.remove("quiet");
  const bubble = $("#host-bubble");
  bubble.classList.remove("fresh");
  void bubble.offsetWidth;
  bubble.classList.add("fresh");
  // Type the line out at roughly speaking speed.
  clearInterval(typeTimer);
  let shown = 0;
  typeTimer = setInterval(() => {
    shown = Math.min(h.text.length, shown + 2);
    bubble.textContent = h.text.slice(0, shown);
    bubble.scrollTop = bubble.scrollHeight;
    if (shown >= h.text.length) clearInterval(typeTimer);
  }, voice.on ? 140 : 30);
  voice.hush();
  const first = voice.say(h.text);
  const last = (extra && voice.say(extra)) || first;
  if (last) last.addEventListener("end", () => voice.lineDone(h.id));
  showCallouts(s);
}

// ---------- Name That Tune audio ----------
// The TV streams Apple's 30-second preview and plays only a slice of it. Browsers block sound
// until someone has pressed something on the page, so after a refresh we ask for one press.

const tune = {
  audio: new Audio(),
  next: new Audio(),
  stopTimer: null,
  blocked: false,
  last: null,

  prefetch(url) {
    if (!url || this.next.src === url || this.audio.src === url) return;
    this.next.preload = "auto";
    this.next.src = url;
  },

  /** Plays `ms` of `url` from `startMs`, then calls `onEnd`. A null `ms` plays to the end. */
  play(url, startMs, ms, { volume = 1, onEnd } = {}) {
    this.stop();
    this.last = [url, startMs, ms, { volume, onEnd }];
    if (this.next.src === url) [this.audio, this.next] = [this.next, this.audio];
    const a = this.audio;
    if (a.src !== url) a.src = url;
    a.volume = volume;
    const seekAndPlay = () => {
      a.currentTime = startMs / 1000;
      a.play().then(() => {
        this.setBlocked(false);
      }).catch(() => this.setBlocked(true));
    };
    // Count the clip from when sound actually starts, so slow loading doesn't eat it.
    a.onplaying = () => {
      a.onplaying = null; // a stall and resume shouldn't restart the clock
      $("#q-eq").classList.add("playing");
      if (ms != null) this.stopTimer = setTimeout(() => { this.stop(); onEnd && onEnd(); }, ms);
    };
    if (a.readyState >= 1) seekAndPlay();
    else a.addEventListener("loadedmetadata", seekAndPlay, { once: true });
  },

  replay() { if (this.last) this.play(...this.last); },

  stop() {
    clearTimeout(this.stopTimer);
    this.audio.onplaying = null;
    this.audio.pause();
    $("#q-eq").classList.remove("playing");
  },

  setBlocked(on) {
    this.blocked = on;
    $("#sound-btn").classList.toggle("hidden", !on);
  },

  /** Called on any press: a blocked clip gets another go now that the browser allows sound. */
  unlock() {
    if (!this.blocked) return false;
    this.setBlocked(false);
    this.replay();
    return true;
  },
};
let replayTimer = null;


const CALLOUT_ICONS = {
  streak: "🔥", streak_broken: "💔", roast: "🍗", asleep: "😴", only_one: "🧠", fastest: "⚡",
  nobody: "🤦", everyone: "🎯", new_leader: "👑", leader: "🏃", tie: "🤝", last_place: "🐌", winner: "🏆",
};

/** Pops the line's callouts one after another, roughly as the host gets to them. */
function showCallouts(s) {
  const box = $("#callouts");
  box.replaceChildren();
  box.classList.toggle("top", s.phase === "final");
  const byId = Object.fromEntries(s.players.map((p) => [p.id, p]));
  const start = s.phase === "reveal" ? 1.4 : 0.8;
  s.host.callouts.forEach((c, i) => {
    const p = c.player_id != null ? byId[c.player_id] : null;
    const at = start + i * 2.2;
    box.append(el("div", { class: `callout ${c.kind}`, style: `--in:${at}s;--out:${at + (s.phase === "final" ? 8 : 3.4)}s` },
      el("span", { class: "icon" }, CALLOUT_ICONS[c.kind] || "⭐"),
      p ? TBX.avatar(p, "7vh") : null,
      el("span", {}, c.label, p ? el("span", { class: "who" }, p.name) : null)));
  });
}

/** The full-screen "QUESTION 3" card that sweeps across before the question appears. */
function sweepCard(s) {
  const last = s.round === s.total_rounds && s.total_rounds > 1;
  const card = el("div", { class: "card-sweep" + (last ? " final-q" : "") },
    el("div", { class: "small" }, last ? (s.hangman ? "Final puzzle" : "Final question") : roundLabel(s)),
    el("div", { class: "big" }, last ? "LAST ONE" : `Q${s.round}`),
    el("div", { class: "small" }, (s.question || s.hangman).category));
  document.body.append(card);
  setTimeout(() => card.remove(), 1700);
}

/** Restarts the entry animations on a screen. */
function enter(screen) {
  screen.classList.remove("enter");
  void screen.offsetWidth;
  screen.classList.add("enter");
}

function confetti() {
  const canvas = $("#confetti");
  const ctx = canvas.getContext("2d");
  canvas.width = innerWidth;
  canvas.height = innerHeight;
  const colors = ["#ffd23f", "#e63973", "#2f80ed", "#2ee59d", "#fff8f0"];
  const bits = Array.from({ length: 180 }, () => ({
    x: Math.random() * canvas.width, y: -Math.random() * canvas.height * 0.5,
    vx: (Math.random() - 0.5) * 4, vy: 2 + Math.random() * 4, r: Math.random() * Math.PI,
    s: 6 + Math.random() * 8, c: colors[Math.floor(Math.random() * colors.length)],
  }));
  const end = Date.now() + 5000;
  (function frame() {
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    for (const b of bits) {
      b.x += b.vx; b.y += b.vy; b.r += 0.1;
      ctx.save(); ctx.translate(b.x, b.y); ctx.rotate(b.r);
      ctx.fillStyle = b.c; ctx.fillRect(-b.s / 2, -b.s / 4, b.s, b.s / 2);
      ctx.restore();
    }
    if (Date.now() < end) requestAnimationFrame(frame);
    else ctx.clearRect(0, 0, canvas.width, canvas.height);
  })();
}

// ---------- Setup ----------

function renderSetup() {
  document.querySelectorAll("[data-mode]").forEach((b) => {
    b.classList.toggle("secondary", b.dataset.mode !== settings.mode);
  });
  document.querySelectorAll("[data-rounds]").forEach((b) => {
    b.classList.toggle("secondary", Number(b.dataset.rounds) !== settings.rounds);
  });
  document.querySelectorAll("[data-rating]").forEach((b) => {
    b.classList.toggle("secondary", b.dataset.rating !== settings.max_rating);
  });
  document.querySelectorAll("[data-clip]").forEach((b) => {
    b.classList.toggle("secondary", Number(b.dataset.clip) !== settings.clip_ms);
  });
  $("#clip-opts").classList.toggle("hidden", settings.mode !== "songs");
  $("#rounds-label").textContent = { songs: "Songs", hangman: "Puzzles" }[settings.mode] || "Questions";
}

document.querySelectorAll("[data-rounds]").forEach((b) =>
  b.addEventListener("click", () => { settings.rounds = Number(b.dataset.rounds); renderSetup(); }));
document.querySelectorAll("[data-mode]").forEach((b) =>
  b.addEventListener("click", () => { settings.mode = b.dataset.mode; renderSetup(); }));
document.querySelectorAll("[data-rating]").forEach((b) =>
  b.addEventListener("click", () => { settings.max_rating = b.dataset.rating; renderSetup(); }));
document.querySelectorAll("[data-clip]").forEach((b) =>
  b.addEventListener("click", () => { settings.clip_ms = Number(b.dataset.clip); renderSetup(); }));

$("#create").addEventListener("click", async () => {
  $("#setup-error").textContent = "";
  try {
    const res = await fetch("/api/rooms", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(settings),
    });
    const body = await res.json();
    if (!res.ok) throw new Error(body.error || "Couldn't create a room.");
    TBX.save(HOST_KEY, { code: body.code, token: body.host_token });
    join(body.code, body.host_token);
  } catch (e) {
    $("#setup-error").textContent = e.message;
  }
});

// ---------- Connection ----------

function join(code, token) {
  conn && conn.close();
  prev = null;
  conn = TBX.connect("/ws/host", () => ({ code, token }), {
    onMessage(msg) {
      if (msg.type === "state") render(msg);
      if (msg.type === "error") toast(msg.message);
    },
    onFatal(message) {
      TBX.save(HOST_KEY, null);
      show("setup");
      $("#setup-error").textContent = message + " Start a new room.";
    },
    onStatus(ok) { $("#offline").classList.toggle("hidden", ok); },
  });
}

function send(msg) { conn && conn.send(msg); }

function toast(text) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 3000);
}

// ---------- Rendering ----------

function render(s) {
  prev = state;
  state = s;
  const phaseChanged = !prev || prev.phase !== s.phase || prev.round !== s.round;
  $("#skip-btn").classList.toggle("hidden", !["intro", "question", "reveal", "scoreboard"].includes(s.phase));
  if (phaseChanged) {
    $("#callouts").replaceChildren();
    tune.stop();
    clearTimeout(replayTimer);
  }
  tune.prefetch(s.next_clip);
  if (s.hangman && (s.phase === "question" || s.phase === "reveal")) {
    show("hangman");
    renderHangman(s, phaseChanged);
    return hostSays(s);
  }
  show(s.phase);
  ({ lobby: renderLobby, intro: renderIntro, question: renderQuestion, reveal: renderReveal, scoreboard: renderScores, final: renderFinal })[s.phase](s, phaseChanged);
  // The question is read out after the host's banter.
  // In Name That Tune the clip plays instead.
  hostSays(s, s.phase === "question" && s.question.kind !== "song" ? s.question.prompt : null);
}

function renderIntro(s, changed) {
  if (!changed) return;
  $("#i-tagline").textContent = `${s.total_rounds} ${{ hangman: "puzzles", songs: "songs" }[s.settings.mode] || "questions"} · rated ${s.settings.max_rating} · ${s.players.length} contestants`;
  $("#i-lineup").replaceChildren(...s.players.map((p, i) =>
    el("div", { style: `--i:${i}` }, TBX.avatar(p, "11vh"), el("div", {}, p.name))));
}

function renderLobby(s, changed) {
  const url = joinBase.replace(/^https?:\/\//, "");
  $("#join-url").textContent = url;
  $("#room-code").textContent = s.code;
  const link = `${joinBase}/?code=${s.code}`;
  const qr = $("#qr");
  if (qr.dataset.link !== link) {
    qr.dataset.link = link;
    qr.src = "/api/qr?text=" + encodeURIComponent(link);
  }
  $("#lobby-settings").textContent = s.settings.mode === "songs"
    ? `🎵 Name That Tune · ${s.settings.rounds} songs · ${s.settings.clip_ms / 1000}-second clips · rated ${s.settings.max_rating}`
    : s.settings.mode === "hangman"
    ? `Speed Hangman · ${s.settings.rounds} puzzles · rated ${s.settings.max_rating}`
    : `${s.settings.rounds} questions · rated ${s.settings.max_rating}`;

  const slots = $("#slots");
  slots.replaceChildren();
  for (const p of s.players) {
    const slot = el("div", { class: "slot pop" + (p.connected ? "" : " away") },
      TBX.avatar(p, "10vh"),
      el("div", { class: "name" }, p.name),
      p.id === s.vip ? el("span", { class: "vip-tag" }, "VIP") : null);
    slots.append(slot);
  }
  const shown = Math.max(8, s.players.length + (s.players.length < s.max_players ? 1 : 0));
  for (let i = s.players.length; i < Math.min(shown, s.max_players); i++) {
    slots.append(el("div", { class: "slot empty" }, `Player ${i + 1}`));
  }

  const vip = s.players.find((p) => p.id === s.vip);
  $("#lobby-hint").textContent = s.players.length === 0
    ? `Waiting for players (up to ${s.max_players})…`
    : `${s.players.length}/${s.max_players} in. ${vip ? vip.name + " (VIP) can start from their phone, or" : ""} press`;
  $("#start-btn").disabled = s.players.length === 0;
}

function choiceTile(text, i) {
  // Song answers are "Title — Artist"; show the artist smaller under the title.
  const [title, artist] = state && state.question && state.question.kind === "song" ? text.split(" — ") : [text];
  return el("div", { class: `choice choice-${i}`, style: `--i:${i}` }, el("span", { class: "shape" }, TBX.SHAPES[i]),
    artist ? el("span", { class: "song" }, el("span", {}, title), el("small", {}, artist)) : el("span", {}, text));
}

function roundLabel(s) {
  const kind = s.hangman ? "Puzzle" : s.question && s.question.kind === "song" ? "Song" : "Question";
  return `${kind} ${s.round} of ${s.total_rounds}`;
}

/** Plays the clip once the host stops talking, then once more halfway through what's left for anyone who blinked. */
function playClip(s) {
  const clip = s.question.clip;
  if (!clip) return;
  const listen = $("#q-listen");
  const end = Date.now() + (s.ms_left || 0);
  listen.textContent = "Get ready…";
  voice.afterLine(s, () => {
    if (!state || state.phase !== "question" || state.round !== s.round) return;
    listen.textContent = "Listen…";
    tune.play(clip.preview_url, clip.start_ms, clip.clip_ms, { onEnd: () => { listen.textContent = ""; } });
    clearTimeout(replayTimer);
    replayTimer = setTimeout(() => {
      if (!state || state.phase !== "question" || state.round !== s.round) return;
      listen.textContent = "One more time…";
      tune.play(clip.preview_url, clip.start_ms, clip.clip_ms, { onEnd: () => { listen.textContent = "That's all you get!"; } });
    }, Math.max(0, end - Date.now()) / 2);
  });
}

function renderQuestion(s, changed) {
  $("#q-round").textContent = roundLabel(s);
  $("#q-code").textContent = s.code;
  $("#q-cat").textContent = s.question.category;
  $("#q-prompt").textContent = s.question.prompt;
  const song = s.question.kind === "song";
  $("#question").classList.toggle("song", song);
  $("#q-eq").classList.toggle("hidden", !song);
  $("#q-listen").classList.toggle("hidden", !song);
  if (changed) {
    $("#q-choices").replaceChildren(...s.question.choices.map(choiceTile));
    TBX.runTimer($("#q-timer"), s.ms_left, s.phase_ms);
    startSecs($("#q-secs"), s.ms_left);
    sweepCard(s);
    enter($("#question"));
    if (song) playClip(s);
  }
  $("#q-answered").replaceChildren(...s.players.map((p) => {
    const a = TBX.avatar(p, "6vh");
    if (!p.answered) a.classList.add("waiting");
    a.title = p.name;
    return a;
  }));
}

function startSecs(target, ms, dangerAt = 0) {
  clearInterval(secsTimer);
  const end = Date.now() + (ms || 0);
  const tick = () => {
    const left = Math.max(0, Math.ceil((end - Date.now()) / 1000));
    target.textContent = left;
    target.classList.toggle("danger", left > 0 && left <= dangerAt);
    if (target.id === "q-secs") $("#question").classList.toggle("hurry", left > 0 && left <= 5);
    if (left === 0) clearInterval(secsTimer);
  };
  tick();
  secsTimer = setInterval(tick, 250);
}

function renderReveal(s, changed) {
  clearInterval(secsTimer);
  $("#r-round").textContent = roundLabel(s);
  $("#r-code").textContent = s.code;
  $("#r-cat").textContent = s.question.category;
  $("#r-prompt").textContent = s.question.prompt;
  const song = s.reveal.song;
  $("#reveal").classList.toggle("song", !!song);
  $("#r-prompt").classList.toggle("hidden", !!song);
  $("#r-song").classList.toggle("hidden", !song);
  if (song) {
    $("#r-art").src = song.artwork_url || "";
    $("#r-art").classList.toggle("hidden", !song.artwork_url);
    $("#r-title").textContent = song.title;
    $("#r-artist").textContent = `${song.artist} · ${song.year}`;
  }
  const byId = Object.fromEntries(s.players.map((p) => [p.id, p]));
  $("#r-choices").replaceChildren(...s.question.choices.map((text, i) => {
    const tile = choiceTile(text, i);
    tile.classList.add(i === s.reveal.correct ? "right" : "dim");
    const pickers = el("div", { class: "pickers" });
    for (const r of s.reveal.results) {
      if (r.choice === i && byId[r.player_id]) pickers.append(TBX.avatar(byId[r.player_id], "5vh"));
    }
    tile.append(pickers);
    return tile;
  }));
  if (changed) {
    TBX.runTimer($("#r-timer"), s.ms_left, s.phase_ms);
    enter($("#reveal"));
    // Points float up over the right answer for everyone who got it.
    const right = $("#r-choices").children[s.reveal.correct];
    const best = Math.max(0, ...s.reveal.results.map((r) => r.points));
    if (right && best > 0) right.append(el("span", { class: "float-pts" }, `+${best}`));
    // Let the song play out under the reveal once the host has said the answer.
    const clip = s.question.clip;
    if (clip) {
      voice.afterLine(s, () => {
        if (!state || state.phase !== "reveal" || state.round !== s.round) return;
        tune.play(clip.preview_url, clip.start_ms, null, { volume: 0.8 });
      });
    }
  }
}

// ---------- Speed hangman ----------

function renderHangman(s, changed) {
  const h = s.hangman;
  const live = s.phase === "question";
  const before = prev && prev.hangman && !changed ? prev.hangman : null;
  const byId = Object.fromEntries(s.players.map((p) => [p.id, p]));
  $("#h-round").textContent = roundLabel(s);
  $("#h-code").textContent = s.code;
  $("#h-cat").textContent = h.category;

  // The board: one group per word so words never break across lines.
  const board = $("#h-board");
  board.classList.toggle("long", h.board.length > 18);
  let i = 0;
  board.replaceChildren(...h.board.split(" ").map((word) => {
    const group = el("div", { class: "hm-word" });
    for (const ch of word) {
      const wasBlank = before && before.board[i] === "_";
      const tile = el("div", { class: "tile" }, ch === "_" ? "" : ch);
      if (ch === "_") tile.classList.add("blank");
      else if (!/[A-Za-z]/.test(ch)) tile.classList.add("punct");
      else if (wasBlank && live) tile.classList.add("pop");
      // On the reveal, letters the solver filled in go green, or red if nobody got it.
      if (!live && /[A-Za-z]/.test(ch) && !h.hits.includes(ch.toUpperCase())) {
        tile.classList.add(h.outcome === "solved" ? "solved" : "missed");
      }
      group.append(tile);
      i++;
    }
    i++; // the space
    return group;
  }));

  const gallows = $("#h-gallows");
  gallows.querySelectorAll(".part").forEach((part, n) => part.classList.toggle("on", n < h.strikes));
  gallows.classList.toggle("dead", h.outcome === "hanged");
  if (before && h.strikes > before.strikes) {
    gallows.classList.remove("shake");
    void gallows.offsetWidth;
    gallows.classList.add("shake");
  }

  $("#h-letters").replaceChildren(..."ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("").map((c) =>
    el("span", { class: h.hits.includes(c) ? "hit" : h.misses.includes(c) ? "miss" : "" }, c)));

  $("#h-feed").replaceChildren(...h.feed.slice(-4).reverse().map((g) => {
    const p = byId[g.player_id];
    if (!p) return null;
    let text, cls;
    if (g.solve) [text, cls] = g.hit ? ["SOLVED IT!", "win"] : [`tried "${g.guess}"`, "bad"];
    else [text, cls] = g.hit ? [`${g.guess}${g.count > 1 ? " ×" + g.count : ""}`, "good"] : [g.guess, "bad"];
    return el("div", {}, TBX.avatar(p, "3.4vh"), el("span", {}, p.name), el("span", { class: cls }, text));
  }).filter(Boolean));

  $("#h-scores").replaceChildren(...sortedPlayers(s).map((p) =>
    el("div", {}, TBX.avatar(p, "3.4vh"), el("span", {}, p.name), el("b", {}, p.score.toLocaleString()))));

  const banner = $("#h-banner");
  banner.className = "hm-banner " + (h.outcome || "");
  const solver = byId[h.solved_by];
  banner.textContent = h.outcome === "solved" ? `${solver ? solver.name : "Someone"} solved it!`
    : h.outcome === "hanged" ? "Hanged!" : h.outcome === "time" ? "Time's up!" : "";

  if (changed) {
    TBX.runTimer($("#h-timer"), s.ms_left, s.phase_ms);
    if (live) {
      startSecs($("#h-secs"), s.ms_left, 10);
      sweepCard(s);
    } else {
      clearInterval(secsTimer);
      $("#h-secs").textContent = "";
      $("#h-secs").classList.remove("danger");
    }
  }
}

function sortedPlayers(s) { return [...s.players].sort((a, b) => b.score - a.score); }

function renderScores(s, changed) {
  $("#s-round").textContent = roundLabel(s);
  $("#s-code").textContent = s.code;
  const results = s.reveal ? s.reveal.results : s.hangman ? s.hangman.results : [];
  const gains = Object.fromEntries(results.map((r) => [r.player_id, r.points]));
  let rank = 0, last = null;
  const n = s.players.length;
  const board = $("#s-board");
  board.style.setProperty("--rows", n > 8 ? Math.ceil(n / 2) : Math.max(n, 1));
  // Where everyone stood before this question, so rows can slide from old place to new.
  const before = [...s.players].sort((a, b) => (b.score - (gains[b.id] || 0)) - (a.score - (gains[a.id] || 0))).map((p) => p.id);
  const rows = sortedPlayers(s).map((p, i) => {
    if (p.score !== last) { rank = i + 1; last = p.score; }
    const moved = before.indexOf(p.id) - i;
    const score = el("span", { class: "score" }, p.score.toLocaleString());
    const row = el("div", { class: "row" + (changed && moved > 0 ? " up" : changed && moved < 0 ? " down" : ""), "data-moved": moved },
      el("span", { class: "rank" }, `#${rank}`),
      TBX.avatar(p, "1.3em"),
      el("span", { class: "name" }, p.name),
      gains[p.id] ? el("span", { class: "gain", style: gains[p.id] < 0 ? "color:var(--bad)" : "" }, `${gains[p.id] > 0 ? "+" : ""}${gains[p.id]}`) : null,
      score);
    if (changed && gains[p.id]) countUp(score, p.score - gains[p.id], p.score);
    return row;
  });
  board.replaceChildren(...rows);
  if (!changed) return;
  // One column: slide from the old position. Two columns: just pop in.
  if (n <= 8) {
    const step = rows[1] ? rows[1].offsetTop - rows[0].offsetTop : 0;
    rows.forEach((r) => { r.style.transition = "none"; r.style.transform = `translateY(${Number(r.dataset.moved) * step}px)`; });
    void board.offsetWidth;
    setTimeout(() => rows.forEach((r) => { r.style.transition = ""; r.style.transform = ""; }), 500);
  } else {
    rows.forEach((r, i) => { r.classList.add("pop"); r.style.animationDelay = `${i * 60}ms`; });
  }
}

function countUp(node, from, to) {
  const t0 = performance.now() + 500;
  const step = (t) => {
    const k = Math.min(1, Math.max(0, (t - t0) / 1200));
    node.textContent = Math.round(from + (to - from) * k).toLocaleString();
    if (k < 1) requestAnimationFrame(step);
  };
  node.textContent = from.toLocaleString();
  requestAnimationFrame(step);
}

function renderFinal(s, changed) {
  clearInterval(secsTimer);
  const ranked = sortedPlayers(s);
  const top = ranked.slice(0, 3);
  const heights = ["26vh", "18vh", "12vh"];
  const medal = ["🥇", "🥈", "🥉"];
  // Arrange 2nd, 1st, 3rd like a real podium.
  const order = [1, 0, 2].filter((i) => top[i]);
  $("#f-title").textContent = top[0] ? `${top[0].name} wins!` : "Game over";
  $("#f-podium").replaceChildren(...order.map((i) => el("div", { class: "step pop" },
    TBX.avatar(top[i], "10vh"),
    el("div", {}, top[i].name),
    el("div", { class: "muted" }, top[i].score.toLocaleString()),
    i === 0 ? el("div", { class: "crown" }, "👑") : null,
    el("div", { class: "block", style: `height:${heights[i]};background:${top[i].color}55` }, medal[i]))));
  $("#f-rest").textContent = ranked.slice(3).map((p, i) => `#${i + 4} ${p.name} (${p.score.toLocaleString()})`).join("   ·   ");
  if (changed && top[0]) confetti();
}

// ---------- Controls: mouse optional, keyboard / TV remote friendly ----------

function primaryAction() {
  if (!state) return $("#setup").classList.contains("hidden") ? null : $("#create").click();
  if (state.phase === "lobby") {
    if (state.settings.mode === "songs" && state.players.length) toast("Cueing up the tunes…");
    send({ type: "start" });
  }
  else if (state.phase === "final") send({ type: "play_again" });
  else send({ type: "next" });
}

$("#start-btn").addEventListener("click", () => primaryAction());
$("#sound-btn").addEventListener("click", () => tune.unlock());
document.addEventListener("pointerdown", () => tune.unlock());
$("#again-btn").addEventListener("click", () => send({ type: "play_again" }));
$("#skip-btn").addEventListener("click", () => send({ type: "next" }));

function toggleVoice() {
  voice.on = !voice.on;
  if (!voice.on) voice.hush();
  TBX.save("tbx:voice", voice.on);
  $("#mute-btn").textContent = voice.on ? "🔊 Host voice" : "🔇 Host voice";
}
$("#mute-btn").addEventListener("click", toggleVoice);
$("#mute-btn").textContent = voice.on ? "🔊 Host voice" : "🔇 Host voice";

function nextVoice() {
  const name = voice.next();
  toast(`Host voice: ${name}`);
  voice.hush();
  voice.say(`Hi, I'm ${(state && state.host && state.host.name) || "your host"}. How do I sound?`);
}
$("#voice-btn").addEventListener("click", nextVoice);

function toggleFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen();
  else document.documentElement.requestFullscreen && document.documentElement.requestFullscreen().catch(() => {});
}
$("#fs-btn").addEventListener("click", toggleFullscreen);

document.addEventListener("keydown", (e) => {
  if (e.target.tagName === "INPUT") return;
  // On the setup screen, let a focused button (e.g. picked with a remote's arrows) click itself.
  if (!state && e.target.tagName === "BUTTON") return;
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    // If the browser blocked the clip, this press turns sound on instead of skipping.
    if (tune.unlock()) return;
    primaryAction();
  } else if ((e.key === "r" || e.key === "R") && state && state.phase === "question") {
    tune.replay();
  } else if (e.key === "m" || e.key === "M") toggleVoice();
  else if (e.key === "f" || e.key === "F") toggleFullscreen();
  else if (e.key === "v" || e.key === "V") nextVoice();
});

// ---------- Boot ----------

(async function boot() {
  renderSetup();
  await TBX.loadCharacters();
  try {
    const cfg = await (await fetch("/api/config")).json();
    if (cfg.public_url) joinBase = cfg.public_url.replace(/\/$/, "");
  } catch { /* fall back to this page's address */ }

  // Coming back after a refresh: rejoin the same room if it's still alive.
  const saved = TBX.load(HOST_KEY);
  if (saved) {
    const res = await fetch(`/api/rooms/${saved.code}`).catch(() => null);
    if (res && res.ok) return join(saved.code, saved.token);
    TBX.save(HOST_KEY, null);
  }
  show("setup");
})();
