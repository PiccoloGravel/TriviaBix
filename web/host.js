// The TV screen. It's display only: it opens a room by itself, the VIP runs the game from
// their phone, and the server moves things along on timers. Enter/Space still skips ahead.

const { $, el, show } = TBX;
const HOST_KEY = "tbx:host";

let conn = null;
let state = null;
let prev = null;
let joinBase = location.origin;
let secsTimer = null;

// ---------- The host: voice, speech bubble and callout graphics ----------
// The server writes the script (see server/host-script.json) and sends the current line in
// `state.host`; this page says it out loud and draws the graphics that go with it. With a TTS
// service (see tts/), each piece of the line comes with recorded audio of Bix; anything without
// audio, or whose audio isn't ready in time, is spoken by the browser instead.

/** Ming's inline sound effects, like [Laughter]. Only the TTS service can perform them. */
const SOUND_TAGS = /\[(Breathing|Cough|Sigh|Laughter|Uhm|Surprise-ah|Surprise-oh|Dissatisfaction-hnn|Confirmation-en|Question-(?:ei|ah|en|oh|huh))\]/g;
const plain = (text) => text.replace(SOUND_TAGS, "").replace(/\s{2,}/g, " ").trim();
/** How long to wait for a line's audio before the browser reads it instead. */
const AUDIO_WAIT_MS = 3500;
const MING = "ming";

const voice = {
  /** How fast Bix talks (Settings → Speaking speed); his lines play faster at the same pitch. */
  speed: 1,
  on: TBX.load("tbx:voice") !== false,
  pick: TBX.load("tbx:voice-name"),
  chosen: null,
  queue: 0,
  /** Set at boot when the server has a TTS service. */
  ming: false,
  /** Someone pressed V past Bix's own voice to a browser one. */
  mingOff: TBX.load("tbx:ming-off") === true,
  token: 0,
  playing: null,
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
  /** Bix's own voice from the TTS service, unless someone picked a browser voice with V. */
  usingMing() {
    return this.ming && !this.mingOff;
  },
  /** Cycles to the next voice; returns its name. Bix's own voice comes first. */
  next() {
    const names = [...(this.ming ? [MING] : []), ...this.ranked().map((v) => v.name)];
    if (!names.length) return "the default voice";
    const current = this.usingMing() ? MING : this.chosen && this.chosen.name;
    const pick = names[(names.indexOf(current) + 1) % names.length];
    if (this.ming) {
      this.mingOff = pick !== MING;
      TBX.save("tbx:ming-off", this.mingOff);
    }
    if (pick === MING) return "Bix (Ming TTS)";
    this.pick = pick;
    TBX.save("tbx:voice-name", pick);
    this.choose();
    return this.pick;
  },
  talking(on) {
    hostBar.classList.toggle("talking", on);
  },
  say(text, cameo) {
    text = text && plain(text);
    if (!this.on || !text || !("speechSynthesis" in window)) return;
    if (!this.chosen) this.choose();
    const u = new SpeechSynthesisUtterance(text);
    if (this.chosen) u.voice = this.chosen;
    const rate = cameo ? cameo.rate : 1;
    u.rate = 1.05 * this.speed * rate;
    u.pitch = 1.1 * (cameo && cameo.shift ? rate : 1);
    u.onstart = () => { this.queue++; this.talking(true); };
    u.onend = u.onerror = () => { if (--this.queue <= 0) { this.queue = 0; this.talking(false); } };
    speechSynthesis.speak(u);
    return u;
  },
  /** Starts loading a piece's audio. `ready` settles true once it can play, false if it can't. */
  load(url) {
    const audio = new Audio();
    audio.preload = "auto";
    const ready = new Promise((resolve) => {
      audio.oncanplaythrough = () => resolve(true);
      audio.onerror = () => resolve(false);
    });
    audio.src = url;
    return { audio, ready };
  },
  /**
   * Says a line made of `parts` ({text, audio}) in order, then calls `done`. All the audio starts
   * loading at once, so later pieces get made while the first one plays.
   */
  speak(parts, done) {
    this.hush();
    const token = this.token;
    const live = () => token === this.token;
    const clips = parts.map((p) => (this.on && this.usingMing() && p.audio ? this.load(p.audio) : null));
    let i = 0;
    const next = () => {
      if (!live()) return;
      // The last costume stays on with the words until the next line.
      if (i >= parts.length) return done && done();
      const text = parts[i].text;
      const cameo = parts[i].cameo || null;
      const clip = clips[i++];
      dress(cameo);
      if (!clip) return this.browser(text, next, cameo);
      const late = new Promise((resolve) => setTimeout(() => resolve(false), AUDIO_WAIT_MS));
      Promise.race([clip.ready, late]).then((ok) => {
        if (!live()) return;
        if (!ok) {
          clip.audio.src = "";
          return this.browser(text, next, cameo);
        }
        const a = clip.audio;
        // A character cameo plays at its own speed; some let the pitch move with it.
        a.preservesPitch = !(cameo && cameo.shift);
        a.playbackRate = this.speed * (cameo ? cameo.rate : 1);
        this.playing = a;
        let finished = false;
        const end = () => { if (!finished) { finished = true; this.talking(false); next(); } };
        a.onended = end;
        a.onerror = end;
        this.talking(true);
        a.play().catch(() => { this.talking(false); finished = true; this.browser(text, next, cameo); });
      });
    };
    next();
  },
  /** Reads `text` with the browser's voice, then calls `next`, even on TVs that never say it's done. */
  browser(text, next, cameo) {
    const u = this.say(text, cameo);
    let called = false;
    const go = () => { if (!called) { called = true; next(); } };
    if (!u) return setTimeout(go, 300);
    u.addEventListener("end", go);
    setTimeout(go, 1500 + plain(text).length * 80);
  },
  /** Runs `fn` once the host has finished the line in state `s`, or soon if there's nothing to wait for. */
  afterLine(s, fn) {
    let done = false;
    const go = () => { if (!done) { done = true; fn(); } };
    const text = s.host ? s.host.text : "";
    if (!this.on || !text || !(this.usingMing() || "speechSynthesis" in window)) return setTimeout(go, 600);
    this.pending = { id: s.host.id, go };
    // In case the end of the line never gets reported, give up after a rough speaking time.
    setTimeout(go, (this.usingMing() ? AUDIO_WAIT_MS : 0) + 1500 + text.length * 80);
  },
  /** Called when the last part of host line `id` has been spoken. */
  lineDone(id) {
    if (this.pending && this.pending.id === id) this.pending.go();
  },
  hush() {
    this.token++;
    if ("speechSynthesis" in window) speechSynthesis.cancel();
    if (this.playing) {
      this.playing.pause();
      this.playing = null;
    }
    this.queue = 0;
    this.talking(false);
    dress(null);
  },
};
if ("speechSynthesis" in window) speechSynthesis.addEventListener("voiceschanged", () => voice.choose());

