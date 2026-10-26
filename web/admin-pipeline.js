// The content pipeline: import trivia, then have Claude write Bix's lines, voice them and make any
// new art. Each paid step shows what's missing, checks the price with a dry run, waits for a
// confirm, then follows the log until the GPU is given back. Each import is tracked through the steps.

const { $, el, show } = TBX;

const KINDS = {
  import: { title: "📥 Import trivia", step: 1 },
  lines: { title: "✍️ Bix's lines", what: "questions without a lead-in or reveal line", gpu: false, step: 2 },
  voice: { title: "🎙️ Bix's voice", what: "voice lines waiting", gpu: true, step: 3 },
  art: { title: "🎨 Art", what: "pieces of art the TV doesn't have yet", gpu: true, step: 4 },
  design: { title: "🎭 Design Bix's voice", gpu: true },
  cleanup: { title: "🧹 Destroy leftover GPUs" },
};
const OPENTDB = [[9, "General Knowledge"], [10, "Books"], [11, "Film"], [12, "Music"], [13, "Musicals & Theatres"],
  [14, "Television"], [15, "Video Games"], [16, "Board Games"], [17, "Science & Nature"], [18, "Computers"],
  [19, "Mathematics"], [20, "Mythology"], [21, "Sports"], [22, "Geography"], [23, "History"], [24, "Politics"],
  [25, "Art"], [26, "Celebrities"], [27, "Animals"], [28, "Vehicles"], [29, "Comics"], [30, "Gadgets"],
  [31, "Anime & Manga"], [32, "Cartoons & Animations"]];
const TRIVIA_API = [["music", "Music"], ["film_and_tv", "Film & TV"], ["sport_and_leisure", "Sport & Leisure"],
  ["history", "History"], ["geography", "Geography"], ["science", "Science"], ["food_and_drink", "Food & Drink"],
  ["society_and_culture", "Society & Culture"], ["arts_and_literature", "Arts & Literature"],
  ["general_knowledge", "General Knowledge"]];
