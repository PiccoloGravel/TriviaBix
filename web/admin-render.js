// Run the rendering jobs (Bix's voice, cutscene art, host lines) from the admin panel: see what's
// missing, check the price with a dry run, confirm, then follow the log until the GPU is given back.

const { $, el, show } = TBX;

const KINDS = {
  voice: { title: "🎙️ Bix's voice", what: "voice lines waiting", gpu: true },
  art: { title: "🎨 Cutscene art", what: "pieces of art the TV doesn't have yet", gpu: true },
  lines: { title: "✍️ Host lines (Claude)", what: "questions without a lead-in or reveal line", gpu: false },
  cleanup: { title: "🧹 Destroy leftover GPUs" },
};
const STATES = { running: "Running", done: "Done", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted by a restart" };

let status = null;
/** kind -> the last dry run's answer. */
const quotes = {};
/** kind -> the options picked on its card. */
const picked = { voice: { max_hours: 3 }, art: { kinds: "all" }, lines: { limit: 50, missing: "either" } };
let checking = null;
/** The job shown in the panel, and how much of its log is on screen. */
let shown = null;
let logSeq = 0;
let logLines = [];
let timer = null;

async function api(path, body) {
  const res = await fetch(`/api/admin/${path}`, body === undefined ? {} : {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await res.json().catch(() => ({}));
  if (res.status === 401) {
    clearTimeout(timer);
    show("login");
  }
  if (!res.ok) throw new Error(data.error || `Request failed (${res.status})`);
  return data;
}

function toast(text, ok = false) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.toggle("ok", ok);
  t.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 4000);
}

const money = (n) => n == null ? "–" : `$${n < 0.1 && n > 0 ? n.toFixed(3) : n.toFixed(2)}`;
const time = (ms) => new Date(ms).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });

function duration(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.round(s / 60);
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)}h ${m % 60}m`;
}

function select(options, value, onchange) {
  const s = el("select", { onchange: (e) => onchange(e.target.value) },
    options.map(([v, label]) => el("option", { value: v }, label)));
  s.value = String(value);
  return s;
}

// ---------- Loading ----------

async function load(refresh = false) {
  try {
    status = await api(`render${refresh ? "?refresh=true" : ""}`);
  } catch (e) {
    if (refresh) toast(e.message);
    return;
  }
  render();
  if (status.job && (status.job.state === "running" || !shown)) follow(status.job);
  schedule();
}

function schedule() {
  clearTimeout(timer);
  const running = status && status.job && status.job.state === "running";
  timer = setTimeout(tick, running ? 2000 : 15000);
}

async function tick() {
  if (shown && shown.state === "running") {
    try {
      const job = await api(`render/jobs/${shown.id}?since=${logSeq}`);
      follow(job, true);
      if (job.state !== "running") {
        toast(`${KINDS[job.kind].title}: ${STATES[job.state].toLowerCase()}`, job.state === "done");
        await load();
      }
    } catch { /* try again */ }
    schedule();
  } else {
    await load();
  }
}

// ---------- Setup notices ----------

function renderSetup() {
  const r = status.ready;
  const todo = [];
  if (!r.python || !r.gpu_tools || !r.art_tools) todo.push(["Python and the GPU scripts aren't in this server. Use the ghcr.io/piccologravel/triviabix image, which has them."]);
  if (!r.line_writer) todo.push(["The ", el("code", {}, "triviabix-questions"), " tool isn't in this server, so host lines can't be written here."]);
  if (!r.vast_key) todo.push(["Voice and art rent GPUs on vast.ai: set ", el("code", {}, "VAST_API_KEY"), " in the container's settings (Unraid Docker tab → TriviaBix → Edit). Make the key at cloud.vast.ai/manage-keys."]);
  if (!r.anthropic_key) todo.push(["Host lines are written by Claude: set ", el("code", {}, "ANTHROPIC_API_KEY"), " in the container's settings. Calls are billed to that Anthropic account."]);
  const box = $("#setup");
  box.classList.toggle("hidden", !todo.length);
  box.replaceChildren(el("b", {}, "To finish setting up"), el("ul", {}, todo.map((t) => el("li", {}, ...t))));
}

// ---------- Job cards ----------

function canRun(kind) {
  const r = status.ready;
  if (kind === "lines") return r.line_writer && r.anthropic_key;
  return r.python && r.gpu_tools && r.vast_key && (kind !== "art" || r.art_tools);
}

function missingCount(kind) {
  const m = status.missing;
  return kind === "voice" ? m.voice : m[kind];
}

function options(kind) {
  const set = (key, cast = String) => (v) => { picked[kind][key] = cast(v); delete quotes[kind]; renderCards(); };
  const o = picked[kind];
  if (kind === "voice") {
    return el("label", { class: "inline" }, "GPU for at most",
      select([[0.5, "30 minutes"], [1, "1 hour"], [2, "2 hours"], [3, "3 hours"]], o.max_hours, set("max_hours", Number)));
  }
  if (kind === "art") {
    return el("label", { class: "inline" }, "Make",
      select([["all", "Everything missing"], ["backdrop", "Backdrops"], ["sticker", "Stickers"], ["category", "Category cards"], ["clip", "Clips"]],
        o.kinds, set("kinds")));
  }
  return [
    el("label", { class: "inline" }, "Up to",
      select([[10, "10"], [25, "25"], [50, "50"], [100, "100"], [200, "200"], [500, "500"]], o.limit, set("limit", Number)), "questions"),
    el("label", { class: "inline" }, "Write",
      select([["either", "Any missing"], ["lead-in", "Lead-ins"], ["reveal", "Reveal lines"]], o.missing, set("missing"))),
  ];
}

function quoteBox(kind) {
  const q = quotes[kind];
  if (checking === kind) return el("div", { class: "quote" }, "Checking… (this rents nothing)");
  if (!q) return null;
  if (!q.ok) {
    return el("div", { class: "quote" },
      el("div", { class: "error" }, "Can't start: ", q.error || "the dry run failed"),
      q.log.length ? el("details", {}, el("summary", {}, "What it said"), el("pre", { class: "log" }, q.log.join("\n"))) : null);
  }
  if (q.nothing) return el("div", { class: "quote" }, "Nothing to make, so nothing would be rented.");
  const d = q.details || {};
  const lines = [];
  if (kind === "voice") lines.push(`${d.backlog} lines to voice`);
  if (kind === "art" && d.todo) lines.push(`${d.todo.count} pieces need a GPU${d.todo.clips ? ` (${d.todo.clips} clips)` : ""}, about ${d.todo.minutes} minutes`);
  if (kind === "lines" && d.todo) lines.push(`Lines for ${d.todo.count} of ${d.todo.total} questions, about ${Math.ceil(d.todo.count / 10)} ${d.todo.count > 10 ? "calls" : "call"} to Claude`);
  const head = q.gpu
    ? el("div", { class: "big" }, `${q.gpu} · ${money(q.price_hr)}/hr`)
    : el("div", { class: "big" }, "Billed to your Anthropic account");
  return el("div", { class: "quote" },
    head,
    ...lines.map((l) => el("div", {}, l)),
    q.max_cost != null ? el("div", {}, `Stops after ${duration(q.minutes * 60000)} at most, so it can cost at most `, el("b", {}, money(q.max_cost)), ".") : null,
    el("div", { class: "row" },
      el("button", { onclick: () => start(kind) }, q.max_cost != null ? `Start, up to ${money(q.max_cost)}` : "Start"),
      el("button", { class: "secondary", onclick: () => { delete quotes[kind]; renderCards(); } }, "Never mind")));
}

function card(kind) {
  const k = KINDS[kind];
  const n = missingCount(kind);
  const running = status.job && status.job.state === "running";
  const btn = el("button", { class: "secondary", onclick: () => check(kind) }, k.gpu ? "Check the price" : "Check what it'd write");
  btn.disabled = running || !!checking || !canRun(kind);
  return el("div", { class: "card" },
    el("h2", {}, k.title),
    el("div", {}, el("span", { class: "count" }, n == null ? "?" : n.toLocaleString()), " ", el("span", { class: "what" }, k.what)),
    el("div", { class: "opts" }, options(kind)),
    quoteBox(kind) || btn);
}

function renderCards() {
  $("#cards").replaceChildren(...["voice", "art", "lines"].map(card));
}

async function check(kind) {
  checking = kind;
  renderCards();
  try {
    quotes[kind] = await api("render/preview", { kind, options: picked[kind] });
  } catch (e) {
    toast(e.message);
  }
  checking = null;
  renderCards();
}

async function start(kind) {
  const q = quotes[kind];
  const sure = q.max_cost != null
    ? `Rent ${q.gpu} at ${money(q.price_hr)}/hr? It stops by itself after ${duration(q.minutes * 60000)}, so it can cost at most ${money(q.max_cost)}.`
    : `Have Claude write lines for ${q.details.todo.count} questions? It's billed to your Anthropic account.`;
  if (!confirm(sure)) return;
  delete quotes[kind];
  try {
    const job = await api("render/start", { token: q.token, confirm: true });
    shown = null;
    follow(job);
    await load();
  } catch (e) {
    toast(e.message);
    renderCards();
  }
}