const hostBar = $("#host-bar");
const bixFace = Bix.create("happy");
$("#host-face").append(bixFace);
let spokenId = null;
let typeTimer = null;
let hostName = "Bix";

/**
 * Puts Bix in a character's costume for a character cameo (`{id, name, emoji}` from the server),
 * or back in his own clothes with `null`.
 */
function dress(cameo) {
  const face = $("#host-face");
  if (!face) return;
  const was = face.dataset.cameo || "";
  const now = cameo ? cameo.id : "";
  if (was === now) return;
  face.dataset.cameo = now;
  $("#host-as").textContent = cameo ? `${hostName} as ${cameo.name}` : "";
  hostBar.classList.toggle("cameo", !!cameo);
  $("#host-costume").textContent = cameo ? cameo.emoji : "";
  if (cameo) {
    hostBar.classList.remove("react");
    void hostBar.offsetWidth;
    hostBar.classList.add("react");
  }
}

/** Shows or hides Bix and his speech bubble, as set in the admin panel (Settings → Game show). */
function hostDisplay(s) {
  const tv = s.tv || { bix: true, captions: true };
  hostBar.classList.toggle("off", !tv.bix);
  // With the TV's voice muted, the words are the only way to hear him.
  hostBar.classList.toggle("no-words", !tv.captions && voice.on);
}

/** Speaks and shows a new host line, once per line. `extra` ({text, audio}) is read out after it (the question). */
function hostSays(s, extra) {
  const h = s.host;
  hostDisplay(s);
  if (!h) {
    hostBar.classList.add("quiet");
    return;
  }
  if (h.id === spokenId) return;
  spokenId = h.id;
  hostName = h.name;
  $("#host-face").dataset.name = h.name;
  hostBar.classList.remove("quiet");
  const bubble = $("#host-bubble");
  bubble.classList.remove("fresh");
  void bubble.offsetWidth;
  bubble.classList.add("fresh");
  // Type the line out at roughly speaking speed.
  clearInterval(typeTimer);
  const said = plain(h.text);
  let shown = 0;
  typeTimer = setInterval(() => {
    shown = Math.min(said.length, shown + 2);
    bubble.textContent = said.slice(0, shown);
    bubble.scrollTop = bubble.scrollHeight;
    if (shown >= said.length) clearInterval(typeTimer);
  }, voice.on ? 140 : 30);
  const parts = h.voice && h.voice.length ? [...h.voice] : [{ text: h.text }];
  if (extra) parts.push(extra);
  const voiced = voice.on;
  const finished = new Promise((resolve) => voice.speak(parts, () => { voice.lineDone(h.id); resolve(); }));
  showCallouts(s);
  const scene = react(s);
  // Tell the server once Bix is done and his cutscene is over, so it can move on without waiting
  // out its own guess at how long he talks. Muted, people read the line, so it keeps its guess.
  if (voiced) Promise.all([finished, scene]).then(() => { if (voice.on) send({ type: "spoken", id: h.id }); });
}

// ---------- Cutscenes and Bix's reactions ----------
// The server tags each line with a cue; web/cutscenes/index.json maps cues to scenes and faces.

/** Pulls Bix's face for the line's cue and plays its cutscene, if it has one. Resolves when the scene is over. */
function react(s) {
  const cue = s.host.cue;
  Bix.mood(bixFace, Cutscenes.moodFor(cue) || "happy");
  hostBar.classList.remove("react");
  void hostBar.offsetWidth;
  hostBar.classList.add("react");
  // Not on the first state after a refresh: a scene popping up mid-question would just be in the way.
  return (cue && prev && Cutscenes.cue(cue, cutsceneContext(s))) || Promise.resolve();
}

