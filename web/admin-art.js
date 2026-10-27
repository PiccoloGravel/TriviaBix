// The Art page: art themes, category styles, and every piece of art with its prompt, tags and takes.
// What's changed here is saved in appdata (server/src/art.rs). Anything that needs a GPU goes through
// the same price check and confirm as the Pipeline, and its log is followed there.

const { $, el, show } = TBX;

const KIND_NAMES = { backdrop: "Backdrop", sticker: "Sticker", category: "Category card", clip: "Clip" };
const STATUS = { made: "Made", stale: "Changed, not made yet", missing: "Not made yet" };
const RATINGS = ["PG", "PG-13", "MA", "NC-17"];

/** The server's listing for the theme on screen. */
let data = null;
/** The theme on screen (not necessarily the one on the TV). */
let view = null;
/** The open editor: a piece's key, "new" for a new piece, or a theme id for the theme form. */
let editing = null;
let themeForm = null;
let quote = null;
let checking = false;
const filters = { kind: "all", tag: "all", status: "all", text: "" };

async function api(path, method = "GET", body) {
  const res = await fetch(`/api/admin/${path}`, method === "GET" ? {} : {
    method,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body ?? {}),
  });
  const out = await res.json().catch(() => ({}));
  if (res.status === 401) show("login");
  if (!res.ok) throw new Error(out.error || `Request failed (${res.status})`);
  return out;
}

function toast(text, ok = false) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.toggle("ok", ok);
  t.classList.remove("hidden");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 4500);
}

const money = (n) => n == null ? "–" : `$${n < 0.1 && n > 0 ? n.toFixed(3) : n.toFixed(2)}`;

