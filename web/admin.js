// The organizer's admin panel: every live room, its players, and controls.
// Polls the server, so it keeps working through flaky Wi-Fi without sockets.

const { $, el, show } = TBX;
const POLL_MS = 1500;
const PLAYER_SIZES = [2, 4, 6, 8, 10, 12, 16, 20, 24];
const RATINGS = ["PG", "PG-13", "MA", "NC-17"];
const SONG_SOURCES = [["mix", "Library + hits"], ["library", "Your library"], ["hits", "Hits"]];
const PHASES = {
  lobby: "Lobby", intro: "Intro", round_intro: "Round intro", question: "Question", reveal: "Reveal",
  scoreboard: "Scores", recap: "Round recap", final: "Final scores",
};

let rooms = [];
let pollTimer = null;
let joinBase = location.origin;

async function api(path, body) {
  const res = await fetch(`/api/admin/${path}`, body === undefined ? {} : {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await res.json().catch(() => ({}));
  if (res.status === 401 && path !== "login") {
    stopPolling();
    show("login");
  }
  if (!res.ok) throw new Error(data.error || `Request failed (${res.status})`);
  return data;
}

function toast(text) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 3500);
}

async function act(code, action, extra = {}) {
  try {
    await api(`rooms/${code}`, { action, ...extra });
    await refresh();
  } catch (e) {
    toast(e.message);
  }
}

// ---------- Login ----------

$("#login-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  $("#login-error").textContent = "";
  try {
    await api("login", { password: $("#password").value });
    $("#password").value = "";
    startDashboard();
  } catch (err) {
    $("#login-error").textContent = err.message;
  }
});

// ---------- Dashboard ----------

function startDashboard() {
  show("dash");
  refresh();
  loadMusic();
  stopPolling();
  pollTimer = setInterval(refresh, POLL_MS);
}

function stopPolling() { clearInterval(pollTimer); pollTimer = null; }

async function refresh() {
  try {
    rooms = (await api("rooms")).rooms;
  } catch {
    return;
  }
  // Don't yank a dropdown out from under the organizer mid-click.
  const focused = document.activeElement;
  if (focused && focused.closest("#rooms") && focused.tagName === "SELECT") return;
  render();
}

function render() {
  $("#stat-rooms").textContent = rooms.length;
  $("#stat-players").textContent = rooms.reduce((n, r) => n + r.players.length, 0);
  const box = $("#rooms");
  if (!rooms.length) {
    box.replaceChildren(el("div", { class: "empty" }, "No games running. Create a room above, or start one from a TV."));
    return;
  }
  // Opened from a phone in a room's lobby: that room comes first.
  const mine = new URLSearchParams(location.search).get("room");
  const ordered = [...rooms].sort((a, b) => (b.code === mine) - (a.code === mine));
  box.replaceChildren(...ordered.map(roomCard));
}

function select(options, value, onchange) {
  const s = el("select", { onchange: (e) => onchange(e.target.value) },
    options.map(([v, label]) => el("option", { value: v }, label)));
  s.value = String(value);
  return s;
}