/** What a scene needs to fill in names, faces and numbers. */
function cutsceneContext(s) {
  const h = s.host;
  const byId = Object.fromEntries(s.players.map((p) => [p.id, p]));
  // The player the moment is about: the callout matching the cue, or else the first one about someone.
  const kind = h.cue === "winner" ? "winner" : h.cue;
  const callout = h.callouts.find((c) => c.kind === kind) || h.callouts.find((c) => c.player_id != null) || null;
  const subject = callout && callout.player_id != null ? byId[callout.player_id] || null : null;
  let players = h.cue === "intro" ? s.players : sortedPlayers(s);
  if (h.cue === "finale_tie") players = players.filter((p) => p.score === players[0].score);
  const item = s.question || s.hangman;
  const stage = s.stage ? (s.stages || [])[s.stage - 1] : null;
  return {
    players,
    subject,
    // Generated art is rated like questions; a family room never sees an MA picture.
    rating: s.settings.max_rating,
    vars: {
      host: h.name,
      // In trivia rounds, count questions within the current round.
      round: s.stage_question || s.round,
      total: (stage && stage.questions) || s.total_rounds,
      kind: s.hangman || s.settings.mode === "hangman" ? "Puzzle" : s.settings.mode === "songs" ? "Song" : "Question",
      category: item ? item.category : "",
      name: subject ? subject.name : "",
      score: subject ? subject.score.toLocaleString() : "",
      label: callout ? callout.label : "",
    },
  };
}

// ---------- Name That Tune audio ----------
// The TV streams Apple's 30-second preview, or a song from the music library, and plays only a
// slice of it. Browsers block sound until someone has pressed something on the page, so after a
// refresh we ask for one press.

/** Library clips come as /media/... links; <audio>.src always reads back absolute. */
const absUrl = (url) => (url ? new URL(url, location.href).href : url);

const tune = {
  audio: new Audio(),
  next: new Audio(),
  stopTimer: null,
  blocked: false,
  last: null,

  prefetch(url) {
    url = absUrl(url);
    if (!url || this.next.src === url || this.audio.src === url) return;
    this.next.preload = "auto";
    this.next.src = url;
  },

  /** Plays `ms` of `url` from `startMs`, then calls `onEnd`. A null `ms` plays to the end. */
  play(url, startMs, ms, { volume = 1, onEnd } = {}) {
    url = absUrl(url);
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
  round_winner: "🏅",
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

// ---------- Room ----------
// The TV needs no input: it opens a room straight away, and the VIP picks the game on their phone.

async function createRoom() {
  try {
    const res = await fetch("/api/rooms", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    const body = await res.json();
    if (!res.ok) throw new Error(body.error || "Couldn't create a room.");
    TBX.save(HOST_KEY, { code: body.code, token: body.host_token });
    join(body.code, body.host_token);
  } catch (e) {
    show("setup");
    $("#setup-error").textContent = `${e.message} Trying again…`;
    setTimeout(createRoom, 5000);
  }
}

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
      // The room is gone (closed from the admin panel, or the server restarted): open a fresh one.
      TBX.save(HOST_KEY, null);
      toast(`${message} Opening a new room…`);
      createRoom();
    },
    onStatus(ok) { $("#offline").classList.toggle("hidden", ok); },
  });
}

function send(msg) { conn && conn.send(msg); }

function toast(text, kind) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.toggle("info", kind === "info");
  t.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 3000);
}

// ---------- Rendering ----------

function render(s) {
  prev = state;
  state = s;
  const phaseChanged = !prev || prev.phase !== s.phase || prev.round !== s.round;
  if (phaseChanged) {
    Cutscenes.stop();
    $("#callouts").replaceChildren();
    tune.stop();
    clearTimeout(replayTimer);
  }
  tune.prefetch(s.next_clip);
  Bgm.setList(s.playlist);
  if (phaseChanged) phaseSound(s);
  // The playlist can arrive after the phase started (a fresh room, or after a library sync).
  else if (["lobby", "scoreboard", "recap"].includes(s.phase) && !Bgm.blocked) Bgm.start();
  if (s.hangman && (s.phase === "question" || s.phase === "reveal")) {
    show("hangman");
    document.body.classList.add("in-game");
    renderHangman(s, phaseChanged);
    return hostSays(s);
  }
  show(s.phase);
  document.body.classList.toggle("in-game", s.phase !== "lobby" && s.phase !== "final");
  announceLateJoiners(s);
  ({
    lobby: renderLobby, intro: renderIntro, round_intro: renderRoundIntro, question: renderQuestion,
    reveal: renderReveal, scoreboard: renderScores, recap: renderRecap, minigame: renderMinigame, final: renderFinal,
  })[s.phase](s, phaseChanged);
  // The question is read out after the host's banter.
  // In Name That Tune the clip plays instead.
  hostSays(s, s.phase === "question" && s.question.kind !== "song"
    ? { text: s.question.prompt, audio: s.question.prompt_audio } : null);
}

// ---------- Music and stings ----------