function duration(ms) {
  const m = Math.max(1, Math.round(ms / 60000));
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)}h ${m % 60}m`;
}

function select(options, value, onchange) {
  const s = el("select", { onchange: (e) => onchange(e.target.value) },
    options.map(([v, label]) => el("option", { value: v }, label)));
  s.value = value;
  return s;
}

function themeName(id) {
  const t = data && data.themes.find((x) => x.id === id);
  return t ? t.name : id;
}

async function load(theme) {
  try {
    const q = theme ? `?theme=${encodeURIComponent(theme)}` : "";
    got(await api(`art${q}`));
    show("art");
  } catch (e) {
    if (!$("#login").classList.contains("hidden")) return;
    show("art");
    toast(e.message);
  }
}

/** Takes the server's answer to a listing or a change, and redraws. */
function got(d) {
  if (data && d.theme !== data.theme) quote = null;
  data = d;
  view = d.theme;
  if (d.needs_gpu) toast(`Saved. ${d.needs_gpu} ${d.needs_gpu === 1 ? "piece needs" : "pieces need"} a GPU run in ${themeName(view)}.`, true);
  render();
}

async function change(path, body, done) {
  try {
    got(await api(path, "POST", { view, ...body }));
    if (done && !data.needs_gpu) toast(done, true);
    return true;
  } catch (e) {
    toast(e.message);
    return false;
  }
}

function render() {
  $("#stat-active").textContent = themeName(data.active);
  $("#stat-view").textContent = themeName(view);
  $("#busy").classList.toggle("hidden", !data.busy);
  if (data.busy) $("#busy").replaceChildren(el("div", {}, "⏳ ", data.busy, " Changes are saved, and the TV's files catch up when it's done. ",
    el("a", { href: "/admin/pipeline" }, "Follow it on the Pipeline")));
  renderThemes();
  renderMake();
  renderCats();
  renderFilters();
  renderPieces();
}

// ---------- Themes ----------

function renderThemes() {
  $("#themes").replaceChildren(...data.themes.map((t) => {
    const pct = t.total ? Math.round((100 * t.made) / t.total) : 0;
    const onTv = t.id === data.active;
    return el("div", { class: `card${t.id === view ? " on" : ""}` },
      el("div", { class: "row" }, el("h3", { style: "flex:1" }, t.name),
        onTv ? el("span", { class: "badge made" }, "On the TV") : null,
        t.builtin ? null : el("span", { class: "badge custom" }, "Yours")),
      el("p", {}, t.description || t.prompt),
      t.palette ? el("p", {}, "🎨 ", t.palette) : null,
      el("div", { class: "bar" }, el("i", { style: `width:${pct}%` })),
      el("p", {}, `${t.made} of ${t.total} pieces made`),
      el("div", { class: "row" },
        t.id === view ? null : el("button", { class: "small secondary", onclick: () => load(t.id) }, "Show its art"),
        onTv ? null : el("button", { class: "small", onclick: () => useTheme(t) }, "Put on the TV"),
        t.builtin ? null : el("button", { class: "small secondary", onclick: () => openThemeForm(t, false) }, "Edit"),
        el("button", { class: "small secondary", onclick: () => openThemeForm(t, true) }, "Copy"),
        t.builtin || onTv ? null : el("button", { class: "small danger", onclick: () => deleteTheme(t) }, "Delete")));
  }), el("div", { class: "card" },
    el("h3", {}, "➕ A new theme"),
    el("p", {}, "Describe a look (pixel art, watercolour, spooky Halloween…) and the whole catalog can be drawn in it."),
    el("div", { class: "row" }, el("button", { class: "small", onclick: () => openThemeForm(null, true) }, "Make a theme"))));
  renderThemeForm();
}

function openThemeForm(t, copy) {
  const base = t || { id: "", name: "", description: "", prompt: "", palette: "", sticker: "", backdrop: "" };
  themeForm = {
    editing: !copy,
    id: copy ? "" : base.id,
    name: copy && t ? `${base.name} (mine)` : base.name,
    description: base.description || "",
    prompt: base.prompt || "",
    palette: base.palette || "",
    sticker: copy ? "" : base.sticker || "",
    backdrop: copy ? "" : base.backdrop || "",
  };
  renderThemeForm();
  $("#theme-form").scrollIntoView({ behavior: "smooth", block: "nearest" });
}

function slugify(s) {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40);
}

function renderThemeForm() {
  const f = themeForm;
  if (!f) return $("#theme-form").replaceChildren();
  const field = (key, label, area, hint) => {
    const input = area
      ? el("textarea", { oninput: (e) => { f[key] = e.target.value; } }, f[key])
      : el("input", { type: "text", value: f[key], oninput: (e) => { f[key] = e.target.value; } });
    return el("label", {}, label, input, hint ? el("span", { class: "muted", style: "font-size:12px" }, hint) : null);
  };
  $("#theme-form").replaceChildren(el("div", { class: "section", style: "margin-top:14px" },
    el("h3", { style: "margin:0" }, f.editing ? `Edit ${f.name}` : "New theme"),
    field("name", "Name"),
    field("description", "What it looks like, in a few words (shown here only)"),
    field("prompt", "The look, as the art model should hear it", true,
      "Goes in front of every piece's own prompt. Say the medium and style, and end with \"no text\"."),
    field("palette", "Palette", false, "e.g. pumpkin orange, slime green and midnight purple"),
    el("details", {}, el("summary", {}, "More"),
      field("sticker", "Extra words for stickers", false, "Stickers already get a plain white background for their cut-out."),
      field("backdrop", "Words for backdrops and category cards, in place of the default", false,
        "The default keeps the middle empty for titles.")),
    f.editing ? el("p", { class: "muted", style: "margin:0;font-size:13px" },
      "Changing the look or palette starts a new version: new art gets made, and the old art stays in the asset store.") : null,
    el("div", { class: "row" },
      el("button", { onclick: saveTheme }, f.editing ? "Save" : "Make the theme"),
      el("button", { class: "secondary", onclick: () => { themeForm = null; renderThemeForm(); } }, "Never mind"))));
}

async function saveTheme() {
  const f = themeForm;
  const id = f.editing ? f.id : slugify(f.name);
  if (!id) return toast("Give the theme a name.");
  if (!f.editing && data.themes.some((t) => t.id === id)) return toast("There's already a theme with that name.");
  const body = { id, name: f.name, description: f.description, prompt: f.prompt, palette: f.palette,
    sticker: f.sticker, backdrop: f.backdrop };
  try {
    got(await api("art/themes", "POST", body));
    themeForm = null;
    renderThemeForm();
    toast(f.editing ? "Saved." : `Made ${f.name}. Its art is below; check the price to draw it.`, true);
  } catch (e) {
    toast(e.message);
  }
}

async function useTheme(t) {
  const missing = t.total - t.made;
  if (missing && !confirm(`${t.name} has ${t.made} of ${t.total} pieces made. Until the rest are made, the TV shows the current art for them. Put it on the TV?`)) return;
  try {
    got(await api(`art/themes/${t.id}/use`, "POST"));
    toast(`${t.name} is on the TV from the next scene.`, true);
  } catch (e) {
    toast(e.message);
  }
}

async function deleteTheme(t) {
  if (!confirm(`Delete the ${t.name} theme? Its art stays in the asset store, so making it again later is free.`)) return;
  try {
    got(await api("art/themes", "POST", { id: t.id, delete: true }));
  } catch (e) {
    toast(e.message);
  }
}

// ---------- Making art ----------

function renderMake() {
  const todo = data.items.filter((i) => i.status !== "made");
  $("#make-title").textContent = `Make the missing art in ${themeName(view)}`;
  const kids = [el("div", {}, todo.length
    ? `${todo.length} of ${data.items.length} pieces aren't made in this theme yet${todo.some((i) => i.status === "stale") ? " (some were changed)" : ""}.`
    : "Every piece is made in this theme.")];
  if (todo.length) kids.push(el("div", { class: "row" }, quoteBox(null)));
  // Every theme's missing pieces on one rented GPU, so the models download once.
  const others = data.themes.filter((t) => t.id !== view && t.made < t.total);
  if (others.length) {
    kids.push(el("div", {}, `${others.length} other ${others.length === 1 ? "theme is" : "themes are"} missing pieces too. Making them all in one run downloads the models once.`),
      el("div", { class: "row" }, quoteBox(ALL)));
  }
  $("#make").replaceChildren(...kids);
}

