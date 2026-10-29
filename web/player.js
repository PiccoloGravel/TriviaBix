// The phone controller.

const { $, el, show } = TBX;
const PLAYER_KEY = "tbx:player";

let conn = null;
let state = null;
let me = null; // { code, name, token, id }
let picked = TBX.load("tbx:character"); // character id chosen on the join screen

function joinGame(code, name, token) {
  conn && conn.close();
  me = { code, name, token, id: null };
  $("#join-error").textContent = "";
  conn = TBX.connect("/ws/play", () => {
    const p = { code, name };
    if (picked) p.character = picked;
    if (me.token) p.token = me.token;
    return p;
  }, {
    onMessage(msg) {
      if (msg.type === "welcome") {
        me.token = msg.token;
        me.id = msg.player_id;
        TBX.save(PLAYER_KEY, { code, name, token: msg.token });
        TBX.keepAwake();
      } else if (msg.type === "state") {
        render(msg);
      } else if (msg.type === "error") {
        flash(msg.message);
      }
    },
    onFatal(message) {
      // A stale token (room closed, or kicked) or a rejected join.
      TBX.save(PLAYER_KEY, null);
      state = null;
      show("join");
      refreshJoinChars();
      $("#join-error").textContent = message;
    },
    onStatus(ok) { $("#offline").classList.toggle("hidden", ok); },
  });
}

function send(msg) { conn && conn.send(msg); }

function flash(text) {
  const target = !state ? $("#join-error") : state.phase === "minigame" ? $("#mg-status") : state.phase === "question" ? (state.hangman ? $("#h-status") : $("#q-status")) : $("#lobby-error");
  target.textContent = text;
}

// Phones love to sleep mid-question; ask them not to while we're playing.
// ---------- Rendering ----------