/** Background music between rounds, a thinking bed during questions, and a sting for each moment. */
function phaseSound(s) {
  const was = prev ? prev.phase : null;
  const songs = s.settings.mode === "songs";
  if (s.phase !== "question") Sfx.bed.stop();
  switch (s.phase) {
    case "question":
      Bgm.stop();
      // Name That Tune needs quiet for the clip.
      if (!songs) Sfx.bed.play(s.ms_left);
      break;
    case "reveal":
      Bgm.stop();
      if (was === "question" && s.reveal && !s.reveal.results.some((r) => r.choice != null)) Sfx.play("timeup");
      else if (!songs) Sfx.play("reveal");
      break;
    case "intro":
    case "round_intro":
      Bgm.stop();
      Sfx.play("round");
      break;
    case "minigame":
      Bgm.stop();
      Sfx.play("round");
      break;
    case "scoreboard":
    case "recap":
      Sfx.play("scores");
      Bgm.start();
      break;
    case "final":
      Bgm.stop();
      Sfx.play("fanfare");
      setTimeout(() => { if (state && state.phase === "final") Bgm.start(); }, 4000);
      break;
    case "lobby":
      Bgm.start();
      break;
  }
}

Bgm.onTrack = (t) => {
  $("#now-playing").classList.toggle("hidden", !t);
  if (t) $("#now-playing").textContent = `♪ ${t.title} · ${t.artist}`;
};
// Dip the music under the host's voice.
new MutationObserver(() => Bgm.duck(hostBar.classList.contains("talking")))
  .observe(hostBar, { attributes: true, attributeFilter: ["class"] });

function toggleMusic() {
  Sfx.on = !Sfx.on;
  if (Sfx.on) {
    Sfx.unlock();
    if (state && ["lobby", "scoreboard", "recap", "final"].includes(state.phase)) Bgm.start();
  } else Bgm.stop();
  $("#music-btn").textContent = Sfx.on ? "🎵 Music" : "🔇 Music";
}
$("#music-btn").addEventListener("click", toggleMusic);
$("#music-btn").textContent = Sfx.on ? "🎵 Music" : "🔇 Music";

// Players who join mid-game wait for the next question; the host greets them, and the TV says when they're in.
function announceLateJoiners(s) {
  if (!prev || s.phase === "lobby") return;
  for (const p of s.players) {
    if (p.waiting && !prev.players.some((q) => q.id === p.id)) toast(`${p.name} joins at the next question`, "info");
  }
}

// Players who are in the current game (not waiting for the next question).
const playing = (s) => s.players.filter((p) => !p.waiting);
const teamOf = (s, p) => s.teams.find((t) => t.id === p.team);

function renderIntro(s, changed) {
  if (!changed) return;
  const length = s.stages.length
    ? `${s.stages.length} rounds · ${s.total_rounds} questions`
    : `${s.total_rounds} ${{ hangman: "puzzles", songs: "songs" }[s.settings.mode] || "questions"}`;
  $("#i-tagline").textContent = `${length} · rated ${s.settings.max_rating} · ${s.players.length} contestants`;
  $("#i-lineup").replaceChildren(...s.players.map((p, i) =>
    el("div", { style: `--i:${i}` }, TBX.avatar(p, "11vh"), el("div", {}, p.name))));
}

