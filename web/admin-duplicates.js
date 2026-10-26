// Look-alike questions in the bank: hide one of each pair, or keep both.

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

async function load() {
  try {
    const report = await api("duplicates");
    show("report");
    render(report);
  } catch (e) {
    if (!$("#login").classList.contains("hidden")) return;
    toast(e.message);
  }
}

async function act(promise, done) {
  try {
    await promise;
    toast(done, true);
    await load();
  } catch (e) {
    toast(e.message);
  }
}

const setHidden = (q, hidden) => api(`questions/${q.id}/hidden`, "POST", { hidden });

function render(report) {
  const sure = report.pairs.filter((p) => p.verdict === "duplicate").length;
  $("#stat-sure").textContent = sure;
  $("#stat-maybe").textContent = report.pairs.length - sure;
  $("#pairs").replaceChildren(...(report.pairs.length
    ? report.pairs.map(pairCard)
    : [el("p", { class: "none" }, "No duplicates. Every question asks something different.")]));
  $("#hidden").replaceChildren(...(report.hidden.length
    ? report.hidden.map(hiddenCard)
    : [el("p", { class: "none" }, "Nothing hidden.")]));
}

function side(q, ...actions) {
  return el("div", { class: "side" },
    el("div", { class: "meta" },
      el("span", { class: "badge" }, q.category),
      el("span", { class: "badge" }, q.rating),
      q.kind !== "multiple_choice" ? el("span", { class: "badge" }, q.kind.replace("_", " ")) : null,
      el("span", {}, q.bank_id || `#${q.id}`),
      el("span", {}, q.source)),
    el("div", { class: "prompt" }, q.prompt),
    el("div", { class: "answer" }, "Answer: ", q.answer),
    el("div", { class: "actions" }, ...actions));
}

function pairCard(p) {
  const keep = (q, other) => el("button", {
    class: "small",
    onclick: () => act(setHidden(other, true), "Hidden. The other one stays."),
  }, "Keep this one");
  return el("div", { class: "pair" },
    el("div", { class: "pair-head" },
      el("span", { class: `badge ${p.verdict === "duplicate" ? "sure" : "maybe"}` },
        p.verdict === "duplicate" ? "Same question" : "Might be the same"),
      el("span", {}, `${Math.round(p.score * 100)}% of the wording matches`),
      el("span", { style: "flex:1" }),
      el("button", {
        class: "secondary small",
        onclick: () => act(api("duplicates/keep", "POST", { a: p.a.id, b: p.b.id }), "Kept both."),
      }, "Different questions")),
    el("div", { class: "sides" }, side(p.a, keep(p.a, p.b)), side(p.b, keep(p.b, p.a))));
}

function hiddenCard(q) {
  return side(q, el("button", {
    class: "secondary small",
    onclick: () => act(setHidden(q, false), "It's back in the game."),
  }, "Bring back"));
}

load();