function render(s) {
  const prev = state;
  state = s;
  const you = s.you;
  const player = s.players.find((p) => p.id === you.player_id);
  if (!player) return;

  const team = s.teams.find((t) => t.id === you.team);
  document.querySelectorAll("[data-me]").forEach((box) => {
    box.replaceChildren(...[
      TBX.avatar(player, "40px"),
      el("span", { class: "name" }, player.name, you.is_vip ? " 👑" : ""),
      team ? el("span", { class: "team-chip", style: `background:${team.color}` }, team.name) : null,
      el("span", { class: "score" }, player.score.toLocaleString()),
    ].filter(Boolean));
  });
  document.querySelectorAll("[data-vip-next], [data-vip-end]").forEach((b) => b.classList.toggle("hidden", !you.is_vip));

  // Joined mid-game: sit tight until the next question (or the next game).
  if (you.waiting) {
    show("waiting");
    $("#w-text").textContent = s.phase === "final"
      ? "This game just ended. You're in for the next one!"
      : "You'll jump in at the next question. Watch the TV!";
    return;
  }
  const fresh = !prev || prev.phase !== s.phase || prev.round !== s.round;
  if (s.hangman && s.phase === "question") {
    show("hangman");
    return renderHangman(s, fresh);
  }
  show(s.phase);

  if (s.phase === "lobby") {
    $("#lobby-count").textContent = `${s.players.length} of ${s.max_players} players · room ${s.code}`;
    $("#vip-box").classList.toggle("hidden", !you.is_vip);
    $("#lobby-wait").classList.toggle("hidden", you.is_vip);
    const vip = s.players.find((p) => p.id === s.vip);
    $("#lobby-wait").textContent = `Up next: ${gameSummary(s.settings)}. ${vip ? vip.name : "The VIP"} picks the game and starts it.`;
    $("#admin-link").href = `/admin?room=${s.code}`;
    if (you.is_vip) renderPicker(s);
    $("#lobby-error").textContent = "";
    const taken = s.players.map((p) => p.character);
    TBX.characterGrid($("#lobby-chars"), player.character, taken, (id) => {
      picked = id;
      TBX.save("tbx:character", id);
      send({ type: "profile", character: id });
    });
    $("#rename-input").placeholder = player.name;
    const byId = [...s.teams].sort((a, b) => a.id - b.id);
    $("#team-box").classList.toggle("hidden", !byId.length);
    $("#team-buttons").replaceChildren(...byId.map((t) => el("button", {
      class: t.id === you.team ? "picked" : "secondary",
      style: `--team:${t.color}`,
      onclick: () => send({ type: "team", team: t.id }),
    }, t.name, el("small", {}, ` (${t.members.length})`))));
  } else if (s.phase === "question") {
    renderQuestion(s, fresh);
  } else if (s.phase === "reveal" && s.hangman) {
    renderHangmanReveal(s, fresh);
  } else if (s.phase === "reveal") {
    const result = s.reveal.results.find((r) => r.player_id === you.player_id) || {};
    const right = result.points > 0;
    $("#r-verdict").textContent = right ? "✅" : result.choice == null ? "⏰" : "❌";
    $("#r-text").textContent = right ? "Correct!" : result.choice == null ? "Too slow!" : "Nope!";
    $("#r-points").textContent = right ? `+${result.points}` : "";
    $("#r-answer").textContent = `Answer: ${s.question.choices[s.reveal.correct]}`;
    const song = s.reveal.song;
    $("#r-link").classList.toggle("hidden", !(song && song.store_url));
    if (song && song.store_url) $("#r-link").href = song.store_url;
    // If the host called you out on the TV, you get the badge too.
    const mine = s.host && s.host.callouts.find((c) => c.player_id === you.player_id);
    $("#r-badge").classList.toggle("hidden", !mine);
    if (mine) {
      $("#r-badge").textContent = mine.label;
      $("#r-badge").classList.toggle("good", !["roast", "asleep", "streak_broken", "last_place"].includes(mine.kind));
    }
    if (fresh && navigator.vibrate) navigator.vibrate(right ? 80 : [60, 60, 60]);
  } else if (s.phase === "round_intro") {
    const st = s.stages[s.stage - 1];
    $("#ri-title").textContent = st.final_round ? "Final round" : `Round ${st.number} of ${s.stages.length}`;
    $("#ri-cat").textContent = st.category;
    $("#ri-points").textContent = `×${st.points_pct / 100} points`;
    $("#ri-diff").textContent = `${st.final_round ? "The hardest questions" : st.difficulty[0].toUpperCase() + st.difficulty.slice(1) + " questions"} · eyes on the TV!`;
  } else if (s.phase === "recap") {
    const st = s.stages[s.stage - 1];
    const mine = s.recap.points.find((p) => p.player_id === you.player_id);
    const right = s.recap.questions.filter((q) => q.right.includes(you.player_id)).length;
    $("#rc-title").textContent = `${st.final_round ? "Final round" : "Round " + st.number} done`;
    $("#rc-points").textContent = `+${(mine ? mine.points : 0).toLocaleString()}`;
    $("#rc-right").textContent = `${right} of ${s.recap.questions.length} right`;
    $("#rc-rank").textContent = `You're in ${ordinal(you.rank)} place`;
  } else if (s.phase === "minigame") {
    renderMinigame(s, prev);
  } else if (s.phase === "scoreboard") {
    $("#s-rank").textContent = ordinal(you.rank) + " place";
  } else if (s.phase === "final") {
    $("#f-emoji").textContent = you.rank === 1 ? "🏆" : you.rank <= 3 ? "🎉" : "🫠";
    $("#f-text").textContent = you.rank === 1 ? "You won!" : `You came ${ordinal(you.rank)}`;
    $("#f-score").textContent = `${player.score.toLocaleString()} pts`;
    $("#again-btn").classList.toggle("hidden", !you.is_vip);
    $("#f-wait").classList.toggle("hidden", you.is_vip);
  }
}

// ---------- VIP: pick the game ----------

function gameSummary(st) {
  const rated = `rated ${st.max_rating}`;
  const teams = st.teams ? ` · ${st.teams} teams` : "";
  if (st.mode === "songs") return `Name That Tune, ${st.rounds} songs, ${st.clip_ms / 1000}-second clips, ${rated}${teams}`;
  if (st.mode === "hangman") return `Speed Hangman, ${st.rounds} puzzles, ${rated}${teams}`;
  return `Trivia, ${st.trivia_rounds} rounds of ${st.per_round}${st.trivia_rounds > 3 ? " plus a final" : ""}${st.minigames ? " with mini games between" : ""}, ${rated}${teams}`;
}

