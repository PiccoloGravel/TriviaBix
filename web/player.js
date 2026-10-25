// The phone controller.

const { $, el, show } = TBX;
const PLAYER_KEY = "tbx:player";

let conn = null;
let state = null;
let me = null; // { code, name, token, id }
let wakeLock = null;
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
        keepAwake();
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
  const target = !state ? $("#join-error") : state.phase === "question" ? $("#q-status") : $("#lobby-error");
  target.textContent = text;
}

// Phones love to sleep mid-question; ask them not to while we're playing.
async function keepAwake() {
  try {
    if ("wakeLock" in navigator && !wakeLock) {
      wakeLock = await navigator.wakeLock.request("screen");
      wakeLock.addEventListener("release", () => { wakeLock = null; });
    }
  } catch { /* not supported or not allowed; fine */ }
}
document.addEventListener("visibilitychange", () => { if (!document.hidden && conn) keepAwake(); });

// ---------- Rendering ----------

function render(s) {
  const prev = state;
  state = s;
  const you = s.you;
  const player = s.players.find((p) => p.id === you.player_id);
  if (!player) return;

  document.querySelectorAll("[data-me]").forEach((box) => box.replaceChildren(
    TBX.avatar(player, "40px"),
    el("span", { class: "name" }, player.name, you.is_vip ? " 👑" : ""),
    el("span", { class: "score" }, player.score.toLocaleString())));
  document.querySelectorAll("[data-vip-next]").forEach((b) => b.classList.toggle("hidden", !you.is_vip));

  show(s.phase);
  const fresh = !prev || prev.phase !== s.phase || prev.round !== s.round;

  if (s.phase === "lobby") {
    $("#lobby-count").textContent = `${s.players.length} of ${s.max_players} players · room ${s.code}`;
    $("#vip-box").classList.toggle("hidden", !you.is_vip);
    $("#lobby-wait").classList.toggle("hidden", you.is_vip);
    $("#lobby-error").textContent = "";
    const taken = s.players.map((p) => p.character);
    TBX.characterGrid($("#lobby-chars"), player.character, taken, (id) => {
      picked = id;
      TBX.save("tbx:character", id);
      send({ type: "profile", character: id });
    });
    $("#rename-input").placeholder = player.name;
  } else if (s.phase === "question") {
    renderQuestion(s, fresh);
  } else if (s.phase === "reveal") {
    const result = s.reveal.results.find((r) => r.player_id === you.player_id) || {};
    const right = result.points > 0;
    $("#r-verdict").textContent = right ? "✅" : result.choice == null ? "⏰" : "❌";
    $("#r-text").textContent = right ? "Correct!" : result.choice == null ? "Too slow!" : "Nope!";
    $("#r-points").textContent = right ? `+${result.points}` : "";
    $("#r-answer").textContent = `Answer: ${s.question.choices[s.reveal.correct]}`;
    // If the host called you out on the TV, you get the badge too.
    const mine = s.host && s.host.callouts.find((c) => c.player_id === you.player_id);
    $("#r-badge").classList.toggle("hidden", !mine);
    if (mine) {
      $("#r-badge").textContent = mine.label;
      $("#r-badge").classList.toggle("good", !["roast", "asleep", "streak_broken", "last_place"].includes(mine.kind));
    }
    if (fresh && navigator.vibrate) navigator.vibrate(right ? 80 : [60, 60, 60]);
  } else if (s.phase === "scoreboard") {
    $("#s-rank").textContent = ordinal(you.rank) + " place";
  } else if (s.phase === "final") {
    $("#f-emoji").textContent = you.rank === 1 ? "🏆" : you.rank <= 3 ? "🎉" : "🫠";
    $("#f-text").textContent = you.rank === 1 ? "You won!" : `You came ${ordinal(you.rank)}`;
    $("#f-score").textContent = `${player.score.toLocaleString()} pts`;
    $("#again-btn").classList.toggle("hidden", !you.is_vip);
  }
}

function renderQuestion(s, fresh) {
  const picked = s.you.answer;
  $("#q-prompt").textContent = s.question.prompt;
  if (fresh) {
    TBX.runTimer($("#q-timer"), s.ms_left, s.phase_ms);
    $("#q-status").textContent = "";
    $("#answers").replaceChildren(...s.question.choices.map((text, i) =>
      el("button", { class: `choice-${i}`, onclick: () => answer(i) },
        el("span", { class: "shape" }, TBX.SHAPES[i]), el("span", {}, text))));
  }
  const buttons = [...$("#answers").children];
  buttons.forEach((b, i) => { b.disabled = picked != null; b.classList.toggle("picked", picked === i); });
  $("#answers").classList.toggle("locked", picked != null);
  if (picked != null) {
    const waiting = s.players.filter((p) => p.connected && !p.answered).length;
    $("#q-status").textContent = waiting ? `Locked in! Waiting on ${waiting} more…` : "Locked in!";
  }
}

function answer(i) {
  if (!state || state.phase !== "question" || state.you.answer != null) return;
  send({ type: "answer", choice: i });
  if (navigator.vibrate) navigator.vibrate(30);
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
$("#leave-btn").addEventListener("click", () => {
  conn && conn.close();
  conn = null;
  TBX.save(PLAYER_KEY, null);
  state = null;
  show("join");
  refreshJoinChars();
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
