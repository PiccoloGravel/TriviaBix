// Browse every question in the database and hand-edit what Bix says about it.

const { $, el, show } = TBX;
const RATINGS = ["PG", "PG-13", "MA", "NC-17"];
const SLOTS = { lead_in: "Lead-in", reveal: "Reveal" };
const PAGE = 25;

let offset = 0;
let total = 0;
let questions = [];
/** "questionId:slot" -> lines being edited. */
const editing = new Map();

async function api(path, method = "GET", body) {
  const res = await fetch(`/api/admin/${path}`, body === undefined && method === "GET" ? {} : {
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

async function load() {
  const params = new URLSearchParams({
    q: $("#search").value,
    missing: $("#missing").value,
    offset,
    limit: PAGE,
  });
  try {
    const page = await api(`questions?${params}`);
    show("editor");
    questions = page.questions;
    total = page.total;
    $("#stat-active").textContent = page.active;
    $("#stat-lead").textContent = page.with_lead_in;
    $("#stat-reveal").textContent = page.with_reveal;
    render();
  } catch (e) {
    if (!$("#login").classList.contains("hidden")) return;
    toast(e.message);
  }
}

function render() {
  $("#list").replaceChildren(...(questions.length
    ? questions.map(card)
    : [el("p", { class: "none" }, "No questions match.")]));
  const page = Math.floor(offset / PAGE) + 1;
  const pages = Math.max(1, Math.ceil(total / PAGE));
  $("#page-info").textContent = `Page ${page} of ${pages} · ${total} questions`;
  $("#prev").disabled = offset === 0;
  $("#next").disabled = offset + PAGE >= total;
}

function card(q) {
  return el("div", { class: "q" },
    el("div", { class: "q-head" },
      el("span", { class: "badge" }, q.category),
      el("span", { class: "badge" }, q.rating),
      el("span", { class: "badge" }, q.difficulty),
      q.kind !== "multiple_choice" ? el("span", { class: "badge" }, q.kind.replace("_", " ")) : null,
      el("span", {}, q.bank_id || `#${q.id}`),
      el("span", {}, q.source)),
    el("div", { class: "prompt" }, q.prompt),
    el("div", { class: "answer" }, "Answer: ", q.answer),
    ...Object.keys(SLOTS).map((slot) => slotBox(q, slot)));
}

/** The lines the game uses for a slot: edited ones if there are any, otherwise the pack's. */
function effective(q, slot) {
  const lines = q.lines.filter((l) => l.slot === slot);
  const edited = lines.filter((l) => l.origin === "edited");
  return { lines: edited.length ? edited : lines, edited: edited.length > 0, hasContent: lines.length > edited.length };
}

function slotBox(q, slot) {
  const key = `${q.id}:${slot}`;
  const { lines, edited, hasContent } = effective(q, slot);
  const head = el("div", { class: "slot-head" },
    el("span", { class: "name" }, SLOTS[slot]),
    edited ? el("span", { class: "badge edited" }, "edited") : null);

  if (editing.has(key)) {
    const rows = editing.get(key);
    const box = el("div", { class: "slot" }, head);
    rows.forEach((row, i) => {
      const rating = el("select", { onchange: (e) => { row.rating = e.target.value; } },
        RATINGS.map((r) => el("option", { value: r }, r)));
      rating.value = row.rating;
      const text = el("input", { type: "text", maxlength: "280", placeholder: "What Bix says", oninput: (e) => { row.text = e.target.value; } });
      text.value = row.text;
      box.append(el("div", { class: "edit-row" }, rating, text,
        el("button", { class: "secondary small", type: "button", title: "Remove", onclick: () => { rows.splice(i, 1); render(); } }, "✕")));
    });
    box.append(el("div", { class: "edit-actions" },
      el("button", { class: "secondary small", onclick: () => { rows.push({ rating: q.rating, text: "" }); render(); } }, "+ Line"),
      el("button", { class: "small", onclick: () => save(q, slot, rows.filter((r) => r.text.trim())) }, "Save"),
      el("button", { class: "secondary small", onclick: () => { editing.delete(key); render(); } }, "Cancel")));
    return box;
  }

  const shown = lines.filter((l) => l.text.trim());
  const body = shown.length
    ? shown.map((l) => el("div", { class: "line" }, el("span", { class: "r" }, l.rating), el("span", {}, l.text)))
    : [el("div", { class: "none" }, edited ? "Muted: Bix says nothing here." : "Nothing yet. Bix uses his usual lines.")];
  return el("div", { class: "slot" },
    head,
    ...body,
    el("div", { class: "edit-actions" },
      el("button", { class: "secondary small", onclick: () => {
        editing.set(key, shown.length ? shown.map((l) => ({ rating: l.rating, text: l.text })) : [{ rating: q.rating, text: "" }]);
        render();
      } }, "Edit"),
      edited ? el("button", { class: "secondary small", onclick: () => save(q, slot, []) }, hasContent ? "Use pack lines" : "Clear") : null,
      !(edited && !shown.length) && (shown.length || hasContent)
        ? el("button", { class: "secondary small", onclick: () => save(q, slot, [{ rating: "PG", text: "" }]) }, "Mute")
        : null));
}

async function save(q, slot, lines) {
  try {
    await api(`questions/${q.id}/lines/${slot}`, "PUT", { lines });
    editing.delete(`${q.id}:${slot}`);
    toast("Saved. New games use it right away.", true);
    await load();
  } catch (e) {
    toast(e.message);
  }
}

$("#filters").addEventListener("submit", (e) => { e.preventDefault(); offset = 0; load(); });
$("#missing").addEventListener("change", () => { offset = 0; load(); });
$("#prev").addEventListener("click", () => { offset = Math.max(0, offset - PAGE); load(); });
$("#next").addEventListener("click", () => { offset += PAGE; load(); });
$("#reload").addEventListener("click", async () => {
  try {
    const r = await api("content/reload", "POST", {});
    toast(`Reloaded: ${r.added} new, ${r.updated} updated, ${r.retired} retired.`, true);
    if (r.warnings.length) console.warn("Content warnings:", r.warnings);
    await load();
  } catch (e) {
    toast(e.message);
  }
});

load();