function renderPicker(s) {
  const st = s.settings;
  document.querySelectorAll("#vip-box [data-opt]").forEach((b) => {
    b.classList.toggle("picked", String(st[b.dataset.opt]) === b.dataset.val);
  });
  $("#pick-summary").textContent = `${gameSummary(st)}. The admin sets the rating, length and teams.`;
  // Phones that dropped out can be cleared from the lobby, now that the TV has no controls.
  const others = s.players.filter((p) => p.id !== s.you.player_id);
  $("#kick-box").classList.toggle("hidden", !others.length);
  $("#kick-list").replaceChildren(...others.map((p) => el("button", {
    class: "secondary",
    onclick: () => { if (confirm(`Remove ${p.name} from the game?`)) send({ type: "kick", player_id: p.id }); },
  }, `${TBX.characters[p.character] ? TBX.characters[p.character].emoji + " " : ""}${p.name}${p.connected ? "" : " (away)"} ✕`)));
}

document.querySelectorAll("#vip-box [data-opt]").forEach((b) => b.addEventListener("click", () => {
  const { opt, val } = b.dataset;
  send({ type: "settings", [opt]: val });
}));

function renderQuestion(s, fresh) {
  const picked = s.you.answer;
  const song = s.question.kind === "song";
  $("#q-prompt").textContent = song ? "🎧 Listen to the TV! Name that tune." : s.question.prompt;
  if (fresh) {
    TBX.runTimer($("#q-timer"), s.ms_left, s.phase_ms);
    $("#q-status").textContent = "";
    $("#answers").replaceChildren(...s.question.choices.map((text, i) =>
      el("button", { class: `choice-${i}`, onclick: () => answer(i) },
        el("span", { class: "shape" }, TBX.SHAPES[i]), song ? songLabel(text) : el("span", {}, text))));
  }
  const buttons = [...$("#answers").children];
  buttons.forEach((b, i) => { b.disabled = picked != null; b.classList.toggle("picked", picked === i); });
  $("#answers").classList.toggle("locked", picked != null);
  if (picked != null) {
    const waiting = s.players.filter((p) => p.connected && !p.answered).length;
    $("#q-status").textContent = waiting ? `Locked in! Waiting on ${waiting} more…` : "Locked in!";
  }
}

/** "Title — Artist" as a bold title over a smaller artist. */
function songLabel(text) {
  const [title, artist] = text.split(" — ");
  return el("span", { class: "song" }, el("b", {}, title), artist ? el("small", {}, artist) : null);
}

function answer(i) {
  if (!state || state.phase !== "question" || state.you.answer != null) return;
  send({ type: "answer", choice: i });
  if (navigator.vibrate) navigator.vibrate(30);
}

// ---------- Mini games between rounds ----------

let lastTap = 0;
let localTaps = 0; // shown right away; the server's count catches up a moment later

