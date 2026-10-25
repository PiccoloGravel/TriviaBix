// The TV screen. After "Start a room" it needs no mouse: the server moves the
// game along on timers, and Enter/Space (a remote's OK button) skips ahead.

const { $, el, show } = TBX;
const HOST_KEY = "tbx:host";

let settings = { rounds: 10, max_rating: "PG" };
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
  },
  hush() { if ("speechSynthesis" in window) speechSynthesis.cancel(); },
};

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
const nameWithCharacter = (p) => {
  const ch = TBX.characters[p.character];
  return ch && Math.random() < 0.5 ? `${p.name} the ${ch.name}` : p.name;
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
}

document.querySelectorAll("[data-rounds]").forEach((b) =>
  b.addEventListener("click", () => { settings.rounds = Number(b.dataset.rounds); renderSetup(); }));
document.querySelectorAll("[data-rating]").forEach((b) =>
  b.addEventListener("click", () => { settings.max_rating = b.dataset.rating; renderSetup(); }));

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
  $("#lobby-settings").textContent = `${s.settings.rounds} questions · rated ${s.settings.max_rating}`;

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

  if (prev && prev.phase === "lobby") {
    for (const p of s.players) {
      if (!prev.players.some((q) => q.id === p.id)) voice.say(line("join", nameWithCharacter(p)));
    }
  }
}

function choiceTile(text, i) {
  return el("div", { class: `choice choice-${i}` }, el("span", { class: "shape" }, TBX.SHAPES[i]), el("span", {}, text));
}

function roundLabel(s) { return `Question ${s.round} of ${s.total_rounds}`; }

function renderQuestion(s, changed) {
  $("#q-round").textContent = roundLabel(s);
  $("#q-code").textContent = s.code;
  $("#q-cat").textContent = s.question.category;
  $("#q-prompt").textContent = s.question.prompt;
  if (changed) {
    $("#q-choices").replaceChildren(...s.question.choices.map(choiceTile));
    TBX.runTimer($("#q-timer"), s.ms_left, s.phase_ms);
    startSecs(s.ms_left);
    voice.hush();
    if (s.round === 1) voice.say(line("start"));
    voice.say(s.question.prompt);
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
    const answer = s.question.choices[s.reveal.correct];
    const winners = s.reveal.results.filter((r) => r.points > 0).sort((a, b) => b.points - a.points);
    let quip;
    if (winners.length === 0) quip = line("nobody");
    else if (winners.length === s.players.length && s.players.length > 1) quip = line("everyone");
    else quip = line(s.settings.max_rating !== "PG" && Math.random() < 0.5 ? "spicyFastest" : "fastest", byId[winners[0].player_id].name);
    voice.say(`It's ${answer}! ${quip}`);
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
  if (state.phase === "lobby") send({ type: "start" });
  else if (state.phase === "final") send({ type: "play_again" });
  else send({ type: "next" });
}

$("#start-btn").addEventListener("click", () => send({ type: "start" }));
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
    primaryAction();
  } else if (e.key === "m" || e.key === "M") toggleVoice();
  else if (e.key === "f" || e.key === "F") toggleFullscreen();
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
