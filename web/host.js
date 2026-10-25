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

// ---------- Cheeky host voice (browser text-to-speech) ----------

const voice = {
  on: TBX.load("tbx:voice") !== false,
  say(text) {
    if (!this.on || !("speechSynthesis" in window)) return;
    const u = new SpeechSynthesisUtterance(text);
    u.rate = 1.05;
    u.pitch = 1.1;
    speechSynthesis.speak(u);
    return u;
  },
  hush() { if ("speechSynthesis" in window) speechSynthesis.cancel(); },
};

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

const pick = (lines) => lines[Math.floor(Math.random() * lines.length)];
const LINES = {
  join: [
    "{n} has entered the building.",
    "Welcome, {n}. Lower your expectations.",
    "Oh good, {n} is here. Now it's a party.",
    "{n} joined. I've heard things about you, {n}.",
  ],
  start: [
    "Phones out, brains on. Let's go!",
    "Welcome to TriviaBix, where friendships go to die.",
    "Let's find out who here actually reads.",
  ],
  nobody: ["Not a single one of you. Incredible.", "Zero. Nobody. I'm embarrassed for all of us."],
  everyone: ["Everyone got it. Too easy, I'll be meaner next time.", "A clean sweep. Don't get cocky."],
  fastest: ["{n} got it fastest. Show-off.", "{n} was quickest on the draw.", "Big points for {n}."],
  spicyFastest: ["{n} knew that one a little too well.", "{n} answered that suspiciously fast."],
  win: [
    "{n} wins! Everyone else, there's always next time. Probably.",
    "All hail {n}, the big brain of the room!",
    "{n} takes the crown. Speech! Actually, no, please don't.",
  ],
};
const line = (key, name) => pick(LINES[key]).replace(/\{n\}/g, name || "");

// ---------- Setup ----------

function renderSetup() {
  document.querySelectorAll("[data-rounds]").forEach((b) => {
    b.classList.toggle("secondary", Number(b.dataset.rounds) !== settings.rounds);
  });
  document.querySelectorAll("[data-rating]").forEach((b) => {
    b.classList.toggle("secondary", b.dataset.rating !== settings.max_rating);
  });
  document.querySelectorAll("[data-mode]").forEach((b) => {
    b.classList.toggle("secondary", b.dataset.mode !== settings.mode);
  });
  document.querySelectorAll("[data-clip]").forEach((b) => {
    b.classList.toggle("secondary", Number(b.dataset.clip) !== settings.clip_ms);
  });
  $("#clip-opts").classList.toggle("hidden", settings.mode !== "songs");
  $("#rounds-label").textContent = settings.mode === "songs" ? "Songs" : "Questions";
}

document.querySelectorAll("[data-rounds]").forEach((b) =>
  b.addEventListener("click", () => { settings.rounds = Number(b.dataset.rounds); renderSetup(); }));
document.querySelectorAll("[data-rating]").forEach((b) =>
  b.addEventListener("click", () => { settings.max_rating = b.dataset.rating; renderSetup(); }));