function renderMinigame(s, prev) {
  const m = s.minigame;
  const was = prev && prev.phase === "minigame" && prev.minigame;
  const stepChanged = !was || was.step !== m.step || was.kind !== m.kind;
  const mine = m.entries.find((e) => e.player_id === s.you.player_id) || {};
  const playing = m.step === "play" || m.step === "ready";
  $("#mg-title").textContent = m.title;
  $("#mg-rules").textContent = m.rules;
  if (stepChanged) {
    TBX.runTimer($("#mg-timer"), m.step === "ready" ? null : s.ms_left, s.phase_ms);
    $("#mg-status").textContent = "";
    localTaps = 0;
    if (m.step === "play" && navigator.vibrate) navigator.vibrate(m.kind === "quick_draw" ? 120 : 40);
  }
  // The VIP can skip the rules and results, but not end a game by mistake mid-tap.
  document.querySelectorAll("#mg-vip [data-vip-next], #mg-vip [data-vip-end]").forEach((b) => b.classList.toggle("hidden", !s.you.is_vip || playing));

  const pad = $("#mg-pad");
  const tapper = ["mash", "quick_draw", "stop_clock"].includes(m.kind) && m.step !== "results";
  pad.classList.toggle("hidden", !tapper);
  pad.classList.remove("wait", "go");
  if (tapper) {
    if (m.step === "rules") {
      pad.disabled = true;
      pad.textContent = m.kind === "mash" ? "Get ready…" : "Hands off…";
    } else if (m.kind === "stop_clock") {
      pad.disabled = mine.done;
      pad.classList.toggle("go", !mine.done);
      pad.textContent = mine.done ? "Stopped ✓" : "STOP!";
    } else if (m.kind === "mash") {
      pad.disabled = false;
      pad.textContent = "TAP!";
    } else if (mine.false_start) {
      pad.disabled = true;
      pad.textContent = "False start 😬";
    } else if (m.step === "ready") {
      pad.disabled = false;
      pad.classList.add("wait");
      pad.textContent = "WAIT…";
    } else if (mine.value != null) {
      pad.disabled = true;
      pad.textContent = `${(mine.value / 1000).toFixed(3)}s`;
    } else {
      pad.disabled = false;
      pad.classList.add("go");
      pad.textContent = "GO!";
    }
  }
  $("#mg-count").textContent = m.kind === "mash" && m.step === "play" ? `${Math.max(localTaps, mine.value || 0)} taps` : "";

  const guessing = m.kind === "closest" && m.step === "play";
  $("#mg-prompt").classList.toggle("hidden", (m.kind !== "closest" && m.kind !== "majority") || m.step === "rules");
  $("#mg-prompt").textContent = m.prompt || "";
  $("#mg-guess-form").classList.toggle("hidden", !guessing || mine.done);
  if (guessing && stepChanged) $("#mg-guess-input").value = "";
  if (guessing && mine.done) $("#mg-count").textContent = `Locked in: ${mine.value.toLocaleString()}${m.unit ? " " + m.unit : ""}`;

  // Majority Rules and Lowball: tap one button to lock in.
  const picking = (m.kind === "majority" || m.kind === "lowball") && m.step === "play";
  const picks = $("#mg-picks");
  picks.classList.toggle("hidden", !picking);
  if (picking) {
    const labels = m.kind === "majority" ? m.options : Array.from({ length: m.max_pick }, (_, i) => String(i + 1));
    const values = m.kind === "majority" ? labels.map((_, i) => i) : labels.map((_, i) => i + 1);
    if (stepChanged || picks.children.length !== labels.length) {
      picks.className = `mg-picks ${m.kind === "majority" ? "options" : "numbers"}`;
      picks.replaceChildren(...labels.map((label, i) => el("button", {
        type: "button",
        class: m.kind === "majority" ? `choice-${i}` : "secondary",
        onclick: () => send({ type: "mini_guess", value: values[i] }),
      }, label)));
    }
    [...picks.children].forEach((b, i) => {
      b.disabled = mine.done;
      b.classList.toggle("picked", mine.value === values[i]);
    });
    picks.classList.toggle("locked", !!mine.done);
    if (mine.done) $("#mg-count").textContent = "Locked in!";
  }

  const results = m.step === "results";
  $("#mg-result").classList.toggle("hidden", !results);
  if (results) {
    const place = m.entries.findIndex((e) => e.player_id === s.you.player_id);
    const won = mine.points > 0 && mine.points === m.entries[0].points;
    $("#mg-result-text").textContent = won ? "You won it! 🏆" : mine.points > 0 ? `${ordinal(place + 1)} place` : mine.false_start ? "Jumped the gun!" : "No bonus this time";
    $("#mg-result-points").textContent = mine.points ? `+${mine.points}` : "";
    $("#mg-result-more").textContent = m.kind === "closest"
      ? `Answer: ${m.answer.toLocaleString()}${m.unit ? " " + m.unit : ""}${mine.value != null ? ` · you said ${mine.value.toLocaleString()}` : ""}`
      : m.kind === "mash" ? `${mine.value || 0} taps`
      : m.kind === "majority" ? (mine.value != null ? `You picked ${m.options[mine.value]}` : "You didn't pick")
      : m.kind === "lowball" ? (mine.value != null ? `You picked ${mine.value}${m.entries.filter((e) => e.value === mine.value).length > 1 ? ", and so did someone else!" : ""}` : "You didn't pick")
      : m.kind === "stop_clock" ? (mine.value != null ? `You stopped at ${(mine.value / 1000).toFixed(2)}s` : "You never stopped it")
      : mine.value != null ? `${(mine.value / 1000).toFixed(3)} seconds` : "";
    if (stepChanged && navigator.vibrate) navigator.vibrate(mine.points ? [80, 40, 80] : 60);
  }
}

// Pointer down, not click, so fast tapping isn't slowed by the browser's tap handling.
$("#mg-pad").addEventListener("pointerdown", (e) => {
  e.preventDefault();
  const pad = e.currentTarget;
  if (!state || state.phase !== "minigame" || pad.disabled) return;
  const now = Date.now();
  if (now - lastTap < 50) return;
  lastTap = now;
  send({ type: "mini_tap" });
  pad.classList.add("hit");
  setTimeout(() => pad.classList.remove("hit"), 60);
  if (state.minigame.kind === "mash") {
    localTaps++;
    $("#mg-count").textContent = `${localTaps} taps`;
  } else if (navigator.vibrate) navigator.vibrate(30);
});