function renderLobby(s, changed) {
  if (!changed && prev && s.players.length > prev.players.length) Sfx.play("join");
  const url = joinBase.replace(/^https?:\/\//, "");
  $("#join-url").textContent = url;
  $("#join-url").parentNode.classList.toggle("long", url.length > 24);
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
    : `${s.settings.trivia_rounds} rounds of ${s.settings.per_round} questions, easy to hard${s.settings.trivia_rounds > 3 ? " plus a final" : ""}${s.settings.minigames ? " · mini games between rounds" : ""} · rated ${s.settings.max_rating}`;
  if (s.settings.teams) $("#lobby-settings").textContent += ` · ${s.settings.teams} teams`;

  const slots = $("#slots");
  slots.replaceChildren();
  for (const p of s.players) {
    const team = teamOf(s, p);
    const slot = el("div", { class: "slot pop" + (p.connected ? "" : " away"), style: team ? `box-shadow: inset 0 -1vh 0 ${team.color}` : "" },
      TBX.avatar(p, "10vh"),
      el("div", { class: "name" }, p.name),
      p.id === s.vip ? el("span", { class: "vip-tag" }, "VIP") : null,
      team ? el("span", { class: "team-tag", style: `color:${team.color}` }, team.name) : null);
    slots.append(slot);
  }
  const shown = Math.max(8, s.players.length + (s.players.length < s.max_players ? 1 : 0));
  for (let i = s.players.length; i < Math.min(shown, s.max_players); i++) {
    slots.append(el("div", { class: "slot empty" }, `Player ${i + 1}`));
  }

  const vip = s.players.find((p) => p.id === s.vip);
  $("#lobby-hint").textContent = s.players.length === 0
    ? `Scan the code or go to ${url} on your phone. Up to ${s.max_players} players, and the first one in is the VIP.`
    : `${s.players.length}/${s.max_players} in. ${vip ? vip.name + " is the VIP: pick the game and start it from your phone." : ""}`;
}

function choiceTile(text, i) {
  // Song answers are "Title — Artist"; show the artist smaller under the title.
  const [title, artist] = state && state.question && state.question.kind === "song" ? text.split(" — ") : [text];
  return el("div", { class: `choice choice-${i}`, style: `--i:${i}` }, el("span", { class: "shape" }, TBX.SHAPES[i]),
    artist ? el("span", { class: "song" }, el("span", {}, title), el("small", {}, artist)) : el("span", {}, text));
}

function roundLabel(s) {
  const st = stageOf(s);
  if (st && s.stage_question) {
    return `${stageName(st)} · Question ${s.stage_question} of ${st.questions} · ${mult(st.points_pct)} points`;
  }
  const kind = s.hangman ? "Puzzle" : s.question && s.question.kind === "song" ? "Song" : "Question";
  return `${kind} ${s.round} of ${s.total_rounds}`;
}

// ---------- Trivia rounds ----------

const DIFFICULTY = { easy: "Easy", medium: "Medium", hard: "Hard" };
const stageOf = (s) => (s.stage ? s.stages[s.stage - 1] : null);
const stageName = (st) => (st.final_round ? "Final round" : `Round ${st.number}`);
const mult = (pct) => `×${pct / 100}`;
const diffClass = (st) => `diff-${st.final_round ? "final" : st.difficulty}`;

/** The rounds as a staircase, easiest first, with the current one lifted. */
function renderLadder(s, box) {
  box.replaceChildren(...s.stages.map((st, i) =>
    el("div", { class: `rung ${st.status} ${diffClass(st)}`, style: `--i:${i}` },
      el("div", { class: "rung-title" }, st.final_round ? "Final" : `Round ${st.number}`),
      el("div", { class: "rung-cat" }, st.category || "???"),
      el("div", { class: "rung-diff" }, `${st.final_round ? "Hardest" : DIFFICULTY[st.difficulty]} · ${mult(st.points_pct)}`))));
}

function renderRoundIntro(s, changed) {
  const st = stageOf(s);
  const screen = $("#round_intro");
  screen.classList.remove("diff-easy", "diff-medium", "diff-hard", "diff-final");
  screen.classList.add(diffClass(st));
  $("#ri-count").textContent = `Round ${st.number} of ${s.stages.length}`;
  $("#ri-code").textContent = s.code;
  $("#ri-title").textContent = st.final_round ? "Final round" : `Round ${st.number}`;
  $("#ri-cat").textContent = st.category;
  $("#ri-diff").textContent = st.final_round ? "Hardest" : DIFFICULTY[st.difficulty];
  $("#ri-mult").textContent = `${mult(st.points_pct)} points`;
  $("#ri-qs").textContent = `${st.questions} question${st.questions === 1 ? "" : "s"}`;
  renderLadder(s, $("#ri-ladder"));
  // The category's generated title card, if art/generate.py has made one, behind the title.
  const art = Cutscenes.pickArt({ kind: "category" }, { rating: s.settings.max_rating, vars: { category: st.category } });
  screen.classList.toggle("has-art", !!art);
  if (art) screen.style.setProperty("--cat-art", `url("${art}")`);
  if (changed) enter(screen);
}

function renderRecap(s, changed) {
  const st = stageOf(s);
  const r = s.recap;
  const byId = Object.fromEntries(s.players.map((p) => [p.id, p]));
  $("#rc-round").textContent = `Round ${st.number} of ${s.stages.length}`;
  $("#rc-code").textContent = s.code;
  $("#rc-title").textContent = `${stageName(st)}: ${st.category}`;
  $("#rc-teams").replaceChildren(...teamStrip(s));
  fillBoard($("#rc-board"), s, Object.fromEntries(r.points.map((p) => [p.player_id, p.points])), changed);
  $("#rc-qs").replaceChildren(...r.questions.map((q, i) => el("div", { class: "rq", style: `--i:${i}` },
    el("span", { class: "rq-n" }, i + 1),
    el("div", { class: "rq-text" }, el("div", { class: "rq-prompt" }, q.prompt), el("div", { class: "rq-answer" }, q.answer)),
    el("div", { class: "rq-who" }, q.right.length
      ? q.right.map((id) => byId[id] && TBX.avatar(byId[id], "3.6vh"))
      : el("span", { class: "muted" }, "Nobody!")))));
  renderLadder(s, $("#rc-ladder"));
  if (changed) enter($("#recap"));
}

// ---------- Mini games between rounds ----------

/** Art tags for each mini game (see the art catalog). */
const MINI_TAGS = {
  mash: "button_mash", closest: "closest_guess", quick_draw: "quick_draw",
  stop_clock: "stop_the_clock", majority: "majority_rules", lowball: "lowball",
};
/** Games where everyone locks in an answer, kept secret until the results. */
const MINI_PICKS = ["closest", "majority", "lowball"];
let miniClock = null;

/** The value a mini game ranks by, as the TV shows it. */
function miniValue(m, e) {
  if (e.false_start) return "False start!";
  const picks = MINI_PICKS.indexOf(m.kind) >= 0;
  if (e.value == null && m.step === "results") return picks ? "No pick" : "No tap";
  if (e.value == null) return e.done ? (picks ? "Locked in" : "Stopped") : "";
  if (m.kind === "mash") return `${e.value} tap${e.value === 1 ? "" : "s"}`;
  if (m.kind === "stop_clock") {
    const off = Math.abs(e.value - m.target_ms) / 1000;
    return `${(e.value / 1000).toFixed(2)}s${off ? ` (off by ${off.toFixed(2)})` : " · perfect!"}`;
  }
  if (m.kind === "majority") return m.options[e.value] || "";
  if (m.kind === "lowball") {
    const clash = m.entries.filter((x) => x.value === e.value).length > 1;
    return `${e.value}${clash ? " · clash!" : ""}`;
  }
  if (m.kind === "quick_draw") return `${(e.value / 1000).toFixed(3)}s`;
  const off = m.answer != null ? Math.abs(e.value - m.answer) : null;
  return `${e.value.toLocaleString()}${off != null ? (off ? ` (off by ${off.toLocaleString()})` : " · spot on!") : ""}`;
}

/** Up to `n` different stickers with all of `tags`, from the art pack (fewer, or none, if not made). */
function miniStickers(tags, rating, n) {
  const got = [];
  for (let i = 0; i < n * 4 && got.length < n; i++) {
    const url = Cutscenes.pickArt({ kind: "sticker", tags: tags }, { rating: rating });
    if (!url) break;
    if (got.indexOf(url) < 0) got.push(url);
  }
  return got;
}

/** Shows a sticker in `img`, or hides it when there's none. */
function showSticker(img, url) {
  img.classList.toggle("hidden", !url);
  if (url) img.src = url;
}

function renderMinigame(s, changed) {
  const m = s.minigame;
  const was = prev && prev.minigame;
  const stepChanged = changed || !was || was.step !== m.step;
  const screen = $("#minigame");
  const st = stageOf(s);
  $("#mg-round").textContent = st ? `After ${stageName(st).toLowerCase()} · bonus points` : "Bonus points";
  $("#mg-code").textContent = s.code;
  $("#mg-title").textContent = m.title;
  $("#mg-rules").textContent = m.step === "results" ? "Results" : m.rules;
  if (changed) {
    const art = Cutscenes.pickArt({ kind: "backdrop", tags: [MINI_TAGS[m.kind]] }, { rating: s.settings.max_rating });
    screen.classList.toggle("has-art", !!art);
    if (art) screen.style.setProperty("--cat-art", `url("${art}")`);
    const pair = miniStickers([MINI_TAGS[m.kind]], s.settings.max_rating, 2);
    showSticker($("#mg-st-a"), pair[0]);
    showSticker($("#mg-st-b"), pair[1]);
  }
  // The results swap one side for the winner's prize.
  if (stepChanged && m.step === "results") {
    const prize = miniStickers(["minigame_winner"], s.settings.max_rating, 1)[0];
    if (prize) showSticker($("#mg-st-b"), prize);
  }

  const big = $("#mg-big");
  big.className = "mg-big";
  const clockRunning = m.kind === "stop_clock" && m.step === "play";
  if (!clockRunning && miniClock) { clearInterval(miniClock); miniClock = null; }
  if ((m.kind === "closest" || m.kind === "majority") && m.step !== "rules") {
    big.classList.add("prompt");
    big.textContent = m.prompt;
  } else big.textContent = "";
  if (m.step === "rules") {
    big.textContent = {
      mash: "Get your thumbs ready…", closest: "Phones out…", quick_draw: "Fingers off the screen…",
      stop_clock: "Count in your head…", majority: "Think like the crowd…", lowball: "Go low. Not too low…",
    }[m.kind];
  } else if (clockRunning) {
    // The stopwatch runs for 3 seconds on the TV, then players are on their own.
    big.classList.add("clock");
    if (stepChanged || !miniClock) {
      const started = Date.now();
      clearInterval(miniClock);
      const tick = () => {
        const t = Date.now() - started;
        big.textContent = t < 3000 ? `${(t / 1000).toFixed(1)}s` : "?.?s";
        big.classList.toggle("hidden-clock", t >= 3000);
      };
      tick();
      miniClock = setInterval(tick, 50);
    }
  } else if (m.kind === "lowball" && m.step === "play") {
    big.textContent = `Pick 1 to ${m.max_pick}`;
  } else if (m.step === "results" && m.kind === "stop_clock") {
    big.classList.add("answer");
    big.textContent = `Target: ${(m.target_ms / 1000).toFixed(2)}s`;
  } else if (m.step === "results" && m.kind === "majority") {
    big.classList.remove("prompt");
    big.classList.add("answer");
    big.textContent = m.options.map((o, i) => `${o} ${m.entries.filter((e) => e.value === i).length}`).join(" · ");
  } else if (m.step === "results" && m.kind === "lowball") {
    big.classList.add("answer");
    const win = m.entries[0];
    big.textContent = win && win.points ? `Lowest lonely number: ${win.value}` : "Everybody clashed!";
  }
  else if (m.kind === "quick_draw" && m.step === "ready") {
    big.classList.add("wait");
    big.textContent = "WAIT…";
  } else if (m.kind === "quick_draw" && m.step === "play") {
    big.classList.add("go");
    big.textContent = "GO!";
  } else if (m.kind === "mash" && m.step === "play") {
    big.textContent = "TAP TAP TAP!";
  } else if (m.step === "results" && m.kind === "closest") {
    big.classList.remove("prompt");
    big.classList.add("answer");
    big.textContent = `${m.answer.toLocaleString()}${m.unit ? " " + m.unit : ""}`;
  } else if (m.step === "results") {
    big.textContent = "";
  }
  if (stepChanged) {
    if (m.step === "play" && (m.kind === "quick_draw" || m.kind === "stop_clock")) Sfx.play("reveal");
    else if (m.step === "play") Sfx.play("round");
    else if (m.step === "results") Sfx.play("scores");
    // No countdown before GO; that would give it away.
    TBX.runTimer($("#mg-timer"), m.step === "ready" ? null : s.ms_left, s.phase_ms);
    // (The server sends no clock in Quick Draw's wait or in Stop the Clock, so there's none to show.)
    if (m.step === "play" && s.ms_left) startSecs($("#mg-secs"), s.ms_left);
    else { clearInterval(secsTimer); $("#mg-secs").textContent = ""; }
    if (m.step === "play" && m.kind === "mash") Sfx.bed.play(s.ms_left);
    else Sfx.bed.stop();
  }

  const byId = Object.fromEntries(s.players.map((p) => [p.id, p]));
  const most = Math.max(1, ...m.entries.map((e) => (m.kind === "mash" && e.value) || 0));
  const rows = m.entries.filter((e) => byId[e.player_id]);
  $("#mg-rows").style.setProperty("--cols", rows.length > 6 ? 2 : 1);
  $("#mg-rows").replaceChildren(...rows.map((e, i) => {
    const p = byId[e.player_id];
    const results = m.step === "results";
    return el("div", { class: "mg-row" + (stepChanged ? " fresh" : "") + (e.false_start ? " out" : "") + (results && i === 0 && e.points > 0 ? " first" : ""), style: `--i:${i}` },
      m.kind === "mash" ? el("div", { class: "mg-bar", style: `width:${((e.value || 0) / most) * 100}%;background:${p.color}55` }) : null,
      TBX.avatar(p, "1.4em"),
      el("span", { class: "mg-name" }, p.name),
      el("span", { class: "mg-val" }, m.step === "rules" ? "" : miniValue(m, e)),
      results ? el("span", { class: "mg-pts" }, e.points ? `+${e.points}` : "") : null);
  }));
  if (stepChanged) enter(screen);
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
    enter($("#question"));
    if (song) playClip(s);
  }
  const late = s.players.length - playing(s).length;
  $("#q-answered").replaceChildren(...playing(s).map((p) => {
    const a = TBX.avatar(p, "6vh");
    if (!p.answered) a.classList.add("waiting");
    a.title = p.name;
    return a;
  }), ...(late ? [el("span", { class: "muted" }, `+${late} joining next question`)] : []));
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
    $("#r-credit").textContent = song.from_library
      ? `From your music library${song.album ? ` · ${song.album}` : ""}`
      : "Song preview courtesy of Apple Music";
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
    } else {
      clearInterval(secsTimer);
      $("#h-secs").textContent = "";
      $("#h-secs").classList.remove("danger");
    }
  }
}

