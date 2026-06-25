// agentstack dashboard — vanilla JS, no framework. Sidebar sections over a
// read-only snapshot (/api/state), with editing actions and diff-before-apply.
const token = new URLSearchParams(location.search).get("token") || "";
let DATA = null;
let SECTION = "overview";
let READONLY = false;
let OPEN_SERVER = null;
let ADD_FORM = false;

const SECTIONS = [
  { id: "overview", label: "Overview" },
  { id: "discover", label: "Discover" },
  { id: "servers", label: "Servers", count: (d) => d.servers.length },
  { id: "skills", label: "Skills", count: (d) => d.skills.length },
  { id: "instructions", label: "Instructions", count: (d) => d.instructions.length },
  { id: "secrets", label: "Secrets", count: (d) => d.secrets.length },
  { id: "health", label: "Health" },
];

function el(tag, attrs, children) {
  const e = document.createElement(tag);
  if (attrs) for (const k in attrs) {
    if (k === "class") e.className = attrs[k];
    else if (k === "html") e.innerHTML = attrs[k];
    else if (k.startsWith("on") && typeof attrs[k] === "function") e.addEventListener(k.slice(2), attrs[k]);
    else e.setAttribute(k, attrs[k]);
  }
  (children || []).forEach((c) => c != null && e.appendChild(typeof c === "string" ? document.createTextNode(c) : c));
  return e;
}
const badge = (t, kind) => el("span", { class: "badge " + (kind || "") }, [t]);
function btn(label, onClick, cls) {
  return el("button", { class: "btn " + (cls || ""), onclick: onClick }, [label]);
}
function toast(msg, ok) {
  const t = el("div", { class: "toast " + (ok ? "ok" : "err") }, [msg]);
  document.body.appendChild(t);
  setTimeout(() => t.remove(), 3400);
}
const q = (p) => p + "?token=" + encodeURIComponent(token);

function post(path, body, label) {
  return fetch(q(path), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body || {}) })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => {
      if (!ok || d.error) throw new Error(d.error || "request failed");
      toast((label || "Done") + " ✓", true);
      return load();
    })
    .catch((e) => toast((label || "Action") + ": " + e.message, false));
}

/* ---------- shell ---------- */
function renderNav() {
  const nav = document.getElementById("nav");
  nav.innerHTML = "";
  SECTIONS.forEach((s) => {
    const count = s.count ? s.count(DATA) : null;
    nav.appendChild(
      el("button", { class: "nav-item" + (s.id === SECTION ? " active" : ""), onclick: () => show(s.id) }, [
        el("span", null, [s.label]),
        count != null ? el("span", { class: "nav-count" }, [String(count)]) : null,
      ])
    );
  });
}
function show(id) {
  SECTION = id;
  OPEN_SERVER = null;
  renderNav();
  const c = document.getElementById("content");
  c.innerHTML = "";
  ({ overview, discover, servers, skills, instructions, secrets, health }[id] || overview)(c);
}

/* ---------- discover (browse providers → add) ---------- */
const DISCOVER = { q: "", results: null, loading: false };

function doDiscoverSearch(query) {
  DISCOVER.q = query;
  DISCOVER.loading = true;
  if (SECTION === "discover") show("discover");
  return fetch(q("/api/search") + "&q=" + encodeURIComponent(query))
    .then((r) => r.json())
    .then((d) => {
      DISCOVER.loading = false;
      DISCOVER.results = d.results || [];
      if (SECTION === "discover") show("discover");
    })
    .catch((e) => {
      DISCOVER.loading = false;
      toast("Search failed: " + e.message, false);
    });
}

function addFrom(id) {
  return fetch(q("/api/add_from"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ id }),
  })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => {
      if (!ok || d.error) throw new Error(d.error || "add failed");
      toast("Added ✓ — review secrets, then apply", true);
      return load().then(() => doDiscoverSearch(DISCOVER.q));
    })
    .catch((e) => toast("Add: " + e.message, false));
}

function trustBadges(t) {
  const out = [];
  if (t.namespaced) out.push(badge("verified namespace", "green"));
  if (t.runsCode) out.push(badge("runs code", "amber"));
  if (t.needsSecret) out.push(badge("needs secret", ""));
  return out;
}