$("#mg-guess-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const value = Math.round(Number($("#mg-guess-input").value));
  if (!state || state.phase !== "minigame" || $("#mg-guess-input").value === "" || !Number.isFinite(value)) return;
  send({ type: "mini_guess", value });
  $("#mg-guess-input").blur();
});

// ---------- Speed hangman ----------

const KEY_ROWS = ["QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"];
let coolUntil = 0;

function renderHangman(s, fresh) {
  const h = s.hangman;
  const mine = s.you.hangman || { solve_locked: false, cooldown_ms: 0 };
  $("#h-cat").textContent = h.category;
  $("#h-board").textContent = h.board;
  $("#h-strikes").textContent = "💀".repeat(h.strikes) + "⬜".repeat(Math.max(0, h.max_strikes - h.strikes));
  if (fresh) {
    TBX.runTimer($("#h-timer"), s.ms_left, s.phase_ms);
    $("#h-status").textContent = "";
    $("#solve-input").value = "";
    coolUntil = 0;
    $("#h-keys").replaceChildren(...KEY_ROWS.map((row) => el("div", { class: "krow" },
      ...row.split("").map((c) => el("button", { type: "button", "data-key": c, onclick: () => guess(c) }, c)))));
  }
  for (const b of $("#h-keys").querySelectorAll("button")) {
    const c = b.dataset.key;
    const hit = h.hits.includes(c), miss = h.misses.includes(c);
    b.classList.toggle("hit", hit);
    b.classList.toggle("miss", miss);
    b.disabled = hit || miss;
  }
  // Trust the server's cooldown too, e.g. after a refresh.
  if (mine.cooldown_ms > 0) coolDown(mine.cooldown_ms);
  $("#solve-input").disabled = mine.solve_locked;
  $("#solve-form button").disabled = mine.solve_locked;
  $("#solve-input").placeholder = mine.solve_locked ? "You blew your solve 😬" : "Know it? Solve it";
}

function coolDown(ms) {
  coolUntil = Math.max(coolUntil, Date.now() + ms);
  $("#h-keys").classList.add("cooling");
  clearTimeout(coolDown.timer);
  coolDown.timer = setTimeout(() => $("#h-keys").classList.remove("cooling"), coolUntil - Date.now());
}

function guess(c) {
  if (!state || !state.hangman || state.phase !== "question" || Date.now() < coolUntil) return;
  send({ type: "guess", letter: c });
  coolDown(1000);
  if (navigator.vibrate) navigator.vibrate(20);
}

$("#solve-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const text = $("#solve-input").value.trim();
  if (!text || !state || !state.hangman || state.phase !== "question") return;
  send({ type: "solve", text });
  $("#solve-input").value = "";
  $("#solve-input").blur();
});

function renderHangmanReveal(s, fresh) {
  const h = s.hangman;
  const result = h.results.find((r) => r.player_id === s.you.player_id) || { points: 0 };
  const solvedIt = h.solved_by === s.you.player_id;
  const solver = s.players.find((p) => p.id === h.solved_by);
  $("#r-verdict").textContent = solvedIt ? "🏆" : h.outcome === "hanged" ? "💀" : h.outcome === "time" ? "⏰" : result.points > 0 ? "👍" : "😬";
  $("#r-text").textContent = solvedIt ? "You solved it!"
    : h.outcome === "solved" ? `${solver ? solver.name : "Someone"} solved it`
    : h.outcome === "hanged" ? "Hanged!" : "Time's up!";
  $("#r-points").textContent = result.points ? `${result.points > 0 ? "+" : ""}${result.points}` : "";
  $("#r-answer").textContent = `Answer: ${h.board}`;
  if (fresh && navigator.vibrate) navigator.vibrate(solvedIt ? [80, 40, 80] : 60);
}

function ordinal(n) {
  const s = ["th", "st", "nd", "rd"], v = n % 100;
  return n + (s[(v - 20) % 10] || s[v] || s[0]);
}

// ---------- Controls ----------