function sortedPlayers(s) { return playing(s).sort((a, b) => b.score - a.score); }

function teamStrip(s) {
  return s.teams.map((t, i) => el("div", { class: "team-pill pop", style: `background:${t.color};animation-delay:${i * 80}ms` },
    el("span", {}, `${i + 1}. ${t.name}`), el("b", {}, t.score.toLocaleString())));
}

function renderScores(s, changed) {
  $("#s-round").textContent = roundLabel(s);
  $("#s-code").textContent = s.code;
  const results = s.reveal ? s.reveal.results : s.hangman ? s.hangman.results : [];
  $("#s-teams").replaceChildren(...teamStrip(s));
  fillBoard($("#s-board"), s, Object.fromEntries(results.map((r) => [r.player_id, r.points])), changed);
}

/** Ranked scores with what each player just gained; rows slide up or down from where they were. */
function fillBoard(board, s, gains, changed) {
  let rank = 0, last = null;
  const n = playing(s).length;
  board.style.setProperty("--rows", n > 8 ? Math.ceil(n / 2) : Math.max(n, 1));
  // Where everyone stood before these points, so rows can slide from old place to new.
  const before = playing(s).sort((a, b) => (b.score - (gains[b.id] || 0)) - (a.score - (gains[a.id] || 0))).map((p) => p.id);
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
  const topTeam = s.teams[0];
  const teamTie = s.teams.length > 1 && s.teams[1].score === topTeam.score;
  $("#f-title").textContent = topTeam && !teamTie ? `${topTeam.name} wins!` : top[0] ? `${top[0].name} wins!` : "Game over";
  $("#f-teams").replaceChildren(...teamStrip(s));
  $("#f-podium").replaceChildren(...order.map((i) => el("div", { class: "step pop" },
    TBX.avatar(top[i], "10vh"),
    el("div", {}, top[i].name),
    el("div", { class: "muted" }, top[i].score.toLocaleString()),
    i === 0 ? el("div", { class: "crown" }, "👑") : null,
    el("div", { class: "block", style: `height:${heights[i]};background:${top[i].color}55` }, medal[i]))));
  $("#f-rest").textContent = ranked.slice(3).map((p, i) => `#${i + 4} ${p.name} (${p.score.toLocaleString()})`).join("   ·   ");
  if (changed && top[0]) confetti();
}