function discover(c) {
  c.appendChild(pageHead("Discover", "Search the catalog + official MCP Registry, then add — it renders to all your CLIs on apply."));

  const input = el("input", { class: "inp", placeholder: "search capabilities…", style: "width:280px", value: DISCOVER.q });
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") doDiscoverSearch(input.value.trim()); });
  c.appendChild(el("div", { class: "toolbar", style: "margin-bottom:16px" }, [
    input,
    btn("Search", () => doDiscoverSearch(input.value.trim()), "primary"),
  ]));

  // Left: results. Right: your stack.
  const left = el("div", { class: "card" }, [el("div", { class: "hd" }, ["Results"]), el("div", { class: "bd" }, [resultsBody()])]);
  const stackRows = DATA.servers.map((s) =>
    el("div", { class: "list-row" }, [el("span", { class: "name" }, [s.name]), badge(s.type, "solid")])
  );
  if (!stackRows.length) stackRows.push(el("div", { class: "empty" }, ["Nothing added yet."]));
  const right = el("div", { class: "card" }, [el("div", { class: "hd" }, ["Your stack", el("small", null, [`${DATA.servers.length} server(s)`])]), el("div", { class: "bd" }, stackRows)]);

  c.appendChild(el("div", { class: "grid", style: "grid-template-columns: 1.5fr 1fr; align-items:start" }, [left, right]));
}

function resultsBody() {
  if (DISCOVER.loading) return el("div", { class: "empty" }, ["Searching…"]);
  if (DISCOVER.results == null) return el("div", { class: "empty" }, ["Type a query and search the catalog + official MCP Registry."]);
  if (!DISCOVER.results.length) return el("div", { class: "empty" }, [`No matches for "${DISCOVER.q}".`]);
  const wrap = el("div");
  DISCOVER.results.forEach((r) => {
    const head = el("div", { style: "display:flex;align-items:center;justify-content:space-between;gap:10px" }, [
      el("span", null, [el("span", { class: "name" }, [r.name]), el("span", { class: "k" }, [r.source])]),
      r.installed
        ? badge("in stack", "green")
        : READONLY
        ? null
        : btn("add ›", () => addFrom(r.addId), "primary"),
    ]);
    const meta = el("div", { class: "muted", style: "font-size:12px;margin:2px 0 6px" }, [r.description || r.id]);
    const trust = el("div", { class: "row-actions", style: "margin-bottom:6px" }, trustBadges(r.trust));
    wrap.appendChild(el("div", { style: "padding:10px 0;border-top:1px solid hsl(var(--border))" }, [head, meta, trust]));
  });
  return wrap;
}

function pageHead(title, sub) {
  return el("div", null, [el("h1", { class: "page-title" }, [title]), sub ? el("p", { class: "page-sub" }, [sub]) : null]);
}
function statCard(label, value, sub) {
  return el("div", { class: "card stat" }, [
    el("div", { class: "label" }, [label]),
    el("div", { class: "value" }, [String(value)]),
    el("div", { class: "sub" }, [sub || " "]),
  ]);
}

/* ---------- overview ---------- */
function overview(c) {
  const d = DATA;
  const installed = d.adapters.filter((a) => a.installed).length;
  const secretsOk = d.secrets.filter((s) => s.resolved).length;
  const errs = d.health.filter((h) => h.level === "error").length;
  const warns = d.health.filter((h) => h.level === "warn").length;
  c.appendChild(pageHead("Overview", d.meta.dir));
  c.appendChild(el("div", { class: "grid cols-4" }, [
    statCard("Harnesses", installed, `${d.adapters.length} known`),
    statCard("Servers", d.servers.length, "MCP"),
    statCard("Skills", d.skills.length, `${d.skills.filter((s) => s.installed).length} installed`),
    statCard("Secrets", `${secretsOk}/${d.secrets.length}`, "resolved"),
  ]));

  if (!READONLY) {
    c.appendChild(el("div", { class: "section-title" }, ["Actions"]));
    c.appendChild(el("div", { class: "toolbar" }, [
      btn("Preview & apply → global", () => openPreview("global"), "primary"),
      btn("Preview & apply → project", () => openPreview("project")),
      btn("Install skills", () => post("/api/install", {}, "Install")),
    ]));
  }

  c.appendChild(el("div", { class: "section-title" }, ["Health"]));
  const hsum = errs ? badge(`${errs} error(s)`, "red") : warns ? badge(`${warns} warning(s)`, "amber") : badge("all good", "green");
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [
    el("div", { style: "margin-bottom:8px" }, [hsum]),
    ...d.health.slice(0, 5).map(healthRow),
  ])]));

  c.appendChild(el("div", { class: "grid cols-2", style: "margin-top:18px" }, [profilesCard(), usageCard()]));
}