// ---------- The job panel ----------

function follow(job, append = false) {
  if (!append || !shown || shown.id !== job.id) {
    logLines = [];
    logSeq = 0;
  }
  for (const [n, line] of job.log) {
    if (n > logSeq) logLines.push(line);
  }
  logSeq = Math.max(logSeq, job.log_seq);
  if (logLines.length > 600) logLines = logLines.slice(-500);
  shown = job;
  renderJob();
}

function progress(job) {
  const d = job.details || {};
  if (job.kind === "voice" && d.backlog_start) {
    const done = d.backlog_start - (d.backlog ?? d.backlog_start);
    return [done / d.backlog_start, `${done.toLocaleString()} of ${d.backlog_start.toLocaleString()} lines voiced`];
  }
  if (job.kind === "lines" && d.progress) {
    return [d.progress.batch / d.progress.batches,
      `${d.progress.written} questions written · ${d.progress.input_tokens.toLocaleString()} tokens in, ${d.progress.output_tokens.toLocaleString()} out`];
  }
  if (job.kind === "art" && d.todo) return [null, `${d.todo.count} pieces to make`];
  return [null, null];
}

function renderJob() {
  const box = $("#job");
  const job = shown;
  if (!job) { box.replaceChildren(); return; }
  const running = job.state === "running";
  const end = job.finished_at || Date.now();
  const [frac, says] = progress(job);
  const pre = el("pre", { class: "log" }, logLines.join("\n") || "Starting…");
  const cancel = running ? el("button", { class: "danger", onclick: () => cancelJob(job) }, job.cancelling ? "Stopping…" : "✖ Cancel") : null;
  if (cancel) cancel.disabled = job.cancelling;
  box.replaceChildren(el("div", { class: "job" },
    el("div", { class: "job-head" },
      el("h2", {}, KINDS[job.kind].title),
      el("span", { class: `badge ${job.state}` }, STATES[job.state] || job.state),
      el("span", { style: "flex:1" }),
      cancel,
      !running ? el("button", { class: "secondary small", onclick: () => { shown = null; renderJob(); } }, "Close") : null),
    el("div", { class: "meta" },
      el("span", {}, `Started ${time(job.started_at)}`),
      el("span", {}, running ? `running ${duration(end - job.started_at)}` : `took ${duration(end - job.started_at)}`),
      job.gpu ? el("span", {}, el("b", {}, job.gpu), ` at ${money(job.price_hr)}/hr`) : null,
      job.cost != null ? el("span", {}, running ? "so far " : "cost ", el("b", {}, money(job.cost))) : null,
      job.max_cost != null ? el("span", {}, `at most ${money(job.max_cost)}`) : null,
      job.instance && !job.released ? el("span", {}, `GPU ${job.instance} rented`) : null,
      job.instance && job.released ? el("span", {}, "GPU given back") : null),
    frac != null ? el("div", { class: "bar" }, el("div", { style: `width:${Math.round(frac * 100)}%` })) : null,
    says ? el("div", { class: "meta" }, says) : null,
    pre));
  pre.scrollTop = pre.scrollHeight;
  renderCards();
}