/** Stands for every theme in quoteBox and check. */
const ALL = "*every-theme*";

/** The price check for the whole theme (only = null), every theme (only = ALL) or one piece. */
function quoteBox(only) {
  const mine = quote && quote.only === only;
  if (checking === (only || "all")) return el("div", { class: "quote" }, "Checking… (this rents nothing)");
  if (!mine) {
    const b = el("button", { class: only ? "small secondary" : "secondary", onclick: () => check(only) },
      only === ALL ? "Check the price for every theme" : only ? "Check the price to make it" : "Check the price");
    b.disabled = !!data.busy || !!checking;
    return b;
  }
  const q = quote.q;
  const again = el("button", { class: "secondary small", onclick: () => check(only) }, "Check again");
  if (!q.ok) {
    return el("div", { class: "quote" }, el("div", { class: "error" }, "Can't start: ", q.error || "the dry run failed"),
      q.log && q.log.length ? el("details", {}, el("summary", {}, "What it said"), el("pre", { class: "prompt", style: "max-height:none;white-space:pre-wrap" }, q.log.join("\n"))) : null,
      el("div", { class: "row" }, again));
  }
  if (q.nothing) {
    return el("div", { class: "quote" }, "The asset store had it all, so it's made, for free. Nothing to rent.",
      el("div", { class: "row" }, again));
  }
  const d = q.details || {};
  return el("div", { class: "quote" },
    el("div", { class: "big" }, `${q.gpu} · ${money(q.price_hr)}/hr`),
    d.todo ? el("div", {}, `${d.todo.count} ${d.todo.count === 1 ? "piece needs" : "pieces need"} a GPU${d.todo.clips ? ` (${d.todo.clips} clips)` : ""}.`) : null,
    q.estimate ? el("div", {}, "Expected to take about ", el("b", {}, duration(q.estimate.minutes * 60000)),
      " and cost about ", el("b", {}, money(q.estimate.cost)), ". Most of a small run is the GPU downloading the models, so a few pieces at once cost little more than one.") : null,
    q.max_cost != null ? el("div", {}, `Stops after ${duration(q.minutes * 60000)} at most, so it can cost at most `, el("b", {}, money(q.max_cost)), ".") : null,
    el("div", { class: "row" },
      el("button", { onclick: () => start() }, q.max_cost != null ? `Start, up to ${money(q.max_cost)}` : "Start"),
      el("button", { class: "secondary", onclick: () => { quote = null; render(); } }, "Never mind")));
}

async function check(only) {
  checking = only || "all";
  quote = null;
  render();
  try {
    const options = only === ALL ? { theme: "all" } : { theme: view, ...(only ? { only } : {}) };
    const q = await api("render/preview", "POST", { kind: "art", options });
    quote = { only, q, at: Date.now() };
    // The check made whatever the asset store had into TV files, so the counts may have moved.
    if (q.nothing || q.ok) got(await api(`art?theme=${encodeURIComponent(view)}`));
  } catch (e) {
    toast(e.message);
  }
  checking = false;
  render();
}