function roomCard(r) {
  const running = !["lobby", "final"].includes(r.phase);
  const trivia = r.settings.mode === "trivia";
  const lobby = r.phase === "lobby";
  const tvLink = `/host?code=${r.code}&token=${encodeURIComponent(r.host_token)}`;
  const joinLink = `${joinBase}/?code=${r.code}`;
  const waiting = r.players.filter((p) => p.waiting).length;

  const head = el("div", { class: "room-head" },
    el("span", { class: "room-code" }, r.code),
    el("span", { class: "badge " + (running ? "live" : lobby ? "lobby" : "") }, PHASES[r.phase] || r.phase),
    el("span", { class: "badge " + (r.host_connected ? "" : "off"), title: "Is a TV showing this room?" },
      r.host_connected ? "📺 TV on" : "📺 No TV"),
    el("span", { style: "flex:1" }),
    el("a", { href: tvLink, target: "_blank", rel: "noopener" }, "Open TV ↗"));

  const meta = el("div", { class: "meta" },
    r.stage ? el("span", {}, `${r.stage.title} (${r.stage.number}/${r.settings.trivia_rounds}): ${r.stage.category} (${r.stage.difficulty}, ×${r.stage.points_pct / 100})`) : null,
    el("span", {}, running || r.phase === "final" ? `Question ${r.round} of ${r.total_rounds}` : `${r.total_rounds} questions`),
    el("span", {}, `Rated ${r.settings.max_rating}`),
    el("span", {}, `${r.players.length}/${r.max_players} players`),
    r.ms_left != null ? el("span", {}, `${Math.ceil(r.ms_left / 1000)}s left`) : null,
    waiting ? el("span", {}, `${waiting} joining next question`) : null,
    el("span", {}, `Started ${new Date(r.created_at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`));

  const question = r.question && el("div", { class: "question" },
    el("div", { class: "muted" }, r.question.category),
    el("div", {}, r.question.prompt),
    el("div", { class: "answer" }, "Answer: ", r.question.answer));

  const actions = el("div", { class: "actions" },
    lobby && r.players.length ? el("button", { onclick: () => act(r.code, "start") }, "▶ Start") : null,
    running ? el("button", { onclick: () => act(r.code, "skip") }, "⏭ Skip") : null,
    running ? el("button", { class: "secondary", onclick: () => act(r.code, "end_game") }, "🏁 End game") : null,
    !lobby ? el("button", { class: "secondary", onclick: () => {
      if (r.phase === "final" || confirm(`Abandon the game in ${r.code} and go back to the lobby?`)) act(r.code, "lobby");
    } }, "↩ Lobby") : null,
    el("button", { class: "secondary", onclick: () => copy(joinLink) }, "🔗 Join link"),
    el("button", { class: "danger", onclick: () => {
      if (confirm(`Close room ${r.code}? Everyone in it is disconnected.`)) act(r.code, "close");
    } }, "✖ Close room"));

  const lateJoin = el("input", { type: "checkbox", style: "width:auto", onchange: (e) => act(r.code, "settings", { late_join: e.target.checked }) });
  lateJoin.checked = r.settings.late_join;
  // Can be switched mid-game; it takes effect at the next break between rounds.
  const minis = el("input", { type: "checkbox", style: "width:auto", onchange: (e) => act(r.code, "settings", { minigames: e.target.checked }) });
  minis.checked = r.settings.minigames;
  const sizes = [...new Set([...PLAYER_SIZES, r.max_players])].sort((a, b) => a - b)
    .filter((n) => n >= r.players.length);
  const settings = el("div", { class: "settings" },
    el("label", { class: "inline" }, lateJoin, "Late joins"),
    trivia ? el("label", { class: "inline", title: "A quick party game on the phones between rounds" }, minis, "Mini games") : null,
    el("label", { class: "inline" }, "Max players",
      select(sizes.map((n) => [n, n]), r.max_players, (v) => act(r.code, "settings", { max_players: Number(v) }))),
    lobby ? el("label", { class: "inline" }, "Game",
      select([["trivia", "Trivia"], ["hangman", "Speed Hangman"], ["songs", "Name That Tune"]], r.settings.mode, (v) => act(r.code, "settings", { mode: v }))) : null,
    lobby && r.settings.mode === "songs" ? el("label", { class: "inline" }, "Songs from",
      select(SONG_SOURCES, r.settings.song_source, (v) => act(r.code, "settings", { song_source: v }))) : null,
    lobby && r.settings.mode === "songs" ? el("label", { class: "inline" }, "Clip",
      select([[1000, "1 sec"], [2000, "2 sec"], [3000, "3 sec"]], r.settings.clip_ms, (v) => act(r.code, "settings", { clip_ms: Number(v) }))) : null,
    lobby && trivia ? el("label", { class: "inline" }, "Rounds",
      select([3, 4].map((n) => [n, n]), r.settings.trivia_rounds, (v) => act(r.code, "settings", { trivia_rounds: Number(v) }))) : null,
    lobby && trivia ? el("label", { class: "inline" }, "Per round",
      select([3, 5, 7, 10].map((n) => [n, n]), r.settings.per_round, (v) => act(r.code, "settings", { per_round: Number(v) }))) : null,
    lobby && !trivia ? el("label", { class: "inline" }, "Questions",
      select([5, 10, 15, 20, 30].map((n) => [n, n]), r.settings.rounds, (v) => act(r.code, "settings", { rounds: Number(v) }))) : null,
    lobby ? el("label", { class: "inline" }, "Rated",
      select(RATINGS.map((x) => [x, x]), r.settings.max_rating, (v) => act(r.code, "settings", { max_rating: v }))) : null,
    lobby ? el("label", { class: "inline" }, "Teams",
      select([[0, "Off"], [2, 2], [3, 3], [4, 4]], r.settings.teams, (v) => act(r.code, "settings", { teams: Number(v) }))) : null);

  const teams = r.teams.length ? el("div", { class: "teams" }, r.teams.map((t) =>
    el("span", { class: "team", style: `background:${t.color}` }, `${t.name}: ${t.score.toLocaleString()}`))) : null;

  const sorted = [...r.players].sort((a, b) => b.score - a.score);
  const table = r.players.length ? el("table", {},
    el("thead", {}, el("tr", {},
      el("th", {}, "Player"), el("th", {}, "Status"), r.teams.length ? el("th", {}, "Team") : null,
      el("th", { style: "text-align:right" }, "Score"), el("th", {}))),
    el("tbody", {}, sorted.map((p) => el("tr", {},
      el("td", { class: "who" }, TBX.avatar(p, "26px"), " ", p.name, p.id === r.vip ? " 👑" : ""),
      el("td", { class: "muted" }, status(p, r)),
      r.teams.length ? el("td", {}, select(
        [["", "None"], ...r.teams.slice().sort((a, b) => a.id - b.id).map((t) => [t.id, t.name])],
        p.team == null ? "" : p.team,
        (v) => act(r.code, "set_team", { player_id: p.id, team: v === "" ? null : Number(v) }))) : null,
      el("td", { class: "score" }, p.score.toLocaleString()),
      el("td", { style: "text-align:right" }, el("button", { class: "small secondary", title: `Kick ${p.name}`, onclick: () => {
        if (confirm(`Kick ${p.name} from ${r.code}?`)) act(r.code, "kick", { player_id: p.id });
      } }, "Kick")))))) : el("div", { class: "muted" }, "No players yet.");

  return el("div", { class: "room" }, head, meta, question, actions, settings, teams, table);
}

