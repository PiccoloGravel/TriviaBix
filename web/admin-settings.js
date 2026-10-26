// Keys, addresses and models for the services TriviaBix uses. Saved values live in the database and
// win over the container's environment variables. Secrets are write-only: the server never sends them.

const { $, el, show } = TBX;

/** Group name -> the service its Test button checks. */
const TESTS = {
  "GPU rental (vast.ai)": "vast",
  "Hugging Face": "huggingface",
  "Claude (Bix's lines)": "anthropic",
  "Bix's voice": "voice",
  "Music library (Lidarr)": "lidarr",
  "Private images (GitHub)": "github",
};
const SOURCES = { saved: "Saved here", env: "From the container", unset: "Not set" };

let fields = [];
/** Group -> the last test's answer. */
const results = {};

async function api(path, body, method = "POST") {
  const res = await fetch(`/api/admin/${path}`, body === undefined ? {} : {
    method,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
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
  toast.timer = setTimeout(() => t.classList.add("hidden"), 4000);
}

async function save(f, value) {
  try {
    fields = (await api("settings", { key: f.key, value }, "PUT")).fields;
    toast(value == null ? `${f.label}: back to the container's setting` : `${f.label} saved`, true);
    delete results[f.group];
    render();
  } catch (e) {
    toast(e.message);
  }
}

function input(f) {
  const k = f.kind;
  const def = f.default ? `Default: ${f.default}` : "";
  if (k.type === "choice") {
    const s = el("select", { id: f.key },
      el("option", { value: "" }, f.default ? `Default (${f.default})` : "Not set"),
      k.options.map((o) => el("option", { value: o }, o)));
    s.value = f.source === "unset" ? "" : (f.value || "");
    return s;
  }
  if (k.type === "secret") {
    return el("input", { id: f.key, type: "password", autocomplete: "off", spellcheck: "false",
      placeholder: f.source === "unset" ? "Paste the key" : "•••••••• set (paste a new one to replace it)" });
  }
  if (k.type === "long_text") {
    // Starts from the default, so it can be edited rather than written from scratch.
    const t = el("textarea", { id: f.key, rows: "7", maxlength: String(k.max), spellcheck: "true" });
    t.value = f.value || f.default || "";
    return t;
  }
  if (k.type === "price") {
    return el("input", { id: f.key, type: "number", min: "0.05", max: String(k.max), step: "0.05",
      value: f.value || "", placeholder: f.default || "" });
  }
  return el("input", { id: f.key, type: k.type === "url" ? "url" : "text", spellcheck: "false",
    value: f.value || "", placeholder: def || (k.type === "url" ? "http://…" : "") });
}

function field(f) {
  const box = input(f);
  const value = () => box.value.trim();
  const go = el("button", { class: "small", onclick: () => {
    const v = value();
    if (!v && f.kind.type !== "choice") return toast("Type a value first, or use the container's.");
    save(f, v || null);
  } }, "Save");
  const long = f.kind.type === "long_text";
  box.addEventListener("keydown", (e) => { if (e.key === "Enter" && (!long || e.ctrlKey || e.metaKey)) go.click(); });
  if (f.kind.type === "choice") box.addEventListener("change", () => go.click());
  const reset = f.source === "saved"
    ? el("button", { class: "secondary small", onclick: () => save(f, null) }, long && f.default ? "Back to the default" : "Use container's")
    : null;
  return el("div", { class: "field" },
    el("div", { class: "field-head" },
      el("label", { for: f.key }, f.label),
      el("span", { class: `badge ${f.source}` }, f.source === "unset" && f.default ? "Default" : SOURCES[f.source])),
    el("div", { class: `row${long ? " long" : ""}` }, box, f.kind.type === "choice" ? null : go, reset),
    el("div", { class: "help" }, f.help));
}

async function test(group, btn) {
  btn.disabled = true;
  btn.textContent = "Testing…";
  try {
    results[group] = await api("settings/test", { service: TESTS[group] });
  } catch (e) {
    results[group] = { ok: false, error: e.message };
  }
  render();
}

function render() {
  const groups = [];
  for (const f of fields) {
    let g = groups.find((g) => g.name === f.group);
    if (!g) groups.push(g = { name: f.group, fields: [] });
    g.fields.push(f);
  }
  $("#groups").replaceChildren(...groups.map((g) => {
    const r = results[g.name];
    const btn = TESTS[g.name] ? el("button", { class: "secondary small" }, "Test") : null;
    if (btn) btn.onclick = () => test(g.name, btn);
    return el("div", { class: "group" },
      el("div", { class: "group-head" }, el("h2", {}, g.name), btn),
      r ? el("div", { class: `result ${r.ok ? "ok" : "bad"}` }, r.ok ? `✓ ${r.message}` : `✗ ${r.error}`) : null,
      g.fields.map(field));
  }));
}

(async function boot() {
  let session = { logged_in: false };
  try { session = await (await fetch("/api/admin/session")).json(); } catch { /* treat as logged out */ }
  if (!session.logged_in) {
    show("login");
    return;
  }
  show("settings");
  try {
    fields = (await api("settings")).fields;
    render();
  } catch (e) {
    toast(e.message);
  }
})();