function profilesCard() {
  const rows = DATA.profiles.map((p) => {
    const meta = el("span", { class: "muted" }, [`${p.servers.length} server(s) · ${p.skills.length} skill(s)`]);
    return el("div", { class: "list-row" }, [
      el("span", { class: "name" }, [p.name]),
      READONLY ? meta : el("span", { class: "row-actions" }, [meta, btn("activate ›", () => post("/api/use", { profile: p.name, scope: "global" }, "Activate " + p.name))]),
    ]);
  });
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No profiles defined."]));
  return el("div", { class: "card" }, [el("div", { class: "hd" }, ["Profiles"]), el("div", { class: "bd" }, rows)]);
}
function usageCard() {
  const max = Math.max(1, ...DATA.stats.map((s) => s.activations));
  const rows = DATA.stats.slice(0, 6).map((s) =>
    el("div", { class: "list-row" }, [
      el("span", { class: "name" }, [s.name]),
      el("div", { class: "bar-track" }, [el("div", { class: "bar", style: `width:${Math.round((s.activations / max) * 100)}%` })]),
      el("span", { class: "muted" }, [String(s.activations)]),
    ])
  );
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No activations recorded yet."]));
  return el("div", { class: "card" }, [el("div", { class: "hd" }, ["Usage", el("small", null, ["activations"])]), el("div", { class: "bd" }, rows)]);
}

/* ---------- servers (matrix + detail) ---------- */
function toggleCell(serverName, target, currentlyOn) {
  return post("/api/toggle", { server: serverName, target, scope: "global", enable: !currentlyOn },
    (currentlyOn ? "Disabled " : "Enabled ") + serverName);
}

function saveServer() {
  const g = (id) => (document.getElementById(id) || {}).value || "";
  const transport = g("f-transport");
  const body = { name: g("f-name").trim(), transport };
  if (!body.name) return toast("Name is required", false);
  if (transport === "http") body.url = g("f-url").trim();
  else { body.command = g("f-command").trim(); body.args = g("f-args").trim().split(/\s+/).filter(Boolean); }
  const hdr = g("f-header").trim();
  if (hdr.includes("=")) { const [k, ...v] = hdr.split("="); body.headers = { [k.trim()]: v.join("=") }; }
  const env = g("f-env").trim();
  if (env.includes("=")) { const [k, ...v] = env.split("="); body.env = { [k.trim()]: v.join("=") }; }
  fetch(q("/api/add_server"), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => {
      if (!ok || d.error) throw new Error(d.error || "failed");
      ADD_FORM = false;
      toast("Server added — enable it per CLI below", true);
      load();
    })
    .catch((e) => toast("Add server: " + e.message, false));
}

function addServerCard() {
  const row = (label, node) => el("div", { style: "display:flex;align-items:center;gap:10px;margin-bottom:8px" }, [
    el("label", { class: "muted", style: "width:90px;font-size:12px" }, [label]), node,
  ]);
  const transport = el("select", { id: "f-transport", style: "height:32px", onchange: () => {
    document.getElementById("row-url").style.display = transport.value === "http" ? "" : "none";
    document.getElementById("row-cmd").style.display = transport.value === "stdio" ? "" : "none";
    document.getElementById("row-args").style.display = transport.value === "stdio" ? "" : "none";
  } }, [el("option", { value: "http" }, ["http"]), el("option", { value: "stdio" }, ["stdio"])]);
  return el("div", { class: "card", style: "margin-bottom:16px" }, [
    el("div", { class: "hd" }, ["Add MCP server", el("small", null, ["added to the manifest; enable it per CLI in the matrix"])]),
    el("div", { class: "bd" }, [
      row("name", el("input", { id: "f-name", class: "inp", placeholder: "e.g. kibana", style: "width:220px" })),
      row("transport", transport),
      el("div", { id: "row-url" }, [row("url", el("input", { id: "f-url", class: "inp", placeholder: "https://…/mcp", style: "width:300px" }))]),
      el("div", { id: "row-cmd", style: "display:none" }, [row("command", el("input", { id: "f-command", class: "inp", placeholder: "npx", style: "width:300px" }))]),
      el("div", { id: "row-args", style: "display:none" }, [row("args", el("input", { id: "f-args", class: "inp", placeholder: "-y @scope/server", style: "width:300px" }))]),
      row("header", el("input", { id: "f-header", class: "inp", placeholder: "Authorization=Bearer ${TOKEN}", style: "width:300px" })),
      row("env", el("input", { id: "f-env", class: "inp", placeholder: "API_KEY=${API_KEY}", style: "width:300px" })),
      el("div", { class: "toolbar", style: "margin-top:6px" }, [btn("Save", saveServer, "primary"), btn("Cancel", () => { ADD_FORM = false; show("servers"); })]),
    ]),
  ]);
}