async function cancelJob(job) {
  const gpu = KINDS[job.kind].gpu;
  if (!confirm(gpu ? "Cancel and give the GPU back? Whatever is made so far is kept." : "Stop writing lines? Lines written so far are kept.")) return;
  try {
    await api(`render/jobs/${job.id}/cancel`, {});
    shown.cancelling = true;
    renderJob();
  } catch (e) {
    toast(e.message);
  }
}

// ---------- History ----------

function renderHistory() {
  const rows = status.history.map((j) => {
    const d = j.details || {};
    const what = [KINDS[j.kind].title];
    if (j.kind === "lines" && d.progress) what.push(`${d.progress.written} questions`);
    if (j.kind === "voice" && d.backlog_start != null) what.push(`${(d.backlog_start - (d.backlog ?? d.backlog_start)).toLocaleString()} lines`);
    if (j.kind === "art" && d.todo) what.push(`${d.todo.count} pieces`);
    const cost = j.kind === "lines" && d.progress
      ? `${Math.round((d.progress.input_tokens + d.progress.output_tokens) / 1000)}k tokens`
      : money(j.cost);
    return el("tr", { class: "pick", title: "Show the log", onclick: () => openJob(j.id) },
      el("td", {}, time(j.started_at)),
      el("td", {}, what.join(" · ")),
      el("td", {}, el("span", { class: `badge ${j.state}` }, STATES[j.state] || j.state),
        j.instance && !j.released ? el("span", { class: "error" }, " ⚠️ GPU may still be rented") : null),
      el("td", { class: "hide-narrow" }, j.gpu || "–"),
      el("td", { class: "num" }, j.finished_at ? duration(j.finished_at - j.started_at) : "–"),
      el("td", { class: "num" }, cost));
  });
  $("#history").replaceChildren(...(rows.length ? rows : [el("tr", {}, el("td", { colspan: 6, class: "muted" }, "Nothing yet."))]));
}

async function openJob(id) {
  try {
    follow(await api(`render/jobs/${id}`));
    window.scrollTo({ top: 0, behavior: "smooth" });
  } catch (e) {
    toast(e.message);
  }
}

function render() {
  $("#spent-month").textContent = money(status.spent.month);
  $("#spent-total").textContent = money(status.spent.total);
  renderSetup();
  renderCards();
  renderHistory();
  $("#cleanup").disabled = !status.ready.vast_key || (status.job && status.job.state === "running");
  if (status.missing.errors.length) console.warn("counting what's missing:", status.missing.errors);
}

$("#refresh").addEventListener("click", () => load(true));

$("#cleanup").addEventListener("click", async () => {
  if (!confirm("Destroy every TriviaBix instance on your vast.ai account? Use this if a job was interrupted or something was started from the command line.")) return;
  try {
    shown = null;
    follow(await api("render/cleanup", {}));
    await load();
  } catch (e) {
    toast(e.message);
  }
});

// ---------- Boot ----------

(async function boot() {
  let session = { logged_in: false };
  try { session = await (await fetch("/api/admin/session")).json(); } catch { /* treat as logged out */ }
  if (!session.logged_in) {
    show("login");
    return;
  }
  show("render");
  await load();
})();