async function start() {
  const q = quote.q;
  if (Date.now() - quote.at > 14 * 60 * 1000) {
    quote = null;
    render();
    return toast("That price is too old; check again.");
  }
  const expect = q.estimate ? ` Expected: about ${duration(q.estimate.minutes * 60000)}, about ${money(q.estimate.cost)}.` : "";
  if (!confirm(`Rent ${q.gpu} at ${money(q.price_hr)}/hr?${expect} It stops by itself after ${duration(q.minutes * 60000)}, so it can cost at most ${money(q.max_cost)}.`)) return;
  try {
    await api("render/start", "POST", { token: q.token, confirm: true });
    quote = null;
    toast("Started. Follow it on the Pipeline page.", true);
    await load(view);
  } catch (e) {
    toast(e.message);
  }
}

// ---------- Category styles ----------

function fileOf(key) {
  const item = data.items.find((i) => i.key === key);
  return item && item.file;
}

function thumb(file, kind) {
  const box = el("div", { class: `thumb${kind === "sticker" ? " sticker" : ""}` });
  if (!file) {
    box.textContent = "Not made yet";
  } else if (file.endsWith(".mp4")) {
    const v = el("video", { src: `/art-files/${file}`, loop: "", autoplay: "", playsinline: "" });
    v.muted = true;
    box.append(v);
  } else {
    box.append(el("img", { src: `/art-files/${file}`, alt: "", loading: "lazy" }));
  }
  return box;
}

function renderCats() {
  $("#cats").replaceChildren(...data.categories.map((c) => {
    const input = el("input", { type: "text", value: c.style, placeholder: "e.g. chalkboard doodles and test tubes" });
    const save = el("button", { class: "small", onclick: () => change("art/category-styles", { slug: c.slug, style: input.value },
      input.value ? `Saved ${c.name}'s style.` : `Cleared ${c.name}'s style.`) }, "Save");
    return el("div", { class: "cat" },
      thumb(fileOf(`category:${c.slug}`), "category"),
      el("div", {}, el("div", { class: "name" }, c.emoji, " ", c.name), input),
      el("div", { class: "row" }, save));
  }));
}

// ---------- Pieces ----------

function renderFilters() {
  const kinds = [["all", "Everything"], ...Object.entries(KIND_NAMES)];
  $("#f-kind").replaceWith(Object.assign(select(kinds, filters.kind, (v) => { filters.kind = v; renderPieces(); }), { id: "f-kind" }));
  const tags = [["all", "Any"], ...data.tags.filter((t) => !t.startsWith("category:")).map((t) => [t, t])];
  if (!data.tags.includes(filters.tag)) filters.tag = "all";
  $("#f-tag").replaceWith(Object.assign(select(tags, filters.tag, (v) => { filters.tag = v; renderPieces(); }), { id: "f-tag" }));
  const statuses = [["all", "All"], ["made", "Made"], ["todo", "Not made yet"], ["custom", "Added here"]];
  $("#f-status").replaceWith(Object.assign(select(statuses, filters.status, (v) => { filters.status = v; renderPieces(); }), { id: "f-status" }));
}

$("#f-text").addEventListener("input", (e) => { filters.text = e.target.value.toLowerCase(); renderPieces(); });
$("#add").addEventListener("click", () => { editing = "new"; renderNewPiece(); });

function shown(i) {
  if (filters.kind !== "all" && i.kind !== filters.kind) return false;
  if (filters.tag !== "all" && !i.tags.includes(filters.tag)) return false;
  if (filters.status === "made" && i.status !== "made") return false;
  if (filters.status === "todo" && i.status === "made") return false;
  if (filters.status === "custom" && !i.custom) return false;
  return !filters.text || i.id.includes(filters.text) || (i.body || "").toLowerCase().includes(filters.text);
}

function renderPieces() {
  const items = data.items.filter(shown);
  $("#pieces").replaceChildren(...(items.length ? items.map(piece) : [el("p", { class: "muted" }, "Nothing matches.")]));
  const hidden = data.hidden || [];
  $("#hidden-pieces").replaceChildren(...(hidden.length ? [el("h2", {}, "Hidden"),
    el("div", { class: "row" }, hidden.map((key) => {
      const [kind, id] = key.split(":");
      return el("button", { class: "small secondary", onclick: () => change("art/entries", { kind, id, hidden: false }, `${id} is back.`) },
        `${KIND_NAMES[kind] || kind} ${id} · show again`);
    }))] : []));
  renderNewPiece();
}