document.querySelectorAll("[data-mode]").forEach((b) =>
  b.addEventListener("click", () => { settings.mode = b.dataset.mode; renderSetup(); }));
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
    voice.say("Room " + body.code.split("").join(" ") + " is open. Grab your phones!");
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
  $("#skip-btn").classList.toggle("hidden", !["question", "reveal", "scoreboard"].includes(s.phase));
  if (phaseChanged) {
    tune.stop();
    clearTimeout(replayTimer);
  }
  tune.prefetch(s.next_clip);
  show(s.phase);
  ({ lobby: renderLobby, question: renderQuestion, reveal: renderReveal, scoreboard: renderScores, final: renderFinal })[s.phase](s, phaseChanged);
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
    : `${s.settings.rounds} questions · rated ${s.settings.max_rating}`;

  const slots = $("#slots");
  slots.replaceChildren();
  for (const p of s.players) {
    const slot = el("div", { class: "slot pop" + (p.connected ? "" : " away") },
      TBX.avatar(p, "8vh"),
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

  if (prev && prev.phase === "lobby") {
    for (const p of s.players) {
      if (!prev.players.some((q) => q.id === p.id)) voice.say(line("join", p.name));
    }
  }
}

function choiceTile(text, i) {
  // Song answers are "Title — Artist"; show the artist smaller under the title.
  const [title, artist] = state && state.question && state.question.kind === "song" ? text.split(" — ") : [text];
  return el("div", { class: `choice choice-${i}` }, el("span", { class: "shape" }, TBX.SHAPES[i]),
    artist ? el("span", { class: "song" }, el("span", {}, title), el("small", {}, artist)) : el("span", {}, text));
}

function roundLabel(s) {
  return `${s.question && s.question.kind === "song" ? "Song" : "Question"} ${s.round} of ${s.total_rounds}`;
}

/** Plays the clip, then once more halfway through the timer for anyone who blinked. */
function playClip(s) {
  const clip = s.question.clip;
  if (!clip) return;
  const listen = $("#q-listen");
  listen.textContent = "Listen…";
  tune.play(clip.preview_url, clip.start_ms, clip.clip_ms, { onEnd: () => { listen.textContent = ""; } });
  clearTimeout(replayTimer);
  replayTimer = setTimeout(() => {
    if (!state || state.phase !== "question") return;
    listen.textContent = "One more time…";
    tune.play(clip.preview_url, clip.start_ms, clip.clip_ms, { onEnd: () => { listen.textContent = "That's all you get!"; } });
  }, (s.ms_left || 0) / 2);
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
    startSecs(s.ms_left);
    voice.hush();
    // The host keeps quiet during a song round so it doesn't talk over the clip.
    if (song) playClip(s);
    else {
      if (s.round === 1) voice.say(line("start"));
      voice.say(s.question.prompt);
    }
  }
  $("#q-answered").replaceChildren(...s.players.map((p) => {
    const a = TBX.avatar(p, "6vh");
    if (!p.answered) a.classList.add("waiting");
    a.title = p.name;
    return a;
  }));
}

function startSecs(ms) {
  clearInterval(secsTimer);
  const end = Date.now() + (ms || 0);
  const tick = () => {
    const left = Math.max(0, Math.ceil((end - Date.now()) / 1000));
    $("#q-secs").textContent = left;
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
    voice.hush();
    const answer = song ? `${song.title}, by ${song.artist}` : s.question.choices[s.reveal.correct];
    const winners = s.reveal.results.filter((r) => r.points > 0).sort((a, b) => b.points - a.points);
    let quip;
    if (winners.length === 0) quip = line("nobody");
    else if (winners.length === s.players.length && s.players.length > 1) quip = line("everyone");
    else quip = line(s.settings.max_rating !== "PG" && Math.random() < 0.5 ? "spicyFastest" : "fastest", byId[winners[0].player_id].name);
    const said = voice.say(`It's ${answer}! ${quip}`);
    // Let the song play out under the reveal once the host has said the answer.
    const clip = s.question.clip;
    if (clip) {
      let started = false;
      const playSong = () => {
        if (started || !state || state.phase !== "reveal" || state.round !== s.round) return;
        started = true;
        tune.play(clip.preview_url, clip.start_ms, null, { volume: 0.8 });
      };
      if (said) {
        said.onend = playSong;
        setTimeout(playSong, 4000); // some TV browsers never fire onend
      } else playSong();
    }
  }
}

function sortedPlayers(s) { return [...s.players].sort((a, b) => b.score - a.score); }

function renderScores(s) {
  $("#s-round").textContent = roundLabel(s);
  $("#s-code").textContent = s.code;
  const gains = Object.fromEntries((s.reveal ? s.reveal.results : []).map((r) => [r.player_id, r.points]));
  let rank = 0, last = null;
  const n = s.players.length;
  $("#s-board").style.setProperty("--rows", n > 8 ? Math.ceil(n / 2) : Math.max(n, 1));
  $("#s-board").replaceChildren(...sortedPlayers(s).map((p, i) => {
    if (p.score !== last) { rank = i + 1; last = p.score; }
    return el("div", { class: "row pop", style: `animation-delay:${i * 60}ms` },
      el("span", { class: "rank" }, `#${rank}`),
      TBX.avatar(p, "1.3em"),
      el("span", { class: "name" }, p.name),
      gains[p.id] ? el("span", { class: "gain" }, `+${gains[p.id]}`) : null,
      el("span", { class: "score" }, p.score.toLocaleString()));
  }));
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
    el("div", { class: "block", style: `height:${heights[i]};background:${top[i].color}55` }, medal[i]))));
  $("#f-rest").textContent = ranked.slice(3).map((p, i) => `#${i + 4} ${p.name} (${p.score.toLocaleString()})`).join("   ·   ");
  if (changed && top[0]) {
    voice.hush();
    voice.say(line("win", top[0].name));
  }
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
});

// ---------- Boot ----------

(async function boot() {
  renderSetup();
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