// ---------- Controls ----------
// The game is run from the VIP's phone. These are optional extras for a keyboard or remote.

function primaryAction() {
  if (!state) return;
  if (state.phase === "lobby") send({ type: "start" });
  else if (state.phase === "final") send({ type: "play_again" });
  else send({ type: "next" });
}

$("#sound-btn").addEventListener("click", () => tune.unlock());
// A mouse moving over the TV page brings the extras back for a few seconds.
let pointerTimer = null;
document.addEventListener("mousemove", () => {
  document.body.classList.add("pointer");
  clearTimeout(pointerTimer);
  pointerTimer = setTimeout(() => document.body.classList.remove("pointer"), 4000);
});
document.addEventListener("pointerdown", () => { Sfx.unlock(); Bgm.unlock(); tune.unlock(); soundHint(); });

// Browsers keep a page silent until someone clicks it once, so say so until that happens.
const IN_APP = / TriviaBixCast\//.test(navigator.userAgent);
function soundHint() {
  const active = IN_APP || !navigator.userActivation || navigator.userActivation.hasBeenActive;
  $("#sound-hint").classList.toggle("hidden", active);
}

function toggleVoice() {
  voice.on = !voice.on;
  Cutscenes.sound = voice.on;
  if (!voice.on) voice.hush();
  if (state) hostDisplay(state);
  TBX.save("tbx:voice", voice.on);
  $("#mute-btn").textContent = voice.on ? "🔊 Host voice" : "🔇 Host voice";
}
$("#mute-btn").addEventListener("click", toggleVoice);
$("#mute-btn").textContent = voice.on ? "🔊 Host voice" : "🔇 Host voice";