function piece(i) {
  const open = editing === i.key;
  return el("div", { class: "card" },
    thumb(i.file, i.kind),
    el("div", { class: "row" }, el("h3", { style: "flex:1;overflow-wrap:anywhere" }, i.id),
      el("span", { class: `badge ${i.status}` }, STATUS[i.status])),
    el("div", { class: "meta" },
      el("span", { class: "badge" }, KIND_NAMES[i.kind]),
      el("span", { class: "badge" }, i.rating),
      i.custom ? el("span", { class: "badge custom" }, "Added here") : null,
      ...i.tags.filter((t) => !t.startsWith("category")).map((t) => el("span", { class: "badge" }, t))),
    el("div", { class: "prompt", title: i.prompt }, i.body),
    open ? editor(i) : el("div", { class: "row" },
      el("button", { class: "small secondary", onclick: () => { editing = i.key; renderPieces(); } }, "Edit or new take"),
      i.status !== "made" ? quoteBox(i.key) : null));
}

function editor(i) {
  const f = { prompt: i.body, tags: i.tags.filter((t) => !t.startsWith("category")).join(", "), rating: i.rating, from: i.from };
  const tags = (s) => s.split(",").map((t) => t.trim().toLowerCase()).filter(Boolean);
  const backdrops = data.items.filter((x) => x.kind === "backdrop").map((x) => [x.id, x.id]);
  const isCard = i.kind === "category";
  const save = async () => {
    const body = { kind: i.kind, id: i.id, tags: tags(f.tags), rating: f.rating };
    if (f.prompt !== i.body) body.prompt = f.prompt;
    if (i.kind === "clip") body.from = f.from;
    if (await change("art/entries", body, "Saved.")) { editing = null; renderPieces(); }
  };
  const newTake = () => change("art/entries", { kind: i.kind, id: i.id, theme: view, seed: Math.floor(Math.random() * 2 ** 31) },
    "A new take is ready to make; check the price below.");
  const current = (t) => t.seed === i.seed && (t.prompt === i.prompt);
  const takes = (i.takes || []).filter((t) => t.original);
  return el("div", { class: "editor" },
    el("label", {}, isCard ? "What's on the card (the category's style is added after it)" : "What it shows",
      el("textarea", { oninput: (e) => { f.prompt = e.target.value; } }, f.prompt)),
    i.kind === "clip"
      ? el("label", {}, "Animates the backdrop", select(backdrops, f.from, (v) => { f.from = v; }))
      : null,
    el("label", {}, "Tags (scenes pick art by these: winner, nobody, roast, hype…)",
      el("input", { type: "text", value: f.tags, oninput: (e) => { f.tags = e.target.value; } })),
    el("label", { class: "inline" }, "Rating", select(RATINGS.map((r) => [r, r]), f.rating, (v) => { f.rating = v; })),
    el("p", { class: "muted", style: "margin:0;font-size:12px" },
      "Prompt, tags and rating apply in every theme. Changing the prompt means it's drawn again; tags and ratings are free."),
    el("div", { class: "row" },
      el("button", { class: "small", onclick: save }, "Save"),
      i.kind === "clip" ? null : el("button", { class: "small secondary", onclick: newTake, title: "Same prompt, a different seed, in this theme only" }, "🎲 New take"),
      el("button", { class: "small secondary", onclick: () => { editing = null; renderPieces(); } }, "Close")),
    takes.length > 1 || (takes.length === 1 && !current(takes[0])) ? el("div", {},
      el("div", { class: "muted", style: "font-size:13px;margin-bottom:4px" }, "Takes in this theme (kept in the asset store, so going back is free)"),
      el("div", { class: "takes" }, takes.map((t) => el("div", { class: `take${current(t) ? " on" : ""}` },
        el("img", { src: `/api/admin/art/original/${t.original}`, alt: "", loading: "lazy" }),
        current(t) ? "In use" : el("button", { class: "small secondary", onclick: () => useTake(i, t) }, "Use this"))))) : null,
    el("div", { class: "row" },
      i.custom
        ? el("button", { class: "small danger", onclick: () => removePiece(i) }, "Delete")
        : el("button", { class: "small secondary", onclick: () => resetPiece(i) }, "Undo my changes"),
      isCard || i.custom ? null : el("button", { class: "small secondary", onclick: () => hidePiece(i) }, "Hide from the game")));
}

