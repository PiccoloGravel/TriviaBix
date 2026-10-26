// The menu across the top of every admin page. Add a page here and it shows up everywhere.

(function () {
  const PAGES = [
    ["/admin", "🎮 Rooms"],
    ["/admin/questions", "💬 Bix's lines"],
    ["/admin/duplicates", "🔁 Duplicates"],
    ["/admin/render", "🎨 Rendering"],
  ];
  const here = location.pathname.replace(/\/$/, "") || "/admin";

  const style = document.createElement("style");
  style.textContent = `
    nav.admin-nav { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 6px; margin: 0 0 16px;
      padding: 6px; background: var(--panel); border-radius: 24px; }
    nav.admin-nav a { flex: none; padding: 7px 14px; border-radius: 999px; color: var(--text);
      text-decoration: none; font-size: 15px; white-space: nowrap; }
    nav.admin-nav a:hover { background: rgba(255,255,255,.08); }
    nav.admin-nav a.on { background: var(--accent); color: #1a0b3d; }
    nav.admin-nav .gap { flex: 1; }
    nav.admin-nav button { flex: none; padding: 6px 12px; font-size: 14px; box-shadow: none; }
  `;
  document.head.append(style);

  const nav = TBX.el("nav", { class: "admin-nav", "aria-label": "Admin pages" },
    PAGES.map(([href, label]) => TBX.el("a", { href, class: href === here ? "on" : "",
      ...(href === here ? { "aria-current": "page" } : {}) }, label)),
    TBX.el("span", { class: "gap" }),
    TBX.el("button", { class: "secondary", type: "button", onclick: async () => {
      await fetch("/api/admin/logout", { method: "POST" }).catch(() => {});
      location.href = "/admin";
    } }, "Log out"));

  // Above the page's own header, so the login screens stay plain.
  const header = document.querySelector("section[data-screen] > header");
  if (header) header.before(nav);
})();