function status(p, r) {
  if (p.waiting) return "⏳ Joins next question";
  if (!p.connected) return "💤 Away";
  if (r.phase === "question") return p.answered ? "✅ Answered" : "🤔 Thinking";
  return "🟢 Online";
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast("Join link copied");
  } catch {
    prompt("Join link", text);
  }
}

$("#new-room").addEventListener("submit", async (e) => {
  e.preventDefault();
  const f = e.target;
  try {
    const room = await api("rooms", {
      trivia_rounds: Number(f.trivia_rounds.value),
      per_round: Number(f.per_round.value),
      max_rating: f.max_rating.value,
      teams: Number(f.teams.value),
      late_join: f.late_join.checked,
      minigames: f.minigames.checked,
      max_players: Number(f.max_players.value),
      song_source: f.song_source.value,
    });
    toast(`Room ${room.code} is open. Use "Open TV" to put it on a screen.`);
    await refresh();
  } catch (err) {
    toast(err.message);
  }
});

// ---------- Music library ----------

let musicTimer = null;

function showMusic(m) {
  const parts = [];
  if (!m.configured) {
    parts.push("Off. Add Lidarr's address and API key in Settings to use your own music.");
  } else {
    parts.push(`${m.songs.toLocaleString()} songs ready`);
    if (m.syncing) parts.push("syncing with Lidarr…");
    else if (m.last_sync) parts.push(`synced ${new Date(m.last_sync).toLocaleString()}`);
    if (m.last_report && m.last_report.matched) parts.push(`${m.last_report.matched} of the built-in hits play from your files`);
    if (m.missing_files) parts.push(`⚠️ ${m.missing_files.toLocaleString()} files not found in the container`);
    if (m.error) parts.push(`⚠️ ${m.error}`);
  }
  $("#music-status").textContent = parts.join(" · ");
  $("#music-sync").classList.toggle("hidden", !m.configured);
  $("#music-sync").disabled = !!m.syncing;
  clearTimeout(musicTimer);
  if (m.syncing) musicTimer = setTimeout(loadMusic, 2000);
}

async function loadMusic() {
  try { showMusic(await api("music")); } catch { /* the rooms poll reports trouble */ }
}

$("#music-sync").addEventListener("click", async () => {
  try { showMusic(await api("music/sync", {})); } catch (e) { toast(e.message); }
});

// ---------- Boot ----------

(async function boot() {
  await TBX.loadCharacters();
  try {
    const cfg = await (await fetch("/api/config")).json();
    joinBase = TBX.joinBase(cfg.public_url);
  } catch { /* use this page's address */ }
  let session = { enabled: false, logged_in: false };
  try { session = await (await fetch("/api/admin/session")).json(); } catch { /* treat as logged out */ }
  if (!session.enabled) {
    show("login");
    $("#login-form").classList.add("hidden");
    $("#login-error").textContent = "The admin panel is off. Set ADMIN_PASSWORD on the server and restart it.";
    return;
  }
  if (session.logged_in) startDashboard();
  else {
    show("login");
    $("#password").focus();
  }
})();
