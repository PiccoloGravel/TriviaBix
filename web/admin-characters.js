// Bix's character cameos: who they are, their lines, and previews on the TV or right here.

const { $, el, show } = TBX;

async function api(path, method = "GET", body) {
  const res = await fetch(`/api/admin/${path}`, method === "GET" ? {} : {
    method,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body ?? {}),
  });
  const data = await res.json().catch(() => ({}));
  if (res.status === 401) show("login");
  if (!res.ok) throw new Error(data.error || `Request failed (${res.status})`);
  return data;
}

function toast(text, ok = false) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.toggle("ok", ok);
  t.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 3500);
}

const EVENTS = {
  mini_mash: "Button Mash intro",
  mini_closest: "Closest Guess intro",
  mini_quickdraw: "Quick Draw intro",
  mini_win: "Mini game winner",
  fastest: "Fastest answer",
  streak: "Answer streak",
  new_leader: "New leader",
  nobody: "Nobody got it",
  everyone: "Everyone got it",
  round_final: "Final round",
  finale_win: "Champion",
};

/** Rooms the TV preview can go to: they need a TV and to be between games. */
async function loadRooms() {
  const { rooms } = await api("rooms");
  const pick = $("#room");
  const was = pick.value;
  // Rooms with a TV watching come first.
  const ok = rooms.filter((r) => r.phase === "lobby" || r.phase === "final")
    .sort((a, b) => b.host_connected - a.host_connected);
  pick.replaceChildren(...(ok.length
    ? ok.map((r) => el("option", { value: r.code },
      `${r.code}${r.host_connected ? "" : " (no TV connected)"}${r.phase === "final" ? " · final scores" : ""}`))
    : [el("option", { value: "" }, "No room in its lobby")]));
  if (ok.some((r) => r.code === was)) pick.value = was;
  $("#room-note").textContent = ok.length
    ? ""
    : rooms.length ? "Rooms mid-game can't preview; finish or go back to the lobby." : "Open a room on the Rooms page first.";
}

async function playOnTv(c) {
  const code = $("#room").value;
  if (!code) return toast("Pick a room in its lobby first.");
  try {
    await api(`rooms/${code}`, "POST", { action: "cameo", id: c.id });
    toast(`${c.name} is on the TV in ${code}.`, true);
  } catch (e) {
    toast(e.message);
  }
}

let playing = null;

/** Plays the preview here: Bix's own voice if it's made, else this browser's voice. */
function hearHere(c) {
  if (playing) playing.pause();
  if ("speechSynthesis" in window) speechSynthesis.cancel();
  const browser = () => {
    if (!("speechSynthesis" in window)) return toast("This browser can't speak.");
    const u = new SpeechSynthesisUtterance(c.sample);
    u.rate = 1.05 * c.rate;
    u.pitch = 1.1 * (c.shift ? c.rate : 1);
    speechSynthesis.speak(u);
  };
  if (!c.audio) return browser();
  const a = new Audio(c.audio);
  a.preservesPitch = !c.shift;
  a.playbackRate = c.rate;
  a.onerror = browser;
  playing = a;
  a.play().catch(browser);
}

function card(c) {
  const voice = `${Math.round(c.rate * 100) / 100}× speed${c.shift ? ", pitch moves with it" : ""}`;
  return el("div", { class: "card" },
    el("div", { class: "card-head" },
      el("div", { class: "costume" }, c.emoji),
      el("div", {},
        el("div", { class: "name" }, c.name),
        el("div", { class: "slug" }, `[as:${c.id}]`))),
    el("p", { class: "who" }, `Bix as ${c.who}.`),
    el("div", { class: "meta" },
      el("span", { class: "badge" }, `Fits: ${c.fits}`),
      el("span", { class: "badge" }, `🎙️ ${voice}`),
      el("span", { class: "badge" }, `${c.question_lines} question line${c.question_lines === 1 ? "" : "s"}`)),
    el("div", { class: "sample" }, c.sample),
    el("div", { class: "actions" },
      el("button", { type: "button", onclick: () => playOnTv(c) }, "📺 Play on TV"),
      el("button", { type: "button", class: "secondary", onclick: () => hearHere(c) }, "🔊 Hear it here")),
    c.script.length
      ? el("details", {},
        el("summary", {}, `${c.script.length} game show line${c.script.length === 1 ? "" : "s"}`),
        el("ul", {}, ...c.script.map((l) =>
          el("li", {}, l.text, " ", el("span", {}, `${EVENTS[l.event] || l.event} · ${l.rating}`)))))
      : el("details", {}, el("summary", {}, "Only in question lines")));
}

async function load() {
  try {
    const data = await api("cameos");
    show("page");
    $("#setting").textContent = data.setting;
    $("#cards").replaceChildren(...data.cameos.map(card));
    await loadRooms();
  } catch (e) {
    if (!$("#login").classList.contains("hidden")) return;
    toast(e.message);
  }
}

$("#refresh").onclick = () => loadRooms().catch((e) => toast(e.message));
load();