async function useTake(i, t) {
  // A take's own prompt comes back too, if it was drawn from an earlier one.
  if (t.body && t.body !== i.body) {
    if (!await change("art/entries", { kind: i.kind, id: i.id, prompt: t.body })) return;
  }
  await change("art/entries", { kind: i.kind, id: i.id, theme: view, seed: t.seed }, "That take is back on, for free.");
}

async function resetPiece(i) {
  if (!confirm(`Put ${i.id} back as the catalog has it, in every theme? Takes stay in the asset store.`)) return;
  await change("art/entries", { kind: i.kind, id: i.id, reset: true });
  await change("art/entries", { kind: i.kind, id: i.id, theme: view, reset: true }, "Back to the catalog's.");
  editing = null;
  renderPieces();
}

async function removePiece(i) {
  if (!confirm(`Delete ${i.id}? Its art stays in the asset store.`)) return;
  if (await change("art/entries", { kind: i.kind, id: i.id, theme: view, reset: true })
    && await change("art/entries", { kind: i.kind, id: i.id, reset: true }, "Deleted.")) {
    editing = null;
    renderPieces();
  }
}

async function hidePiece(i) {
  if (!confirm(`Hide ${i.id}? Scenes stop using it in every theme; it can be shown again at the bottom of this page.`)) return;
  if (await change("art/entries", { kind: i.kind, id: i.id, hidden: true }, "Hidden.")) {
    editing = null;
    renderPieces();
  }
}

function renderNewPiece() {
  if (editing !== "new") return $("#new-piece").replaceChildren();
  const f = renderNewPiece.f || (renderNewPiece.f = { kind: "sticker", id: "", prompt: "", tags: "", rating: "PG", from: "" });
  const backdrops = data.items.filter((x) => x.kind === "backdrop").map((x) => [x.id, x.id]);
  if (!f.from && backdrops.length) f.from = backdrops[0][0];
  const kinds = [["sticker", "Sticker (a cut-out prop)"], ["backdrop", "Backdrop (a full-screen scene)"], ["clip", "Clip (a backdrop brought to life)"]];
  const add = async () => {
    const id = slugify(f.id || f.prompt.split(" ").slice(0, 4).join(" "));
    const body = { kind: f.kind, id, prompt: f.prompt, rating: f.rating,
      tags: f.tags.split(",").map((t) => t.trim().toLowerCase()).filter(Boolean) };
    if (f.kind === "clip") body.from = f.from;
    if (data.items.some((x) => x.key === `${f.kind}:${id}`)) return toast(`There's already a ${f.kind} called ${id}.`);
    if (await change("art/entries", body, `Added ${id}. Check the price on its card to draw it.`)) {
      editing = null;
      renderNewPiece.f = null;
      filters.status = "all";
      renderFilters();
      renderPieces();
    }
  };
  $("#new-piece").replaceChildren(el("div", { class: "section", style: "margin-bottom:14px" },
    el("h3", { style: "margin:0" }, "Add a piece"),
    el("label", { class: "inline" }, "Kind", select(kinds, f.kind, (v) => { f.kind = v; renderNewPiece(); })),
    f.kind === "clip" ? el("label", { class: "inline" }, "Animates", select(backdrops, f.from, (v) => { f.from = v; })) : null,
    el("label", {}, f.kind === "clip" ? "How it moves" : "What it shows (the theme's look is added for you)",
      el("textarea", { oninput: (e) => { f.prompt = e.target.value; } }, f.prompt)),
    el("label", {}, "Name (letters, digits and dashes; made from the prompt if empty)",
      el("input", { type: "text", value: f.id, oninput: (e) => { f.id = e.target.value; } })),
    el("label", {}, "Tags, comma separated: the moments it suits (winner, nobody, roast, streak, fastest…) and a feeling (hype, funny)",
      el("input", { type: "text", value: f.tags, oninput: (e) => { f.tags = e.target.value; } })),
    el("label", { class: "inline" }, "Rating", select(RATINGS.map((r) => [r, r]), f.rating, (v) => { f.rating = v; })),
    el("div", { class: "row" },
      el("button", { onclick: add }, "Add it"),
      el("button", { class: "secondary", onclick: () => { editing = null; renderNewPiece(); } }, "Never mind"))));
}

load();