$("#join-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const code = $("#code-input").value.trim().toUpperCase();
  const name = $("#name-input").value.trim();
  if (code.length !== 4) return ($("#join-error").textContent = "Room codes are 4 letters.");
  TBX.save("tbx:name", name);
  // A saved token for this room means we're rejoining as the same player.
  const saved = TBX.load(PLAYER_KEY);
  joinGame(code, name, saved && saved.code === code && saved.name === name ? saved.token : null);
});

$("#code-input").addEventListener("input", (e) => {
  e.target.value = e.target.value.toUpperCase().replace(/[^A-Z]/g, "");
  if (e.target.value.length === 4) refreshJoinChars();
});

// Grey out characters already taken in the room being joined.
let joinTaken = [];
async function refreshJoinChars() {
  const code = $("#code-input").value;
  joinTaken = [];
  if (code.length === 4) {
    try {
      const res = await fetch(`/api/rooms/${code}`);
      if (res.ok) joinTaken = (await res.json()).taken_characters || [];
    } catch { /* offline; the server still checks */ }
  }
  if (picked && joinTaken.includes(picked)) picked = null;
  renderJoinChars();
}
function renderJoinChars() {
  TBX.characterGrid($("#join-chars"), picked, joinTaken, (id) => {
    picked = id;
    TBX.save("tbx:character", id);
    renderJoinChars();
  });
  const ch = TBX.characters[picked];
  $("#join-char-label").textContent = ch ? ch.name : "Tap one, or we'll pick for you";
}

$("#rename-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const name = $("#rename-input").value.trim();
  if (!name) return;
  send({ type: "profile", name });
  if (me) me.name = name;
  TBX.save("tbx:name", name);
  const saved = TBX.load(PLAYER_KEY);
  if (saved) TBX.save(PLAYER_KEY, { ...saved, name });
  $("#rename-input").value = "";
});

$("#start-btn").addEventListener("click", () => send({ type: "start" }));
$("#again-btn").addEventListener("click", () => send({ type: "play_again" }));
document.querySelectorAll("[data-vip-next]").forEach((b) => b.addEventListener("click", () => send({ type: "next" })));
document.querySelectorAll("[data-vip-end]").forEach((b) => b.addEventListener("click", () => {
  if (confirm("End this game and go back to the lobby? Scores won't count.")) send({ type: "lobby" });
}));
$("#leave-btn").addEventListener("click", () => {
  conn && conn.close();
  conn = null;
  TBX.save(PLAYER_KEY, null);
  state = null;
  show("join");
  refreshJoinChars();
});

// ---------- Putting the game on the TV ----------

// The TV page opens its own room, so all a TV needs is a browser at this address.
async function tvAddress() {
  let base = location.origin;
  try {
    const cfg = await (await fetch("/api/config")).json();
    base = TBX.joinBase(cfg.public_url);
  } catch { /* this page's address works for the TV too */ }
  return `${base.replace(/^https?:\/\//, "")}/tv`;
}
document.querySelectorAll("[data-tv-help]").forEach((b) => b.addEventListener("click", async () => {
  // Inside the cast app, the app walks through Smart View itself.
  if (window.TriviaBixApp) return window.TriviaBixApp.cast();
  $("#tv-addr").textContent = await tvAddress();
  $("#tv-sheet").classList.remove("hidden");
}));
// The app is for Android; iPhones get the other ways.
if (!/Android/i.test(navigator.userAgent)) $("#tv-app-steps").open = false;
$("#tv-close").addEventListener("click", () => $("#tv-sheet").classList.add("hidden"));
$("#tv-here").addEventListener("click", () => {
  if (conn && !confirm("This phone becomes the TV and leaves the game. Play on another phone. Go?")) return;
  if (conn) {
    conn.close();
    TBX.save(PLAYER_KEY, null);
  }
  location.href = "/tv";
});

// ---------- Boot ----------

(async function boot() {
  await TBX.loadCharacters();
  const params = new URLSearchParams(location.search);
  const saved = TBX.load(PLAYER_KEY);
  const code = (params.get("code") || "").toUpperCase();
  $("#name-input").value = TBX.load("tbx:name") || "";
  $("#code-input").value = code || (saved && saved.code) || "";
  // Refreshed mid-game: slide straight back in.
  if (saved && (!code || code === saved.code)) return joinGame(saved.code, saved.name, saved.token);
  show("join");
  refreshJoinChars();
  (code ? $("#name-input") : $("#code-input")).focus();
})();