function servers(c) {
  const d = DATA;
  c.appendChild(pageHead("Servers", "Click a cell to enable/disable a server for that CLI (global scope). Click a row name for its config."));
  if (!READONLY) {
    c.appendChild(el("div", { class: "toolbar", style: "margin-bottom:14px" }, [
      btn(ADD_FORM ? "Close" : "+ Add MCP server", () => { ADD_FORM = !ADD_FORM; show("servers"); }, "primary"),
    ]));
    if (ADD_FORM) c.appendChild(addServerCard());
  }

  const head = el("tr", null, [el("th", null, ["capability"])]);
  d.adapters.forEach((a) => head.appendChild(el("th", { class: "cell" }, [a.display])));
  head.appendChild(el("th", null, ["type"]));

  const body = el("tbody");
  if (!d.servers.length) body.appendChild(el("tr", null, [el("td", { colspan: d.adapters.length + 2 }, [el("span", { class: "empty" }, ["No servers yet. Use “+ Add MCP server” or the Discover tab."])])]));
  d.servers.forEach((s) => {
    const tr = el("tr", { class: "clickable" }, [
      el("td", { onclick: () => { OPEN_SERVER = OPEN_SERVER === s.name ? null : s.name; show("servers"); } },
        [el("span", { class: "name" }, [s.name]), el("span", { class: "k" }, ["mcp"])]),
    ]);
    d.adapters.forEach((a) => {
      const cell = s.cells.find((x) => x.adapter === a.id) || {};
      const tag = cell.global && cell.project ? "both" : cell.global ? "global" : cell.project ? "project" : "";
      const on = cell.global || cell.project;
      const inner = [el("div", { class: on ? "on" : "off" }, [on ? "✓" : "–"]), tag ? el("div", { class: "sc" }, [tag]) : null];
      const td = el("td", { class: "cell" }, inner);
      if (!READONLY) {
        td.style.cursor = "pointer";
        td.title = `${cell.global ? "disable" : "enable"} ${s.name} for ${a.display} (global)`;
        td.addEventListener("click", (e) => { e.stopPropagation(); toggleCell(s.name, a.id, !!cell.global); });
      }
      tr.appendChild(td);
    });
    tr.appendChild(el("td", null, [badge(s.type, "solid")]));
    body.appendChild(tr);
    if (OPEN_SERVER === s.name) body.appendChild(serverDetail(s, d.adapters.length + 2));
  });
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd", style: "padding:6px 8px" }, [el("table", null, [el("thead", null, [head]), body])])]));
}

function serverDetail(s, span) {
  const kv = [];
  const add = (k, v) => kv.push(el("div", { class: "key" }, [k]), el("div", { class: "mono" }, [v]));
  add("type", s.type);
  if (s.url) add("url", s.url);
  if (s.command) add("command", s.command);
  if (s.args && s.args.length) add("args", s.args.join(" "));
  (s.headers || []).forEach((h) => add("header." + h.key, h.value));
  (s.env || []).forEach((e) => add("env." + e.key, e.value));
  return el("tr", { class: "detail" }, [el("td", { colspan: span }, [el("div", { class: "bd" }, [el("div", { class: "kv" }, kv)])])]);
}

/* ---------- skills ---------- */
function skills(c) {
  c.appendChild(pageHead("Skills", "Sources, versions, and whether each is installed in the store."));
  const rows = DATA.skills.map((s) => {
    const detail = s.source === "git"
      ? `git · ${(s.src.git || "")}${s.lockedRev ? " @ " + s.lockedRev.slice(0, 8) : ""}`
      : `path · ${s.src.path || ""}`;
    return el("div", { class: "list-row" }, [
      el("span", null, [el("span", { class: "name" }, [s.name]), el("div", { class: "muted mono", style: "font-size:12px" }, [detail])]),
      s.installed ? badge("installed", "green") : badge("not installed", "amber"),
    ]);
  });
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No skills in the manifest."]));
  const body = [el("div", null, rows)];
  if (!READONLY && DATA.skills.some((s) => !s.installed)) {
    body.push(el("div", { style: "margin-top:14px" }, [btn("Install missing", () => post("/api/install", {}, "Install"), "primary")]));
  }
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, body)]));
}

/* ---------- instructions ---------- */
function instructions(c) {
  c.appendChild(pageHead("Instructions", "Fragments compiled into each harness's CLAUDE.md / AGENTS.md."));
  const rows = DATA.instructions.map((i) =>
    el("div", { class: "list-row" }, [
      el("span", null, [el("span", { class: "name" }, [i.name]), el("div", { class: "muted mono", style: "font-size:12px" }, [i.path])]),
      el("span", { class: "row-actions" }, [
        ...i.targets.map((t) => badge(t, "solid")),
        i.exists ? badge("found", "green") : badge("missing", "red"),
      ]),
    ])
  );
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No instruction fragments. Add [instructions.*] to the manifest."]));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, rows)]));
}