const SOURCES = { opentdb: "Open Trivia DB", "trivia-api": "The Trivia API", upload: "Uploaded pack" };
const STATES = { running: "Running", done: "Done", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted by a restart" };

let status = null;
/** kind -> the last dry run's answer. */
const quotes = {};
/** kind -> the options picked on its card. */
const picked = {
  import: { source: "opentdb", amount: 20, category: "any", difficulty: "any" },
  voice: { max_hours: 3 }, art: { kinds: "all" }, lines: { limit: 50, missing: "either" },
  design: { description: null, text: null, count: 4 },
};
/** The candidate voices in the asset store, and the voice in use. */
let voices = { voices: [], bix_voice: null };
let pipeline = { batches: [], questions: null };
let checking = null;
/** The job shown in the panel, and how much of its log is on screen. */
let shown = null;
let logSeq = 0;
let logLines = [];
let timer = null;

async function api(path, body, method = "POST") {
  const res = await fetch(`/api/admin/${path}`, body === undefined ? {} : {
    method,
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
    [status, pipeline, voices] = await Promise.all([api(`render${refresh ? "?refresh=true" : ""}`), api("pipeline"), api("voices")]);
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
  // A finished import shows up in the game a moment later, once the server has reloaded the packs.
  const loading = pipeline && pipeline.batches.some((b) => nextStep(b) === "Waiting to load" && Date.now() - b.started_at < 120000);
  timer = setTimeout(tick, running || loading ? 2000 : 15000);
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
  if (!r.vast_key) todo.push(["Voice and art rent GPUs on vast.ai: add your vast.ai API key in ", el("a", { href: "/admin/settings" }, "Settings"), "."]);
  if (!r.anthropic_key) todo.push(["Host lines are written by Claude: add your Anthropic API key in ", el("a", { href: "/admin/settings" }, "Settings"), ". Calls are billed to that Anthropic account."]);
  const box = $("#setup");
  box.classList.toggle("hidden", !todo.length);
  box.replaceChildren(el("b", {}, "To finish setting up"), el("ul", {}, todo.map((t) => el("li", {}, ...t))));
}

// ---------- Job cards ----------

function canRun(kind) {
  const r = status.ready;
  if (kind === "import") return r.line_writer;
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
  if (kind === "import") {
    const cats = o.source === "opentdb" ? OPENTDB : TRIVIA_API;
    return [
      el("label", { class: "inline" }, "From",
        select([["opentdb", "Open Trivia DB"], ["trivia-api", "The Trivia API"]], o.source, (v) => {
          o.source = v;
          o.category = "any";
          renderCards();
        })),
      el("label", { class: "inline" }, "How many",
        select([[10, "10"], [20, "20"], [50, "50"]], o.amount, set("amount", Number))),
      el("label", { class: "inline" }, "Category", select([["any", "Any"], ...cats], o.category, set("category"))),
      el("label", { class: "inline" }, "Difficulty",
        select([["any", "Any"], ["easy", "Easy"], ["medium", "Medium"], ["hard", "Hard"]], o.difficulty, set("difficulty"))),
    ];
  }
  if (kind === "voice") {
    return el("label", { class: "inline" }, "GPU for at most",
      select([[0.5, "30 minutes"], [1, "1 hour"], [2, "2 hours"], [3, "3 hours"]], o.max_hours, set("max_hours", Number)));
  }
  if (kind === "art") {
    return el("label", { class: "inline" }, "Make",
      select([["all", "Everything missing"], ["backdrop", "Backdrops"], ["sticker", "Stickers"], ["category", "Category cards"], ["clip", "Clips"]],
        o.kinds, set("kinds")));
  }
  // Every question missing lines, or just the ones an earlier import brought in.
  const batches = pipeline.batches.filter((b) => b.state === "done" && b.in_game > b.with_lines);
  if (o.batch && !batches.some((b) => b.id === o.batch)) delete o.batch;
  return [
    el("label", { class: "inline" }, "For",
      select([["all", "Every question missing lines"],
        ...batches.map((b) => [b.id, `The import of ${time(b.started_at)} (${b.in_game - b.with_lines} need lines)`])],
      o.batch || "all", (v) => { o.batch = v === "all" ? undefined : Number(v); delete quotes[kind]; renderCards(); })),
    el("label", { class: "inline" }, "Up to",
      select([[10, "10"], [25, "25"], [50, "50"], [100, "100"], [200, "200"], [500, "500"]], o.limit, set("limit", Number)), "questions"),
    el("label", { class: "inline" }, "Write",
      select([["either", "Any missing"], ["lead-in", "Lead-ins"], ["reveal", "Reveal lines"]], o.missing, set("missing"))),
  ];
}

/** How long a check is good for: the server forgets it after 15 minutes. */
const QUOTE_TTL = 14 * 60 * 1000;

function quoteBox(kind) {
  if (checking === kind) return el("div", { class: "quote" }, "Checking… (this rents nothing)");
  if (quotes[kind] && Date.now() - quotes[kind].at > QUOTE_TTL) delete quotes[kind];
  const q = quotes[kind];
  if (!q) return null;
  const again = el("div", { class: "row" },
    el("button", { class: "secondary small", onclick: () => check(kind) }, "Check again"));
  if (!q.ok) {
    return el("div", { class: "quote" },
      el("div", { class: "error" }, "Can't start: ", q.error || "the dry run failed"),
      q.log.length ? el("details", {}, el("summary", {}, "What it said"), el("pre", { class: "log" }, q.log.join("\n"))) : null,
      again);
  }
  if (q.nothing) {
    return el("div", { class: "quote" },
      kind === "lines" ? "Every question here already has lines." : "Nothing to make, so nothing would be rented.", again);
  }
  const d = q.details || {};
  const lines = [];
  if (kind === "voice") lines.push(`${d.backlog} lines to voice`);
  if (kind === "art" && d.todo) lines.push(`${d.todo.count} pieces need a GPU${d.todo.clips ? ` (${d.todo.clips} clips)` : ""}, about ${d.todo.minutes} minutes`);
  if (kind === "design" && d.todo) {
    const reused = d.todo.total - d.todo.count;
    lines.push(`${plural(d.todo.count, "new candidate")} to make` + (reused ? `; ${reused} already made, so free` : ""));
  }
  const said = kind === "design" && q.options.lines
    ? el("ol", { class: "said" }, q.options.lines.map((l) => el("li", {}, `“${l}”`)))
    : null;
  if (kind === "lines" && d.todo) lines.push(`Lines for ${d.todo.count} of ${d.todo.total} questions, about ${Math.ceil(d.todo.count / 10)} ${d.todo.count > 10 ? "calls" : "call"} to Claude`);
  const head = q.gpu
    ? el("div", { class: "big" }, `${q.gpu} · ${money(q.price_hr)}/hr`)
    : el("div", { class: "big" }, "Billed to your Anthropic account");
  return el("div", { class: "quote" },
    head,
    ...lines.map((l) => el("div", {}, l)),
    said,
    q.max_cost != null ? el("div", {}, `Stops after ${duration(q.minutes * 60000)} at most, so it can cost at most `, el("b", {}, money(q.max_cost)), ".") : null,
    el("div", { class: "row" },
      el("button", { onclick: () => start(kind) }, q.max_cost != null ? `Start, up to ${money(q.max_cost)}` : "Start"),
      el("button", { class: "secondary", onclick: () => { delete quotes[kind]; renderCards(); } }, "Never mind")));
}

function card(kind) {
  const k = KINDS[kind];
  const running = status.job && status.job.state === "running";
  if (kind === "import") {
    const go = el("button", { onclick: importNow }, "Import");
    go.disabled = running || !canRun(kind);
    const file = el("input", { type: "file", accept: ".toml,text/plain", class: "hidden", onchange: (e) => uploadPack(e.target) });
    return el("div", { class: "card" },
      el("h2", {}, el("span", { class: "step" }, k.step), k.title),
      el("div", {}, el("span", { class: "count" }, pipeline.questions == null ? "?" : pipeline.questions.toLocaleString()), " ",
        el("span", { class: "what" }, "questions in the game. Duplicates are skipped.")),
      el("div", { class: "opts" }, options(kind)),
      el("div", { class: "row" }, go,
        el("button", { class: "secondary", onclick: () => file.click() }, "Upload a pack…"), file));
  }
  const n = missingCount(kind);
  const btn = el("button", { class: "secondary", onclick: () => check(kind) }, k.gpu ? "Check the price" : "Check what it'd write");
  btn.disabled = running || !!checking || !canRun(kind);
  return el("div", { class: "card" },
    el("h2", {}, el("span", { class: "step" }, k.step), k.title),
    el("div", {}, el("span", { class: "count" }, n == null ? "?" : n.toLocaleString()), " ", el("span", { class: "what" }, k.what)),
    el("div", { class: "opts" }, options(kind)),
    quoteBox(kind) || btn);
}

function renderCards() {
  $("#cards").replaceChildren(...["import", "lines", "voice", "art"].map(card));
  renderDesign();
}

// ---------- Designing Bix's voice ----------

function textBox(key, rows) {
  const o = picked.design;
  if (o[key] == null) o[key] = key === "description" ? status.ready.voice_description : status.ready.voice_samples.join("\n");
  const t = el("textarea", { rows: String(rows), oninput: (e) => {
    o[key] = e.target.value;
    if (quotes.design) { delete quotes.design; renderDesign(); }
  } });
  t.value = o[key];
  return t;
}

let designForm = null;

function renderDesign() {
  const o = picked.design;
  // The form is made once, so a refresh doesn't take the cursor out of a box being typed in.
  if (!designForm) {
    const count = select([1, 2, 3, 4, 6, 8].map((n) => [n, String(n)]), o.count, (v) => { o.count = Number(v); delete quotes.design; renderDesign(); });
    designForm = el("div", { class: "design-form" },
      el("label", {}, "What Bix sounds like", textBox("description", 4)),
      el("label", {}, "Lines he might say, one per line. Each candidate says a different one, picked at random, and his voice is copied from that clip (5 to 15 seconds each)", textBox("text", 8)),
      el("div", { class: "opts" }, el("label", { class: "inline" }, "Make", count, "candidates"),
        el("span", { class: "muted small" }, "The same description and line always make the same candidates, so each is only paid for once.")),
      el("div", { id: "design-action" }));
    $("#design").replaceChildren(designForm, el("div", { id: "design-voices" }));
  }
  const running = status.job && status.job.state === "running";
  const btn = el("button", { class: "secondary", onclick: () => check("design") }, "Check the price");
  btn.disabled = running || !!checking || !canRun("design");
  $("#design-action").replaceChildren(quoteBox("design") || btn);
  const spec = voices.bix_voice || "ip:US_James";
  const inUse = voices.voices.some((v) => v.current);
  $("#design-voices").replaceChildren(
    el("div", { class: "meta" }, "Bix speaks as ", el("b", {}, inUse ? "the voice marked below" : spec),
      " on rented GPUs. ", el("a", { href: "/admin/settings" }, "Settings"), " has the built-in voices."),
    voices.voices.length
      ? el("div", { class: "voices" }, voices.voices.map(voiceCard))
      : el("p", { class: "muted" }, "No candidates yet. Describe the voice above, check the price and start."));
}

function voiceCard(v) {
  const use = v.current
    ? el("span", { class: "badge done" }, "✓ Bix's voice")
    : el("button", { class: "small", onclick: () => useVoice(v) }, "Use this voice");
  return el("div", { class: `voice${v.current ? " current" : ""}` },
    el("audio", { controls: "", preload: "none", src: `/api/admin/voices/${v.key}/clip` }),
    el("div", { class: "row" }, el("b", {}, `Take ${v.seed ?? "?"}`), el("span", { style: "flex:1" }), use),
    v.text ? el("div", { class: "said" }, `“${v.text}”`) : null,
    el("div", { class: "muted small", title: v.description || "" }, v.description || ""));
}

async function useVoice(v) {
  if (!confirm("Make this Bix's voice? Lines already made keep playing in the old voice until the next voice run makes them again in this one.")) return;
  try {
    voices = await api(`voices/${v.key}/use`, {});
    toast("Bix has a new voice. Run step 3 to make his lines in it.", true);
    await load(true);
  } catch (e) {
    toast(e.message);
  }
}

// ---------- Importing ----------

async function importNow() {
  try {
    const job = await api("render/import", picked.import);
    shown = null;
    follow(job);
    await load();
  } catch (e) {
    toast(e.message);
  }
}

async function uploadPack(input) {
  const f = input.files[0];
  input.value = "";
  if (!f) return;
  const name = prompt("Name for this pack (saved as content/packs/mine/<name>.toml):", f.name.replace(/\.toml$/i, ""));
  if (!name) return;
  try {
    const res = await api("pipeline/pack", { name, toml: await f.text() });
    toast(res.message + (res.warnings ? ` (${res.warnings} warnings in the question bank)` : ""), true);
    await load();
  } catch (e) {
    toast(e.message);
  }
}

// ---------- Batches ----------

const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

function stage(done, total) {
  if (!total) return el("span", { class: "muted" }, "–");
  const pct = Math.round((done / total) * 100);
  return el("div", { class: "stage", title: `${done} of ${total}` },
    el("div", { class: "bar" }, el("div", { style: `width:${pct}%` + (done === total ? "" : ";background:var(--accent)") })),
    el("span", {}, done === total ? "✓ all" : `${done}/${total}`));
}

function nextStep(b) {
  if (b.state === "running") return "Importing…";
  if (b.state !== "done") return STATES[b.state] || b.state;
  if (!b.in_game) return b.imported && b.imported.added === 0 ? "Nothing new" : "Waiting to load";
  if (b.with_lines < b.in_game) return "Next: write lines (2)";
  if (b.voiced < b.in_game) return "Next: voice (3)";
  return "✓ Ready to play";
}

/** Step 2 for just this import: picks it on the lines card and checks what would be written. */
function linesFor(b) {
  picked.lines.batch = b.id;
  delete quotes.lines;
  renderCards();
  $("#cards").scrollIntoView({ behavior: "smooth" });
  check("lines");
}

function renderBatches() {
  const rows = pipeline.batches.map((b) => {
    const i = b.imported || {};
    const src = SOURCES[(b.options && b.options.source) || i.source] || "Import";
    const got = i.fetched != null
      ? `${i.added} new` + (i.skipped ? `, ${plural(i.skipped, "duplicate")} skipped` : "") + (i.flagged ? `, ${i.flagged} to check` : "")
      : "–";
    return el("tr", { class: "pick", title: "Show the log", onclick: () => openJob(b.id) },
      el("td", {}, time(b.started_at)),
      el("td", {}, src, i.pack ? el("div", { class: "muted small" }, i.pack) : null),
      el("td", {}, got),
      el("td", {}, stage(b.with_lines, b.in_game)),
      el("td", {}, stage(b.voiced, b.in_game)),
      el("td", {}, nextStep(b) === "Next: write lines (2)" && canRun("lines")
        ? el("button", { class: "small", onclick: (e) => { e.stopPropagation(); linesFor(b); } }, "Write its lines")
        : nextStep(b)));
  });
  $("#batches").replaceChildren(...(rows.length ? rows : [el("tr", {}, el("td", { colspan: 6, class: "muted" }, "No imports yet. Start with step 1."))]));
}

async function check(kind) {
  checking = kind;
  renderCards();
  try {
    quotes[kind] = { ...await api("render/preview", { kind, options: picked[kind] }), at: Date.now() };
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
  renderCards();
  try {
    const job = await api("render/start", { token: q.token, confirm: true });
    shown = null;
    follow(job);
    await load();
  } catch (e) {
    // Most often the check expired; show the button to check again.
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
  if (job.kind === "design" && d.todo) {
    const made = (d.made || []).length;
    return [d.todo.count ? made / d.todo.count : null, `${made} of ${plural(d.todo.count, "candidate")} made`];
  }
  if (job.kind === "import" && d.imported) {
    const i = d.imported;
    return [null, plural(i.added, "new question") + (i.skipped ? `, ${plural(i.skipped, "duplicate")} skipped` : "") + (i.pack ? ` → ${i.pack}` : "")];
  }
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
    if (j.kind === "design") what.push(plural((d.made || []).length, "voice"));
    if (j.kind === "import" && d.imported) what.push(plural(d.imported.added, "new question"));
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
  renderBatches();
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
  show("pipeline");
  await load();
})();