function nextVoice() {
  const name = voice.next();
  toast(`Host voice: ${name}`);
  voice.hush();
  // Replay what the host just said in the new voice.
  if (state && state.host) {
    const h = state.host;
    voice.speak(h.voice && h.voice.length ? h.voice : [{ text: h.text }]);
  } else {
    voice.say("Hi, I'm your host. How do I sound?");
  }
}
$("#voice-btn").addEventListener("click", nextVoice);

function toggleFullscreen() {
  if (document.fullscreenElement) document.exitFullscreen();
  else document.documentElement.requestFullscreen && document.documentElement.requestFullscreen().catch(() => {});
}
$("#fs-btn").addEventListener("click", toggleFullscreen);

// TV sticks and phones mirrored to a TV dim or sleep when nobody touches them; ask them not to.
// Browsers only allow this over HTTPS or on localhost, so on a plain LAN address see docs/tv.md.
let wakeLock = null;
async function keepAwake() {
  try {
    if ("wakeLock" in navigator && !wakeLock && !document.hidden) {
      wakeLock = await navigator.wakeLock.request("screen");
      wakeLock.addEventListener("release", () => { wakeLock = null; });
    }
  } catch (_) { /* not supported, or needs a press first; tried again on the next one */ }
}
document.addEventListener("visibilitychange", keepAwake);
document.addEventListener("pointerdown", keepAwake);
document.addEventListener("keydown", keepAwake);
keepAwake();

document.addEventListener("keydown", (e) => {
  if (e.target.tagName === "INPUT") return;
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    Sfx.unlock();
    Bgm.unlock();
    // If the browser blocked the clip, this press turns sound on instead of skipping.
    if (tune.unlock()) return;
    // During a cutscene, the first press just ends the scene.
    if (Cutscenes.playing) return Cutscenes.stop();
    primaryAction();
  } else if ((e.key === "r" || e.key === "R") && state && state.phase === "question") {
    tune.replay();
  } else if (e.key === "m" || e.key === "M") toggleVoice();
  else if (e.key === "b" || e.key === "B") toggleMusic();
  else if (e.key === "f" || e.key === "F") toggleFullscreen();
  else if (e.key === "v" || e.key === "V") nextVoice();
  soundHint();
});

// ---------- Boot ----------

(async function boot() {
  // The cast app lets the TV page play sound with nobody pressing anything.
  if (IN_APP) { Sfx.unlock(); Bgm.unlock(); }
  soundHint();
  Cutscenes.sound = voice.on;
  await Promise.all([TBX.loadCharacters(), Cutscenes.load()]);
  try {
    const cfg = await (await fetch("/api/config")).json();
    joinBase = TBX.joinBase(cfg.public_url);
    voice.ming = !!cfg.voice;
    voice.speed = cfg.voice_speed || 1;
  } catch (_) { /* fall back to this page's address */ }

  // Opened from the admin panel with a room's link: take over that room.
  const params = new URLSearchParams(location.search);
  if (params.get("code") && params.get("token")) {
    TBX.save(HOST_KEY, { code: params.get("code").toUpperCase(), token: params.get("token") });
    history.replaceState(null, "", location.pathname);
  }

  // Coming back after a refresh: rejoin the same room if it's still alive.
  const saved = TBX.load(HOST_KEY);
  if (saved) {
    const res = await fetch(`/api/rooms/${saved.code}`).catch(() => null);
    if (res && res.ok) return join(saved.code, saved.token);
    TBX.save(HOST_KEY, null);
  }
  createRoom();
})();