/* ---------- secrets ---------- */
function secrets(c) {
  c.appendChild(pageHead("Secrets", "Referenced ${REF}s and where they resolve on this machine. Values are never shown."));
  const rows = DATA.secrets.map((s) => {
    let right;
    if (s.resolved) right = badge("resolved · " + (s.source || "?"), "green");
    else if (READONLY) right = badge("missing", "red");
    else {
      const input = el("input", { type: "password", placeholder: "value", class: "inp" });
      right = el("span", { class: "setter" }, [input, btn("Set", () => input.value && post("/api/secret", { name: s.name, value: input.value }, "Set " + s.name))]);
    }
    return el("div", { class: "list-row" }, [el("span", { class: "mono" }, [s.name]), right]);
  });
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["The manifest references no secrets."]));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, rows)]));
}

/* ---------- health ---------- */
function healthRow(h) {
  const cls = h.level === "error" ? "dot-err" : h.level === "warn" ? "dot-warn" : "dot-ok";
  const mark = h.level === "error" ? "✗" : h.level === "warn" ? "⚠" : "✓";
  return el("div", { class: "health-row" }, [el("span", { class: cls }, [mark]), el("span", null, [h.message])]);
}
function health(c) {
  c.appendChild(pageHead("Health", "Adapters, secrets, and drift — the trust layer."));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, DATA.health.map(healthRow))]));
  if (!READONLY) {
    c.appendChild(el("div", { style: "margin-top:16px" }, [
      btn("Preview & apply → global", () => openPreview("global"), "primary"),
    ]));
  }
}

/* ---------- diff preview modal ---------- */
function colorizeDiff(text) {
  const wrap = el("div", { class: "diff" });
  text.split("\n").forEach((line) => {
    const cls = line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "";
    wrap.appendChild(el("span", { class: cls }, [line + "\n"]));
  });
  return wrap;
}
function closeModal() { document.getElementById("modal").innerHTML = ""; }
function openPreview(scope) {
  fetch(q("/api/diff") + "&scope=" + scope)
    .then((r) => r.json())
    .then((data) => {
      const body = el("div", { class: "mbd" });
      const changed = (data.targets || []).filter((t) => t.changed);
      if (!changed.length) body.appendChild(el("div", { class: "empty" }, ["No changes — everything is already in sync."]));
      changed.forEach((t) => {
        body.appendChild(el("div", { class: "diff-target" }, [`${t.display} · ${t.path}`]));
        body.appendChild(colorizeDiff(t.diff));
      });
      const footer = el("div", { class: "mft" }, [
        btn("Cancel", closeModal),
        changed.length ? btn("Apply " + scope, () => { closeModal(); post("/api/apply", { scope }, "Apply (" + scope + ")"); }, "primary") : null,
      ]);
      const modal = el("div", { class: "modal" }, [
        el("div", { class: "mhd" }, [el("span", null, [`Preview · ${scope} scope`]), btn("✕", closeModal, "icon")]),
        body, footer,
      ]);
      document.getElementById("modal").appendChild(el("div", { class: "overlay", onclick: (e) => e.target.classList.contains("overlay") && closeModal() }, [modal]));
    })
    .catch((e) => toast("Preview failed: " + e.message, false));
}

/* ---------- load ---------- */
function load() {
  return fetch(q("/api/state"))
    .then((r) => r.text().then((t) => ({ status: r.status, t })))
    .then(({ status, t }) => {
      let data;
      try { data = JSON.parse(t); } catch (_) { throw new Error(t || "HTTP " + status); }
      if (data.error) throw new Error(data.error);
      DATA = data;
      READONLY = !!data.readOnly;
      document.getElementById("dir").textContent = data.meta.dir;
      document.getElementById("mode").textContent = READONLY ? "read-only" : "read-write";
      renderNav();
      show(SECTION);
    })
    .catch((e) => {
      document.getElementById("dir").textContent = "—";
      const c = document.getElementById("content");
      c.innerHTML = "";
      c.appendChild(el("div", { class: "error" }, [e.message]));
      c.appendChild(el("div", { class: "muted", style: "padding:0 16px" }, [
        "Tip: open the exact URL agentstack printed in your terminal — the token in the address bar must match the running server.",
      ]));
    });
}
document.getElementById("refresh").addEventListener("click", () => load());
load();
