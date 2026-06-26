// agentstack dashboard — vanilla JS, no framework. Sidebar sections over a
// read-only snapshot (/api/state), with editing actions and diff-before-apply.
const token = new URLSearchParams(location.search).get("token") || "";
let DATA = null;
let SECTION = location.hash.slice(1) || "overview";
let READONLY = false;
let SCOPE = "global"; // active scope for toggles, preview, and the pending bar
let PENDING = null; // { scope, targets } from /api/diff, drives the pending bar
let HISTORY = []; // recent apply events from /api/history
let HISTORY_LOADED = false;
let OPEN_SERVER = null;
let ADD_FORM = false;
let SKILL_FORM = false;
let HOOK_FORM = false;
let PLUGIN_FORM = false;

const SECTIONS = [
  { id: "overview", label: "Overview" },
  { id: "discover", label: "Discover" },
  { id: "servers", label: "Servers", count: (d) => d.servers.length },
  { id: "skills", label: "Skills", count: (d) => d.skills.length },
  { id: "settings", label: "Settings", count: (d) => (d.settingsAdapters || []).length },
  { id: "hooks", label: "Hooks", count: (d) => (d.hooks || []).length },
  { id: "plugins", label: "Plugins", count: (d) => (d.pluginRecipes || []).length + (d.plugins || []).length },
  { id: "instructions", label: "Instructions", count: (d) => d.instructions.length },
  { id: "secrets", label: "Secrets", count: (d) => d.secrets.length },
  { id: "activity", label: "Activity" },
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
  const t = el("div", { class: "toast " + (ok ? "ok" : "err") }, [el("span", null, [msg])]);
  if (ok) {
    setTimeout(() => t.remove(), 3400);
  } else {
    // Errors persist with a manual dismiss — they're easy to miss in a few seconds.
    t.appendChild(el("button", { class: "toast-close", "aria-label": "Dismiss", onclick: () => t.remove() }, ["✕"]));
  }
  document.body.appendChild(t);
}
const q = (p) => p + "?token=" + encodeURIComponent(token);
// "1 server" / "2 servers" — pass an explicit plural for irregulars ("harness", "harnesses").
const plural = (n, w, p) => `${n} ${n === 1 ? w : p || w + "s"}`;

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

function runAction(action, fallbackSection) {
  if (!action) return fallbackSection ? show(fallbackSection) : null;
  if (action.type === "section") return show(action.section || fallbackSection || "overview");
  if (action.type === "preview") return openPreview(action.scope || "global", !!action.all);
  if (action.type === "post") {
    if (READONLY) return toast("Dashboard is read-only", false);
    return post(action.path, {}, action.label || "Action");
  }
  return fallbackSection ? show(fallbackSection) : null;
}

function actionButton(model, cls) {
  if (!model) return null;
  const action = model.action || model;
  if (READONLY && action.type === "post") return null;
  return btn(model.label || "Open", () => runAction(action), cls);
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
const VIEWS = { overview, discover, servers, skills, settings, hooks, plugins, instructions, secrets, activity, health };
function show(id) {
  if (!VIEWS[id]) id = "overview";
  SECTION = id;
  OPEN_SERVER = null;
  // Keep the URL in sync so sections are deep-linkable and survive a refresh.
  // The token lives in the query string, so we only touch the hash.
  if (location.hash.slice(1) !== id) {
    const url = location.pathname + location.search + "#" + id;
    history.replaceState(null, "", url);
  }
  renderNav();
  const c = document.getElementById("content");
  c.innerHTML = "";
  VIEWS[id](c);
}
// Back/forward and manual hash edits navigate without a full reload.
window.addEventListener("hashchange", () => {
  const id = location.hash.slice(1);
  if (VIEWS[id] && id !== SECTION && DATA) show(id);
});

/* ---------- scope switch ---------- */
function renderScopeSwitch() {
  const host = document.getElementById("scope-switch");
  if (!host) return;
  host.innerHTML = "";
  if (READONLY || !DATA || DATA.needsInit) return;
  const seg = el("div", { class: "seg", role: "group", "aria-label": "Scope" });
  [["global", "Global"], ["project", "Project"]].forEach(([id, label]) => {
    seg.appendChild(el("button", {
      class: "seg-btn" + (SCOPE === id ? " active" : ""),
      title: id === "project" ? "Changes for this project directory only" : "Changes for your whole machine",
      "aria-pressed": String(SCOPE === id),
      onclick: () => { if (SCOPE !== id) { SCOPE = id; renderScopeSwitch(); show(SECTION); refreshPending(); } },
    }, [label]));
  });
  host.appendChild(seg);
}

/* ---------- pending changes bar ---------- */
function relTime(unixSec) {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - unixSec));
  if (s < 45) return "just now";
  if (s < 3600) return Math.max(1, Math.round(s / 60)) + " min ago";
  if (s < 86400) return Math.round(s / 3600) + " h ago";
  return Math.round(s / 86400) + " d ago";
}
function diffCounts(text) {
  let add = 0, del = 0;
  (text || "").split("\n").forEach((l) => {
    if (l.startsWith("+") && !l.startsWith("+++")) add++;
    else if (l.startsWith("-") && !l.startsWith("---")) del++;
  });
  return { add, del };
}
function refreshPending() {
  if (!DATA || DATA.needsInit || READONLY) { PENDING = null; renderPending(); return Promise.resolve(); }
  return fetch(q("/api/diff") + "&scope=" + SCOPE)
    .then((r) => r.json())
    .then((d) => { PENDING = d; renderPending(); })
    .catch(() => { PENDING = null; renderPending(); });
}
function refreshHistory() {
  return fetch(q("/api/history"))
    .then((r) => r.json())
    .then((d) => { HISTORY = d.entries || []; HISTORY_LOADED = true; renderPending(); if (SECTION === "activity") show("activity"); })
    .catch(() => {});
}
function lastApply() { return HISTORY.find((h) => !h.undone) || null; }

function renderPending() {
  const host = document.getElementById("pending");
  if (!host) return;
  host.innerHTML = "";
  if (!DATA || DATA.needsInit || READONLY) return;
  // An active session takes over the bar — end it to revert.
  if (DATA.session) {
    const s = DATA.session;
    const loads = s.loads || [];
    const sub = s.scope + " scope" + (s.plugin ? " · plugin " + s.plugin : "") +
      (loads.length ? " · " + plural(loads.length, "skill") + " pulled" : "") + " · reverts when you end it";
    host.appendChild(el("div", { class: "pending-bar session" }, [
      el("div", { class: "pleft" }, [
        el("span", { class: "pdot session" }, []),
        el("div", { style: "min-width:0" }, [
          el("div", { class: "ptitle" }, ["Session active · " + s.profile]),
          el("div", { class: "muted", style: "font-size:12px" }, [sub]),
        ]),
      ]),
      el("div", { class: "row-actions" }, [
        loads.length ? btn("Activity", () => show("activity")) : null,
        btn("End session", endSession, "primary"),
      ]),
    ]));
    return;
  }
  const changed = ((PENDING && PENDING.targets) || []).filter((t) => t.changed);
  if (changed.length) {
    const chips = changed.slice(0, 6).map((t) => {
      const c = diffCounts(t.diff);
      return el("span", { class: "pchip" }, [
        el("span", { class: "name" }, [t.display]),
        c.add ? el("span", { class: "padd" }, ["+" + c.add]) : null,
        c.del ? el("span", { class: "pdel" }, ["−" + c.del]) : null,
      ]);
    });
    const extra = changed.length - chips.length;
    if (extra > 0) chips.push(el("span", { class: "pchip muted" }, ["+" + extra + " more"]));
    host.appendChild(el("div", { class: "pending-bar" }, [
      el("div", { class: "pleft" }, [
        el("span", { class: "pdot warn" }, []),
        el("div", { style: "min-width:0" }, [
          el("div", { class: "ptitle" }, [plural(changed.length, "pending change")]),
          el("div", { class: "muted", style: "font-size:12px" }, ["Not yet written to your tools · " + SCOPE + " scope"]),
        ]),
      ]),
      el("div", { class: "pchips" }, chips),
      el("div", { class: "row-actions" }, [
        btn("Review & apply →", () => openPreview(SCOPE), "primary"),
      ]),
    ]));
    return;
  }
  const last = lastApply();
  if (last) {
    host.appendChild(el("div", { class: "pending-bar quiet" }, [
      el("div", { class: "pleft" }, [
        el("span", { class: "pdot ok" }, []),
        el("div", { style: "min-width:0" }, [
          el("div", { class: "ptitle" }, ["Applied " + relTime(last.timeUnix)]),
          el("div", { class: "muted", style: "font-size:12px" }, [last.summary]),
        ]),
      ]),
      el("div", { class: "row-actions" }, [
        btn("Undo", () => undoApply(last.id)),
        btn("Activity", () => show("activity")),
      ]),
    ]));
  }
}
function undoApply(id) {
  if (!confirm("Undo this apply? Your tools' config files are restored to before it. Your saved stack is unchanged, so the changes will show as pending again.")) return;
  return fetch(q("/api/undo"), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ id }) })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => {
      if (!ok || d.error) throw new Error(d.error || "undo failed");
      toast("Reverted ✓", true);
      return load().then(refreshHistory);
    })
    .catch((e) => toast("Undo: " + e.message, false));
}

/* ---------- command palette (⌘K) ---------- */
function openPalette() {
  if (!DATA || DATA.needsInit) return;
  const items = [];
  SECTIONS.forEach((s) => items.push({ label: s.label, hint: "go to", run: () => show(s.id) }));
  (DATA.servers || []).forEach((s) => items.push({ label: s.name, hint: "server", run: () => { OPEN_SERVER = s.name; show("servers"); } }));
  (DATA.skills || []).forEach((s) => items.push({ label: s.name, hint: "skill", run: () => show("skills") }));
  (DATA.settingsAdapters || []).forEach((a) => items.push({ label: a.display + " settings", hint: "settings", run: () => show("settings") }));

  const input = el("input", { class: "inp", placeholder: "Jump to a section, server, skill…", style: "width:100%;height:40px" });
  const list = el("div", { class: "cmd-list" });
  let active = 0;
  function draw() {
    const term = input.value.trim().toLowerCase();
    const matches = items.filter((it) => it.label.toLowerCase().includes(term)).slice(0, 40);
    active = Math.max(0, Math.min(active, matches.length - 1));
    list.innerHTML = "";
    matches.forEach((it, i) => list.appendChild(el("div", {
      class: "cmd-item" + (i === active ? " active" : ""),
      onclick: () => { closeModal(); it.run(); },
    }, [el("span", null, [it.label]), el("span", { class: "k" }, [it.hint])])));
    if (!matches.length) list.appendChild(el("div", { class: "empty", style: "padding:12px" }, ["No matches."]));
    list._matches = matches;
  }
  input.addEventListener("input", draw);
  input.addEventListener("keydown", (e) => {
    const m = list._matches || [];
    if (e.key === "ArrowDown") { active = Math.min(active + 1, m.length - 1); draw(); e.preventDefault(); }
    else if (e.key === "ArrowUp") { active = Math.max(active - 1, 0); draw(); e.preventDefault(); }
    else if (e.key === "Enter") { if (m[active]) { closeModal(); m[active].run(); } }
    else if (e.key === "Escape") closeModal();
  });
  const modal = el("div", { class: "modal cmd" }, [el("div", { class: "mbd" }, [input, list])]);
  document.getElementById("modal").appendChild(el("div", { class: "overlay top", onclick: (e) => e.target.classList.contains("overlay") && closeModal() }, [modal]));
  draw();
  setTimeout(() => input.focus(), 0);
}
document.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) { e.preventDefault(); openPalette(); }
});

/* ---------- activity (apply history + undo) ---------- */
function activity(c) {
  c.appendChild(pageHead("Activity", "Every apply is backed up here — restore your tools to how they were before any change."));

  // Live session: the skills the agent pulled on demand, with its reasons.
  if (DATA.session) {
    const s = DATA.session;
    const loads = s.loads || [];
    const rows = loads.length
      ? loads.map((l) => el("div", { class: "list-row" }, [
          el("span", null, [el("span", { class: "name" }, [l.name]), el("div", { class: "muted", style: "font-size:12px" }, [l.reason || ""])]),
          el("span", { class: "muted", style: "font-size:12px" }, [relTime(l.ts)]),
        ]))
      : [el("div", { class: "empty" }, ["Nothing pulled yet — the agent loads skills on demand from this session's profile."])];
    c.appendChild(el("div", { class: "card", style: "margin-bottom:16px" }, [
      el("div", { class: "hd", style: "display:flex;align-items:center" }, [
        "Session · " + s.profile,
        el("small", null, [s.scope + " scope · " + plural(loads.length, "skill") + " pulled"]),
        READONLY ? null : el("span", { style: "margin-left:auto" }, [btn("End session", endSession)]),
      ]),
      el("div", { class: "bd" }, rows),
    ]));
  }
  if (!HISTORY.length) {
    c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [el("div", { class: "empty" }, ["No applies yet. When you apply changes they show up here, each with an undo."])])]));
    if (!HISTORY_LOADED) refreshHistory();
    return;
  }
  const rows = HISTORY.map((h) => el("div", { class: "list-row", style: "align-items:flex-start" }, [
    el("span", { style: "min-width:0" }, [
      el("span", { class: "name" }, ["Applied " + relTime(h.timeUnix)]),
      el("span", { class: "k" }, [h.scope + " scope"]),
      el("div", { class: "muted", style: "font-size:12px;margin-top:2px" }, [h.summary]),
      el("details", { style: "margin-top:4px" }, [
        el("summary", { class: "muted", style: "font-size:12px;cursor:pointer" }, [plural(h.files.length, "file") + " changed"]),
        el("div", { class: "muted mono", style: "font-size:11px;margin-top:6px" }, h.files.map((f) => el("div", null, [f.label + " · " + f.path]))),
      ]),
    ]),
    el("span", { class: "row-actions" }, [
      h.undone ? badge("undone", "") : READONLY ? null : btn("Undo", () => undoApply(h.id)),
    ]),
  ]));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, rows)]));
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

function addToStack(r) {
  const targets = DATA.meta.defaultTargets || [];
  const where = targets.length ? targets.join(", ") : "your default tools";
  if (!confirm(`Add "${r.name}" to your stack?\n\nIt'll be enabled for: ${where}.\nNothing is written to your tools until you review and apply.`)) return;
  return addFrom(r.addId);
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
      toast("Added ✓ — review it in the pending changes bar, then apply", true);
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
  c.appendChild(pageHead("Discover", "Find a capability and add it to your stack in one step. It's enabled for your default tools and shows up as a pending change to review before anything is written."));

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
  const right = el("div", { class: "card" }, [el("div", { class: "hd" }, ["Your stack", el("small", null, [plural(DATA.servers.length, "server")])]), el("div", { class: "bd" }, stackRows)]);

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
        : btn("Add to stack", () => addToStack(r), "primary"),
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
function statCard(label, value, sub, section) {
  const attrs = { class: "card stat" + (section ? " clickable" : "") };
  if (section) { attrs.role = "button"; attrs.tabIndex = 0; attrs.onclick = () => show(section); }
  const card = el("div", attrs, [
    el("div", { class: "label" }, [label]),
    el("div", { class: "value" }, [String(value)]),
    el("div", { class: "sub" }, [sub || " "]),
  ]);
  if (section) card.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); show(section); } });
  return card;
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
    statCard("Tools", installed, `${d.adapters.length} known`, "health"),
    statCard("Servers", d.servers.length, "MCP", "servers"),
    statCard("Skills", d.skills.length, `${d.skills.filter((s) => s.installed).length} installed`, "skills"),
    statCard("Health", errs ? `${errs} error` : warns ? `${warns} warning` : "Ready", `${secretsOk}/${d.secrets.length} secrets resolved`, "health"),
  ]));

  c.appendChild(el("div", { class: "overview-grid" }, [
    nextActionsCard(),
    stackSummaryCard(installed),
  ]));

  c.appendChild(el("div", { class: "section-title" }, ["Health"]));
  const hsum = errs ? badge(plural(errs, "error"), "red") : warns ? badge(plural(warns, "warning"), "amber") : badge("all good", "green");
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [
    el("div", { style: "margin-bottom:8px" }, [hsum]),
    ...d.health.slice(0, 5).map(healthRow),
  ])]));

  c.appendChild(el("div", { class: "grid cols-2", style: "margin-top:18px" }, [profilesCard(), usageCard()]));
}

function nextActionsCard() {
  const actions = DATA.nextActions || [];
  const body = [];
  if (!actions.length) {
    body.push(el("div", { class: "empty compact" }, ["Stack is ready. Preview before applying any manual changes."]));
  } else {
    actions.slice(0, 6).forEach((a) => body.push(nextActionRow(a)));
  }
  if (!READONLY) {
    body.push(el("div", { class: "toolbar tight", style: "margin-top:12px" }, [
      btn("Review & apply →", () => openPreview(SCOPE), "primary"),
      btn("Review all tools", () => openPreview(SCOPE, true)),
    ]));
  }
  return el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Next actions", el("small", null, [actions.length ? plural(actions.length, "open item") : "No blocking work detected"])]),
    el("div", { class: "bd" }, body),
  ]);
}

function nextActionRow(a) {
  const kind = a.level === "error" ? "red" : a.level === "warn" ? "amber" : "green";
  return el("div", { class: "next-action" }, [
    el("div", { class: "next-main" }, [
      badge(a.level || "info", kind),
      el("div", null, [
        el("div", { class: "name" }, [a.title || "Action needed"]),
        el("div", { class: "muted", style: "font-size:12px" }, [a.detail || ""]),
      ]),
    ]),
    el("div", { class: "row-actions" }, [
      actionButton(a.primary, "primary"),
      actionButton(a.secondary),
    ]),
  ]);
}

function stackSummaryCard(installed) {
  const targets = (DATA.meta.defaultTargets || []).length ? DATA.meta.defaultTargets.join(", ") : "all registered";
  const plugins = DATA.pluginRecipes || [];
  const readyPlugins = plugins.filter((r) => recipeReady(r)).length;
  return el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Stack summary", el("small", null, [plural(installed, "detected harness", "detected harnesses")])]),
    el("div", { class: "bd" }, [
      summaryLine("Default targets", targets),
      summaryLine("Plugin recipes", `${readyPlugins}/${plugins.length} ready`),
      summaryLine("Instructions", plural(DATA.instructions.length, "fragment")),
      summaryLine("Hooks", plural((DATA.hooks || []).length, "hook")),
      summaryLine("Mode", READONLY ? "read-only" : "read-write"),
    ]),
  ]);
}

function summaryLine(label, value) {
  return el("div", { class: "summary-line" }, [
    el("span", { class: "muted" }, [label]),
    el("span", { class: "mono" }, [String(value)]),
  ]);
}

function profilesCard() {
  const rows = DATA.profiles.map((p) => {
    const meta = el("span", { class: "muted", style: "font-size:12px" }, [`${plural(p.servers.length, "server")} · ${plural(p.skills.length, "skill")}`]);
    const actions = READONLY
      ? [meta]
      : [
          meta,
          btn("Start session", () => openSessionModal(p.name)),
          btn(`Activate (${SCOPE})`, () => post("/api/use", { profile: p.name, scope: SCOPE }, "Activate " + p.name)),
        ];
    return el("div", { class: "list-row" }, [el("span", { class: "name" }, [p.name]), el("span", { class: "row-actions" }, actions)]);
  });
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No profiles yet. Create one to load a set of skills and servers together."]));
  const hd = ["Profiles"];
  if (!READONLY) {
    hd.push(el("span", { style: "margin-left:auto;display:flex;gap:8px" }, [
      btn("+ New profile", openProfileForm),
      DATA.profiles.length ? btn("Start session", () => openSessionModal(null), "primary") : null,
    ]));
  }
  return el("div", { class: "card" }, [el("div", { class: "hd", style: "display:flex;align-items:center" }, hd), el("div", { class: "bd" }, rows)]);
}

/* ---------- sessions ---------- */
function endSession() {
  if (!confirm("End the session? This reverts the loaded skills, servers, and plugin to how they were when you started it.")) return;
  return post("/api/session_end", {}, "Session ended");
}
function openSessionModal(preselect) {
  const profiles = DATA.profiles || [];
  if (!profiles.length) { toast("Create a profile first", false); return openProfileForm(); }
  const profSel = el("select", { class: "inp", style: "height:32px;width:100%" }, profiles.map((p) => el("option", { value: p.name }, [p.name])));
  if (preselect) profSel.value = preselect;
  const scopeSel = el("select", { class: "inp", style: "height:32px;width:100%" }, [
    el("option", { value: "project" }, ["project — this repo only"]),
    el("option", { value: "global" }, ["global — everywhere"]),
  ]);
  const recipes = DATA.pluginRecipes || [];
  const plugSel = el("select", { class: "inp", style: "height:32px;width:100%" }, [
    el("option", { value: "" }, ["(no plugin)"]),
    ...recipes.map((r) => el("option", { value: r.name }, [r.display || r.name])),
  ]);
  const row = (label, node) => el("div", { style: "display:flex;align-items:center;gap:10px;margin-bottom:10px" }, [el("label", { class: "muted", style: "width:64px;font-size:12px" }, [label]), node]);
  const body = el("div", { class: "mbd" }, [
    el("div", { class: "muted", style: "font-size:12px;margin-bottom:12px" }, ["Loads the profile (and an optional plugin) now. Ending the session reverts everything to how it is right now."]),
    row("Profile", profSel), row("Scope", scopeSel), row("Plugin", plugSel),
  ]);
  const footer = el("div", { class: "mft" }, [
    btn("Cancel", closeModal),
    btn("Start session", () => {
      const b = { profile: profSel.value, scope: scopeSel.value };
      if (plugSel.value) b.plugin = plugSel.value;
      closeModal();
      post("/api/session_start", b, "Session started");
    }, "primary"),
  ]);
  const modal = el("div", { class: "modal" }, [el("div", { class: "mhd" }, [el("span", null, ["Start a session"]), btn("✕", closeModal, "icon")]), body, footer]);
  document.getElementById("modal").appendChild(el("div", { class: "overlay", onclick: (e) => e.target.classList.contains("overlay") && closeModal() }, [modal]));
}
function openProfileForm() {
  const nameInp = el("input", { class: "inp", placeholder: "e.g. review-mode", style: "width:100%" });
  const skillChecks = checkboxList("pf-skill", DATA.skills || [], "No skills in your stack yet.");
  const serverChecks = checkboxList("pf-server", DATA.servers || [], "No servers in your stack yet.");
  const body = el("div", { class: "mbd" }, [
    el("div", { style: "margin-bottom:6px" }, [el("label", { class: "muted", style: "font-size:12px" }, ["Name"]), nameInp]),
    el("div", { class: "section-title", style: "margin:14px 0 6px" }, ["Skills"]),
    el("div", null, skillChecks),
    el("div", { class: "section-title", style: "margin:14px 0 6px" }, ["Servers"]),
    el("div", null, serverChecks),
  ]);
  const footer = el("div", { class: "mft" }, [
    btn("Cancel", closeModal),
    btn("Create profile", () => {
      const name = nameInp.value.trim();
      if (!name) return toast("Name is required", false);
      const checked = (n) => Array.from(document.querySelectorAll(`input[name="${n}"]:checked`)).map((x) => x.value);
      closeModal();
      post("/api/add_profile", { name, skills: checked("pf-skill"), servers: checked("pf-server") }, "Profile created");
    }, "primary"),
  ]);
  const modal = el("div", { class: "modal" }, [el("div", { class: "mhd" }, [el("span", null, ["New profile"]), btn("✕", closeModal, "icon")]), body, footer]);
  document.getElementById("modal").appendChild(el("div", { class: "overlay", onclick: (e) => e.target.classList.contains("overlay") && closeModal() }, [modal]));
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
  return post("/api/toggle", { server: serverName, target, scope: SCOPE, enable: !currentlyOn },
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

// A key for the matrix scope tags + the global/project mental model.
function scopeLegend() {
  const item = (tag, desc) => el("span", { class: "legend-item" }, [
    el("span", { class: "sc sc-" + tag, style: "position:static" }, [tag]),
    el("span", { class: "muted", style: "font-size:11px" }, [desc]),
  ]);
  const legend = [
    item("global", "every project"),
    item("project", "this repo only"),
    item("both", "global + here"),
  ];
  if (SCOPE === "project") {
    legend.push(el("span", { class: "legend-item" }, [
      el("span", { class: "inherited" }, ["✓"]),
      el("span", { class: "muted", style: "font-size:11px" }, ["inherited from global"]),
    ]));
  }
  legend.push(el("span", { class: "muted", style: "font-size:11px;flex:1" }, [
    SCOPE === "project"
      ? "Showing project scope — ✓ marks what's set in this repo; faded ✓ is active here via global."
      : "Showing global scope — what's on for every project. Project scope adds to this.",
  ]));
  return el("div", { class: "scope-legend" }, legend);
}

// One matrix cell shared by the servers & skills tables. When `onToggle` is
// given it's a real keyboard-operable toggle (role=button, aria-pressed);
// otherwise it carries an aria-label so the ✓/– glyph isn't the only signal.
function statusCell(cell, opts) {
  // The visible state follows the ACTIVE scope, so flipping the switch actually
  // changes the matrix. In project view, a server that's only on globally shows
  // a faded ✓ ("inherited") — it's active here, just not set at the project level.
  const here = SCOPE === "project" ? !!cell.project : !!cell.global;
  const other = SCOPE === "project" ? !!cell.global : !!cell.project;
  const inherited = SCOPE === "project" && !here && other;

  let mark, markClass, stateText;
  if (here) { mark = "✓"; markClass = "on"; stateText = "on in " + SCOPE; }
  else if (inherited) { mark = "✓"; markClass = "inherited"; stateText = "on here via global"; }
  else { mark = "–"; markClass = "off"; stateText = "off in " + SCOPE; }

  // Tag shows where it's set: both, the active scope, or the other scope.
  const tag = here && other ? "both" : here ? SCOPE : other ? (SCOPE === "project" ? "global" : "project") : "";

  const inner = [el("div", { class: markClass }, [mark]), tag ? el("div", { class: "sc sc-" + tag }, [tag]) : null];
  const td = el("td", { class: "cell" }, inner);
  if (opts.onToggle) {
    const verb = here ? "Disable " : "Enable ";
    td.setAttribute("role", "button");
    td.tabIndex = 0;
    td.setAttribute("aria-pressed", String(here));
    td.setAttribute("aria-label", verb + opts.label + " (" + SCOPE + " scope)");
    td.style.cursor = "pointer";
    td.title = (inherited ? "on here via global · " : "") + verb.toLowerCase() + opts.label + " (" + SCOPE + ")";
    const fire = (e) => { e.stopPropagation(); e.preventDefault(); opts.onToggle(); };
    td.addEventListener("click", fire);
    td.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") fire(e); });
  } else {
    td.setAttribute("aria-label", opts.label + ": " + stateText);
    if (opts.disabledTitle) td.title = opts.disabledTitle;
  }
  return td;
}

function servers(c) {
  const d = DATA;
  // Only CLIs that actually support MCP get a column (Pi, etc. have none).
  const cols = d.adapters.filter((a) => a.mcp !== false);
  c.appendChild(pageHead("Servers", "Turn a server on or off for each tool. Click a name to see its config. Changes apply to the scope selected up top."));
  if (!READONLY) {
    c.appendChild(el("div", { class: "toolbar", style: "margin-bottom:14px" }, [
      btn(ADD_FORM ? "Close" : "+ Add MCP server", () => { ADD_FORM = !ADD_FORM; show("servers"); }, "primary"),
    ]));
    if (ADD_FORM) c.appendChild(addServerCard());
  }

  const head = el("tr", null, [el("th", null, ["capability"])]);
  cols.forEach((a) => head.appendChild(el("th", { class: "cell" }, [a.display])));
  head.appendChild(el("th", null, ["type"]));

  const body = el("tbody");
  if (!d.servers.length) body.appendChild(el("tr", null, [el("td", { colspan: cols.length + 2 }, [el("span", { class: "empty" }, ["No servers yet. Use “+ Add MCP server” or the Discover tab."])])]));
  d.servers.forEach((s) => {
    const tr = el("tr", { class: "clickable" }, [
      el("td", { onclick: () => { OPEN_SERVER = OPEN_SERVER === s.name ? null : s.name; show("servers"); } },
        [el("span", { class: "name" }, [s.name]), el("span", { class: "k" }, ["mcp"])]),
    ]);
    cols.forEach((a) => {
      const cell = s.cells.find((x) => x.adapter === a.id) || {};
      tr.appendChild(statusCell(cell, {
        label: `${s.name} for ${a.display}`,
        onToggle: READONLY ? null : () => toggleCell(s.name, a.id, SCOPE === "project" ? !!cell.project : !!cell.global),
      }));
    });
    tr.appendChild(el("td", null, [badge(s.type, "solid")]));
    body.appendChild(tr);
    if (OPEN_SERVER === s.name) body.appendChild(serverDetail(s, cols.length + 2));
  });
  c.appendChild(scopeLegend());
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd", style: "padding:6px 8px" }, [el("div", { class: "table-wrap" }, [el("table", null, [el("thead", null, [head]), body])])])]));
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
  return el("tr", { class: "detail" }, [el("td", { colspan: span }, [el("div", { class: "bd" }, [
    el("div", { class: "kv" }, kv),
    el("div", { class: "toolbar", style: "margin-top:10px" }, [btn("Explain trust ⓘ", () => explainModal(s.name))]),
  ])])]);
}

/* ---------- skills ---------- */
function toggleSkillCell(skillName, target, currentlyOn) {
  return post("/api/toggle_skill", { skill: skillName, target, scope: SCOPE, enable: !currentlyOn },
    (currentlyOn ? "Disabled " : "Enabled ") + skillName);
}

function saveSkill() {
  const g = (id) => (document.getElementById(id) || {}).value || "";
  const source = g("sk-source");
  const body = { name: g("sk-name").trim(), source };
  if (!body.name) return toast("Name is required", false);
  if (source === "git") { body.git = g("sk-git").trim(); body.rev = g("sk-rev").trim(); }
  else body.path = g("sk-path").trim();
  fetch(q("/api/add_skill"), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => {
      if (!ok || d.error) throw new Error(d.error || "failed");
      SKILL_FORM = false;
      toast("Skill added — click Install, then enable it per CLI", true);
      load();
    })
    .catch((e) => toast("Add skill: " + e.message, false));
}

function addSkillCard() {
  const row = (label, node, id) => el("div", { id, style: "display:flex;align-items:center;gap:10px;margin-bottom:8px" }, [
    el("label", { class: "muted", style: "width:90px;font-size:12px" }, [label]), node,
  ]);
  const source = el("select", { id: "sk-source", style: "height:32px", onchange: () => {
    document.getElementById("row-git").style.display = source.value === "git" ? "" : "none";
    document.getElementById("row-rev").style.display = source.value === "git" ? "" : "none";
    document.getElementById("row-path").style.display = source.value === "path" ? "" : "none";
  } }, [el("option", { value: "git" }, ["git"]), el("option", { value: "path" }, ["path"])]);
  return el("div", { class: "card", style: "margin-bottom:16px" }, [
    el("div", { class: "hd" }, ["Add skill", el("small", null, ["added to the manifest; then Install + enable per CLI"])]),
    el("div", { class: "bd" }, [
      row("name", el("input", { id: "sk-name", class: "inp", placeholder: "e.g. code-review", style: "width:220px" })),
      row("source", source),
      row("git URL", el("input", { id: "sk-git", class: "inp", placeholder: "https://github.com/acme/skills.git", style: "width:340px" }), "row-git"),
      row("rev", el("input", { id: "sk-rev", class: "inp", placeholder: "(optional) tag / branch / sha", style: "width:340px" }), "row-rev"),
      row("path", el("input", { id: "sk-path", class: "inp", placeholder: "./skills/code-review", style: "width:340px" }), "row-path"),
      el("div", { class: "toolbar", style: "margin-top:6px" }, [btn("Save", saveSkill, "primary"), btn("Cancel", () => { SKILL_FORM = false; show("skills"); })]),
    ]),
  ]);
}

function skills(c) {
  c.appendChild(pageHead("Skills", "Turn a skill on or off for each tool. A skill must be installed before you can enable it."));
  const adapters = DATA.skillAdapters || [];

  if (!READONLY) {
    const tools = [btn(SKILL_FORM ? "Close" : "+ Add skill", () => { SKILL_FORM = !SKILL_FORM; show("skills"); }, "primary")];
    if (DATA.skills.some((s) => !s.installed)) tools.push(btn("Install missing", () => post("/api/install", {}, "Install")));
    c.appendChild(el("div", { class: "toolbar", style: "margin-bottom:14px" }, tools));
    if (SKILL_FORM) c.appendChild(addSkillCard());
    // path source is hidden by default (git is the first option)
    if (SKILL_FORM) setTimeout(() => { const p = document.getElementById("row-path"); if (p) p.style.display = "none"; }, 0);
  }

  const head = el("tr", null, [el("th", null, ["skill"])]);
  adapters.forEach((a) => head.appendChild(el("th", { class: "cell" }, [a.display])));
  head.appendChild(el("th", null, ["status"]));

  const body = el("tbody");
  if (!DATA.skills.length) body.appendChild(el("tr", null, [el("td", { colspan: adapters.length + 2 }, [el("span", { class: "empty" }, ["No skills in the manifest. Add [skills.*] or install from a source."])])]));
  DATA.skills.forEach((s) => {
    const detail = s.source === "git"
      ? `git · ${(s.src.git || "")}${s.lockedRev ? " @ " + s.lockedRev.slice(0, 8) : ""}`
      : `path · ${s.src.path || ""}`;
    const tr = el("tr", null, [
      el("td", null, [el("span", { class: "name" }, [s.name]), el("div", { class: "muted mono", style: "font-size:12px" }, [detail])]),
    ]);
    adapters.forEach((a) => {
      const cell = (s.cells || []).find((x) => x.adapter === a.id) || {};
      tr.appendChild(statusCell(cell, {
        label: `${s.name} for ${a.display}`,
        onToggle: !READONLY && s.installed ? () => toggleSkillCell(s.name, a.id, SCOPE === "project" ? !!cell.project : !!cell.global) : null,
        disabledTitle: !READONLY && !s.installed ? "install the skill first to enable it" : null,
      }));
    });
    tr.appendChild(el("td", null, [el("span", { class: "row-actions" }, [
      s.installed ? badge("installed", "green") : badge("not installed", "amber"),
      btn("ⓘ", () => explainModal(s.name), "icon"),
    ])]));
    body.appendChild(tr);
  });
  c.appendChild(scopeLegend());
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd", style: "padding:6px 8px" }, [el("div", { class: "table-wrap" }, [el("table", null, [el("thead", null, [head]), body])])])]));

  // Skills present on disk in your CLIs but not yet in the manifest. Every entry
  // is shown — valid ones are adoptable; broken/non-skill ones show a status.
  const found = (DATA.discoveredSkills || []).filter((d) => !d.inManifest);
  if (found.length) {
    const adoptable = found.filter((d) => d.valid !== false);
    const hd = ["Detected on disk", el("small", null, [`${found.length} found · ${adoptable.length} manageable`])];
    if (!READONLY && adoptable.length) hd.push(el("span", { style: "margin-left:auto;display:flex;gap:8px" }, [
      btn("Move all into agentstack", () => {
        if (confirm("Move " + plural(adoptable.length, "skill folder") + " into ~/.agentstack/skills/ and replace the originals with symlinks? Your agents keep working in place; a backup is kept.")) post("/api/consolidate_skills", {}, "Consolidate skills");
      }, "primary"),
      btn("Adopt all in place", () => post("/api/adopt_all_skills", {}, "Adopt skills")),
    ]));
    const rows = found.map((d) => {
      const where = (d.presentIn || []).map((t) => badge(t, "solid"));
      const ok = d.valid !== false;
      const statusBadge = d.broken ? badge("broken link", "red") : !ok ? badge("no SKILL.md", "amber") : null;
      return el("div", { class: "list-row" }, [
        el("span", null, [
          el("span", { class: "name", style: ok ? "" : "opacity:.7" }, [d.name]),
          el("div", { class: "muted mono", style: "font-size:12px" }, [(d.isSymlink ? "symlink · " : "") + d.source]),
        ]),
        el("span", { class: "row-actions" }, [
          ...where,
          statusBadge,
          READONLY || !ok ? null : btn("Move", () => post("/api/consolidate_skills", { names: [d.name] }, "Consolidate " + d.name)),
          READONLY || !ok ? null : btn("Adopt", () => post("/api/adopt_skill", { name: d.name }, "Adopt " + d.name)),
        ]),
      ]);
    });
    c.appendChild(el("div", { class: "card", style: "margin-top:16px" }, [
      el("div", { class: "hd", style: "display:flex;align-items:center" }, hd),
      el("div", { class: "bd" }, [
        el("div", { class: "muted", style: "font-size:12px;margin-bottom:8px" }, ["“Move into agentstack” relocates the skill files to one managed home (~/.agentstack/skills/) and symlinks the originals back — agents keep working, you control them from here. “Adopt in place” just registers them where they are."]),
        ...rows,
      ]),
    ]));
  }
}

/* ---------- settings ---------- */
// Working copies of each CLI's settings object, keyed by adapter id. Typed
// controls mutate these; Save sends the whole object (so keys we don't have a
// control for are preserved).
let SETTINGS_DRAFT = {};

function getPath(obj, dotted) {
  return dotted.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
}
function setPath(obj, dotted, val) {
  const ks = dotted.split(".");
  let o = obj;
  for (let i = 0; i < ks.length - 1; i++) {
    if (typeof o[ks[i]] !== "object" || o[ks[i]] == null || Array.isArray(o[ks[i]])) o[ks[i]] = {};
    o = o[ks[i]];
  }
  o[ks[ks.length - 1]] = val;
}
function delPath(obj, dotted) {
  const ks = dotted.split(".");
  const stack = [obj];
  let o = obj;
  for (let i = 0; i < ks.length - 1; i++) {
    if (o[ks[i]] == null) return;
    o = o[ks[i]];
    stack.push(o);
  }
  delete o[ks[ks.length - 1]];
  // Prune now-empty ancestor objects (e.g. permissions: {}).
  for (let i = ks.length - 2; i >= 0; i--) {
    const parent = stack[i];
    if (parent[ks[i]] && typeof parent[ks[i]] === "object" && Object.keys(parent[ks[i]]).length === 0) delete parent[ks[i]];
  }
}
function initialFor(f) {
  if (f.default !== undefined && f.default !== null) return f.default;
  if (f.type === "bool") return true;
  if (f.type === "enum") return f.options[0];
  if (f.type === "number") return 0;
  return "";
}

function settings(c) {
  c.appendChild(pageHead("Settings", "Shows each tool's current settings, read from its real config file. Adjust what you want, then Save to let agentstack manage it — Apply writes it back to your tools. Keys you don't manage are left untouched."));
  const adapters = DATA.settingsAdapters || [];
  if (!adapters.length) {
    c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [el("div", { class: "empty" }, ["No CLIs with a managed settings file yet."])])]));
    return;
  }
  adapters.forEach((a, i) => c.appendChild(settingsCard(a, i === 0)));
}

function settingsCard(a, open) {
  // Default to the CLI's live settings file, with manifest-managed keys
  // overriding (top-level ownership) — so the panel reflects reality without a
  // manual import. Save persists the draft to the manifest.
  const draft = JSON.parse(JSON.stringify(a.live || {}));
  Object.entries(a.current || {}).forEach(([k, v]) => { draft[k] = JSON.parse(JSON.stringify(v)); });
  SETTINGS_DRAFT[a.id] = draft;
  const fields = a.fields || [];
  const previewId = "settings-prev-" + a.id;
  const refreshPreview = () => {
    const p = document.getElementById(previewId);
    if (p) p.textContent = Object.keys(draft).length ? JSON.stringify(draft, null, 2) : "(nothing set)";
  };

  // Group fields by their `group`.
  const groups = {};
  fields.forEach((f) => { (groups[f.group || "Other"] = groups[f.group || "Other"] || []).push(f); });

  const body = [el("div", { class: "muted mono", style: "font-size:12px;margin-bottom:10px" }, [a.path])];

  Object.keys(groups).forEach((g) => {
    body.push(el("div", { class: "section-title", style: "margin:14px 0 6px" }, [g]));
    groups[g].forEach((f) => body.push(settingRow(a.id, f, draft, refreshPreview)));
  });

  // Keys present in the file but not in our catalog — preserved, shown read-only.
  const known = new Set(fields.map((f) => f.key.split(".")[0]));
  const extras = Object.keys(draft).filter((k) => !known.has(k));
  if (extras.length) {
    body.push(el("div", { class: "muted", style: "margin-top:12px;font-size:12px" }, [
      "Preserved (no control yet): " + extras.join(", "),
    ]));
  }

  body.push(el("div", { class: "section-title", style: "margin:14px 0 6px" }, ["Resulting settings"]));
  const pre = el("pre", { id: previewId, class: "mono", style: "background:hsl(var(--muted));padding:10px;border-radius:8px;font-size:12px;overflow:auto;max-height:220px" });
  pre.textContent = Object.keys(draft).length ? JSON.stringify(draft, null, 2) : "(nothing set)";
  body.push(pre);

  if (!READONLY) {
    const managed = Object.keys(a.current || {}).length > 0;
    body.push(el("div", { class: "toolbar", style: "margin-top:10px" }, [
      btn("Save", () => post("/api/set_settings", { target: a.id, settings: draft }, a.display + " settings"), "primary"),
      btn("Reset", () => show("settings")),
      el("span", { class: "muted", style: "font-size:12px" }, [
        managed ? "Showing live values with your managed keys applied · Save then Apply to write" : "Showing this CLI's current settings · Save to start managing them",
      ]),
    ]));
  }

  const managedCount = Object.keys(a.current || {}).length;
  const attrs = { class: "card acc", style: "margin-bottom:12px" };
  if (open) attrs.open = "";
  return el("details", attrs, [
    el("summary", { class: "hd acc-sum" }, [
      el("span", null, [a.display, el("small", { style: "display:inline;margin-left:8px" }, [a.id === "codex" ? "config.toml" : "settings.json"])]),
      managedCount ? badge(plural(managedCount, "managed key"), "solid") : el("span", { class: "muted", style: "font-size:12px" }, ["not managed yet"]),
    ]),
    el("div", { class: "bd" }, body),
  ]);
}

function settingRow(adapterId, f, draft, refresh) {
  const managed = getPath(draft, f.key) !== undefined;
  const label = f.label || f.key;

  // The value control for this field type.
  let control;
  const sync = () => refresh();
  if (f.type === "bool") {
    control = el("input", { type: "checkbox" });
    control.checked = managed ? !!getPath(draft, f.key) : (f.default === true);
    control.addEventListener("change", () => { if (getPath(draft, f.key) !== undefined) { setPath(draft, f.key, control.checked); sync(); } });
  } else if (f.type === "enum") {
    control = el("select", { style: "height:30px" }, f.options.map((o) => el("option", { value: o }, [o])));
    control.value = managed ? String(getPath(draft, f.key)) : f.options[0];
    control.addEventListener("change", () => { if (getPath(draft, f.key) !== undefined) { setPath(draft, f.key, control.value); sync(); } });
  } else {
    control = el("input", { class: "inp", type: f.type === "number" ? "number" : "text", style: "width:240px;height:30px" });
    if (managed) control.value = getPath(draft, f.key);
    if (f.default != null) control.placeholder = "default: " + f.default;
    control.addEventListener("input", () => {
      if (getPath(draft, f.key) === undefined) return;
      setPath(draft, f.key, f.type === "number" ? Number(control.value) : control.value);
      sync();
    });
  }
  control.disabled = !managed || READONLY;

  // The "manage this setting" toggle.
  const manage = el("input", { type: "checkbox" });
  manage.checked = managed;
  manage.disabled = READONLY;
  manage.addEventListener("change", () => {
    if (manage.checked) {
      const init = f.type === "bool" ? (control.checked) :
        f.type === "enum" ? control.value :
        (control.value !== "" ? (f.type === "number" ? Number(control.value) : control.value) : initialFor(f));
      setPath(draft, f.key, init);
      control.disabled = READONLY;
    } else {
      delPath(draft, f.key);
      control.disabled = true;
    }
    refresh();
  });

  return el("div", { style: "display:flex;align-items:center;gap:12px;padding:5px 0;border-bottom:1px solid hsl(var(--border))" }, [
    el("label", { style: "display:flex;align-items:center;gap:7px;width:300px;cursor:pointer" }, [
      manage,
      el("span", null, [el("span", { class: "name" }, [label]), el("div", { class: "muted mono", style: "font-size:11px" }, [f.key])]),
    ]),
    control,
    f.help ? el("span", { class: "muted", style: "font-size:11px;flex:1" }, [f.help]) : null,
  ]);
}

/* ---------- hooks ---------- */
const HOOK_EVENTS = ["PreToolUse", "PostToolUse", "UserPromptSubmit", "SessionStart", "SessionEnd", "Stop", "SubagentStop", "PreCompact", "Notification"];
function saveHook() {
  const g = (id) => (document.getElementById(id) || {}).value || "";
  const body = { name: g("hk-name").trim(), event: g("hk-event"), command: g("hk-command").trim() };
  const m = g("hk-matcher").trim();
  if (m) body.matcher = m;
  if (!body.name) return toast("Name is required", false);
  if (!body.command) return toast("Command is required", false);
  fetch(q("/api/add_hook"), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => { if (!ok || d.error) throw new Error(d.error || "failed"); HOOK_FORM = false; toast("Hook added — Apply to write it", true); load(); })
    .catch((e) => toast("Add hook: " + e.message, false));
}
function addHookCard() {
  const row = (label, node) => el("div", { style: "display:flex;align-items:center;gap:10px;margin-bottom:8px" }, [
    el("label", { class: "muted", style: "width:90px;font-size:12px" }, [label]), node,
  ]);
  const ev = el("select", { id: "hk-event", style: "height:32px" }, HOOK_EVENTS.map((e) => el("option", { value: e }, [e])));
  return el("div", { class: "card", style: "margin-bottom:16px" }, [
    el("div", { class: "hd" }, ["Add hook", el("small", null, ["compiled into each harness's native hooks config on Apply"])]),
    el("div", { class: "bd" }, [
      row("name", el("input", { id: "hk-name", class: "inp", placeholder: "e.g. format-on-edit", style: "width:240px" })),
      row("event", ev),
      row("matcher", el("input", { id: "hk-matcher", class: "inp", placeholder: "(optional) e.g. Edit|Write", style: "width:240px" })),
      row("command", el("input", { id: "hk-command", class: "inp", placeholder: "prettier --write", style: "width:340px" })),
      el("div", { class: "toolbar", style: "margin-top:6px" }, [btn("Save", saveHook, "primary"), btn("Cancel", () => { HOOK_FORM = false; show("hooks"); })]),
    ]),
  ]);
}
function hooks(c) {
  c.appendChild(pageHead("Hooks", "Run commands at lifecycle events (PreToolUse, SessionStart, …). Declared once here; compiled into each harness's native hooks config on Apply."));
  const list = DATA.hooks || [];
  if (!READONLY) {
    c.appendChild(el("div", { class: "toolbar", style: "margin-bottom:14px" }, [
      btn(HOOK_FORM ? "Close" : "+ Add hook", () => { HOOK_FORM = !HOOK_FORM; show("hooks"); }, "primary"),
    ]));
    if (HOOK_FORM) c.appendChild(addHookCard());
  }
  const rows = list.map((h) => el("div", { class: "list-row" }, [
    el("span", null, [
      el("span", { class: "name" }, [h.name]),
      el("div", { class: "muted mono", style: "font-size:12px" }, [h.event + (h.matcher ? " · " + h.matcher : "") + " → " + h.command]),
    ]),
    el("span", { class: "row-actions" }, (h.targets || ["*"]).map((t) => badge(t, "solid"))),
  ]));
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No hooks yet. Add one, or [hooks.*] in the manifest."]));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, rows)]));
}

/* ---------- plugins ---------- */
function savePluginRecipe() {
  const g = (id) => (document.getElementById(id) || {}).value || "";
  const checked = (name) => Array.from(document.querySelectorAll(`input[name="${name}"]:checked`)).map((x) => x.value);
  const body = {
    name: g("pl-name").trim(),
    version: g("pl-version").trim() || "0.1.0",
    description: g("pl-description").trim(),
    display: g("pl-display").trim(),
    category: g("pl-category").trim(),
    targets: checked("pl-target"),
    servers: checked("pl-server"),
    skills: checked("pl-skill"),
    hooks: checked("pl-hook"),
    defaultEnabled: !!(document.getElementById("pl-default-enabled") || {}).checked,
  };
  if (!body.name) return toast("Name is required", false);
  if (!body.description) return toast("Description is required", false);
  fetch(q("/api/add_plugin_recipe"), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => {
      if (!ok || d.error) throw new Error(d.error || "failed");
      PLUGIN_FORM = false;
      toast("Plugin recipe added", true);
      load();
    })
    .catch((e) => toast("Add plugin recipe: " + e.message, false));
}

function checkboxList(name, items, empty) {
  if (!items.length) return [el("div", { class: "empty", style: "padding:8px 0" }, [empty])];
  return items.map((item) => el("label", { style: "display:flex;align-items:center;gap:7px;padding:4px 0" }, [
    el("input", { type: "checkbox", name, value: item.name || item.id }),
    el("span", null, [item.display || item.name || item.id]),
  ]));
}

function addPluginRecipeCard() {
  const row = (label, node) => el("div", { style: "display:flex;align-items:flex-start;gap:10px;margin-bottom:8px" }, [
    el("label", { class: "muted", style: "width:92px;font-size:12px;padding-top:7px" }, [label]), node,
  ]);
  const pluginTargets = (DATA.adapters || []).filter((a) => ["codex", "claude-code"].includes(a.id));
  const targetChecks = checkboxList("pl-target", pluginTargets, "No plugin-capable targets detected.");
  targetChecks.forEach((node) => {
    const input = node.querySelector && node.querySelector("input");
    if (input) input.checked = true;
  });
  return el("div", { class: "card", style: "margin-bottom:16px" }, [
    el("div", { class: "hd" }, ["Create managed recipe", el("small", null, ["compose one shareable plugin from existing capabilities"])]),
    el("div", { class: "bd" }, [
      row("name", el("input", { id: "pl-name", class: "inp", placeholder: "e.g. play", style: "width:220px" })),
      row("version", el("input", { id: "pl-version", class: "inp", value: "0.1.0", style: "width:120px" })),
      row("display", el("input", { id: "pl-display", class: "inp", placeholder: "(optional) Play", style: "width:260px" })),
      row("category", el("input", { id: "pl-category", class: "inp", placeholder: "Developer Tools", style: "width:220px" })),
      row("description", el("textarea", { id: "pl-description", class: "inp", placeholder: "What this plugin helps developers do", style: "width:420px;min-height:62px" })),
      row("targets", el("div", null, targetChecks)),
      row("servers", el("div", null, checkboxList("pl-server", DATA.servers || [], "No servers in the manifest."))),
      row("skills", el("div", null, checkboxList("pl-skill", DATA.skills || [], "No skills in the manifest."))),
      row("hooks", el("div", null, checkboxList("pl-hook", DATA.hooks || [], "No hooks in the manifest."))),
      row("", el("label", { style: "display:flex;align-items:center;gap:7px" }, [
        el("input", { id: "pl-default-enabled", type: "checkbox" }),
        el("span", null, ["default enabled in generated manifests"]),
      ])),
      el("div", { class: "toolbar", style: "margin-top:6px" }, [
        btn("Save recipe", savePluginRecipe, "primary"),
        btn("Cancel", () => { PLUGIN_FORM = false; show("plugins"); }),
      ]),
    ]),
  ]);
}

function recipeStateBadge(r) {
  if (r.conflict) return badge("conflict", "red");
  if ((r.missingSkills || []).length) return badge("skill missing", "amber");
  if (!r.generated) return badge("not generated", "amber");
  if (r.stale) return badge("stale", "amber");
  return badge("generated", "green");
}

function recipeInstallBadges(r) {
  const out = [];
  (r.marketplaces || []).forEach((m) => {
    out.push(badge(m.target + (m.present ? (m.stale ? " marketplace stale" : " marketplace") : " marketplace missing"), m.present && !m.stale ? "green" : "amber"));
    out.push(badge(m.target + (m.nativeVisible ? " native visible" : " native hidden"), m.nativeVisible ? "green" : "amber"));
  });
  (r.installs || []).forEach((i) => {
    const label = i.installed
      ? i.target + " " + (i.enabled === false ? "disabled" : "installed")
      : i.target + " not installed";
    out.push(badge(label, i.installed && i.enabled !== false ? "green" : "amber"));
  });
  return out;
}

function recipeGuidance(r) {
  return (r.guidance || []).map((g) =>
    el("div", { class: "muted", style: "font-size:12px;margin-top:3px" }, [
      el("span", { class: "mono" }, [g.target + " next: "]),
      g.nextAction || "Check native plugin UI/CLI.",
    ])
  );
}

function recipeHasInstalled(r) {
  return (r.installs || []).some((i) => i.installed);
}
function recipeHasInstallableTarget(r) {
  return (r.installs || []).some((i) => !i.installed);
}
function recipeReady(r) {
  if (r.conflict || (r.missingSkills || []).length || !r.generated || r.stale) return false;
  if ((r.marketplaces || []).some((m) => !m.present || m.stale || !m.nativeVisible)) return false;
  return (r.installs || []).length > 0 && (r.installs || []).every((i) => i.installed && i.enabled !== false);
}
function recipeCard(r) {
  const targets = r.targets || [];
  const alerts = [];
  if (r.conflict) alerts.push(el("div", { class: "callout red" }, [r.conflict]));
  if ((r.missingSkills || []).length) alerts.push(el("div", { class: "callout amber" }, ["Missing skills: " + r.missingSkills.join(", ")]));
  const actions = [
    ...(!READONLY && recipeHasInstallableTarget(r) ? [btn("Install", () => recipeNativeAction(r, "install"), "primary")] : []),
    ...(!READONLY && recipeHasInstalled(r) ? [btn("Remove", () => recipeNativeAction(r, "remove"))] : []),
  ];
  return el("div", { class: "recipe-card" }, [
    el("div", { class: "recipe-head" }, [
      el("div", null, [
        el("div", null, [el("span", { class: "name" }, [r.display || r.name]), el("span", { class: "k" }, [r.name + " @ " + r.version])]),
        el("div", { class: "muted", style: "font-size:12px" }, [r.description || ""]),
      ]),
      el("div", { class: "row-actions" }, [
        ...actions,
        recipeReady(r) ? badge("ready", "green") : recipeStateBadge(r),
      ]),
    ]),
    el("div", { class: "recipe-meta" }, [
      badge("servers " + ((r.servers || []).length), "solid"),
      badge("skills " + ((r.skills || []).length), "solid"),
      badge("hooks " + ((r.hooks || []).length), "solid"),
      ...(targets.length ? targets.map((t) => badge(t, "")) : [badge("no targets", "amber")]),
    ]),
    ...alerts,
    el("div", { class: "target-steppers" }, targets.map((target) => targetStepper(r, target))),
    el("details", { class: "recipe-details" }, [
      el("summary", null, ["Paths and native guidance"]),
      el("div", { class: "muted mono", style: "font-size:11px;margin-top:8px" }, [r.packagePath || ""]),
      ...recipeGuidance(r),
    ]),
  ]);
}
function targetStepper(r, target) {
  const m = (r.marketplaces || []).find((x) => x.target === target) || {};
  const i = (r.installs || []).find((x) => x.target === target) || {};
  const steps = [
    stepModel("Recipe", r.conflict ? "blocked" : (r.missingSkills || []).length ? "warn" : "done", r.conflict ? "conflict" : (r.missingSkills || []).length ? "skills missing" : "valid"),
    stepModel("Package", !r.generated ? "pending" : r.stale ? "warn" : "done", !r.generated ? "missing" : r.stale ? "stale" : "generated"),
    stepModel("Entry", !m.present ? "pending" : m.stale ? "warn" : "done", !m.present ? "missing" : m.stale ? "stale" : "written"),
    stepModel("Native", m.nativeVisible ? "done" : "pending", m.nativeVisible ? "visible" : "hidden"),
    stepModel("Install", i.installed ? (i.enabled === false ? "warn" : "done") : "pending", i.installed ? (i.enabled === false ? "disabled" : "installed") : "not installed"),
  ];
  return el("div", { class: "target-stepper" }, [
    el("div", { class: "target-label" }, [target]),
    el("div", { class: "steps" }, steps.map((s) => el("div", { class: "step " + s.state, title: s.detail }, [
      el("span", { class: "step-dot" }, []),
      el("span", null, [s.label]),
    ]))),
  ]);
}
function stepModel(label, state, detail) {
  return { label, state, detail };
}
function recipeNativePlan(r, action) {
  const lines = (r.guidance || []).map((g) => g.target + ": " + (g.nextAction || "Check native plugin UI/CLI."));
  return lines.length ? lines : ["AgentStack will run the native harness command plan for selected recipe targets."];
}
function recipeNativeAction(r, action) {
  const path = action === "install" ? "/api/plugins_install" : "/api/plugins_remove";
  const title = (action === "install" ? "Install" : "Remove") + " native plugin";
  openOperationConfirm({
    title,
    detail: r.name + " @ " + r.version,
    items: recipeNativePlan(r, action),
    confirm: action === "install" ? "Install" : "Remove",
    run: () => post(path, { name: r.name }, "Plugin " + action),
  });
}

function plugins(c) {
  c.appendChild(pageHead("Plugins", "AgentStack recipes generate repo-local Claude Code and Codex plugin packages. Native installed plugins remain visible below."));
  const recipes = DATA.pluginRecipes || [];
  const list = DATA.plugins || [];
  const markets = DATA.marketplaces || [];

  if (!READONLY) {
    c.appendChild(el("div", { class: "toolbar", style: "margin-bottom:14px" }, [
      btn(PLUGIN_FORM ? "Close" : "+ Create recipe", () => { PLUGIN_FORM = !PLUGIN_FORM; show("plugins"); }, "primary"),
      btn("Sync recipes", () => post("/api/plugins_sync", {}, "Plugin recipe sync"), "primary"),
    ]));
    if (PLUGIN_FORM) c.appendChild(addPluginRecipeCard());
  }

  const rrows = recipes.map(recipeCard);
  if (!rrows.length) rrows.push(el("div", { class: "empty" }, ["No managed recipes. Add [plugins.*] to agentstack.toml."]));
  c.appendChild(el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Managed recipes", el("small", null, [plural(recipes.length, "recipe")])]),
    el("div", { class: "bd" }, rrows),
  ]));
  c.appendChild(el("div", { class: "muted", style: "font-size:12px;margin:10px 0 4px" }, [
    "Sync writes repo-local marketplaces and plugin packages. Install/trust still happens inside Codex or Claude Code, so the badges show both generated and native install state.",
  ]));

  const prows = list.map((p) => el("div", { class: "list-row" }, [
    el("span", null, [
      el("span", { class: "name" }, [p.name]),
      el("div", { class: "muted mono", style: "font-size:12px" }, [
        (p.harness || "unknown") + " · " + p.marketplace + (p.version ? " @ " + p.version : ""),
      ]),
      ...(p.source ? [el("div", { class: "muted mono", style: "font-size:11px" }, [p.source])] : []),
    ]),
    el("span", { class: "row-actions" }, [
      ...(p.projects || []).map((pr) => badge(pr, "solid")),
      badge(p.status || (p.enabled === false ? "disabled" : "installed"), p.enabled === false ? "" : "solid"),
      badge(p.scope || "local", ""),
    ]),
  ]));
  if (!prows.length) prows.push(el("div", { class: "empty" }, ["No native plugins found."]));
  c.appendChild(el("details", { class: "card acc", style: "margin-top:16px" }, [
    el("summary", { class: "hd acc-sum" }, ["Installed", el("small", { style: "display:inline" }, [plural(list.length, "plugin")])]),
    el("div", { class: "bd" }, prows),
  ]));

  const mrows = markets.map((m) => el("div", { class: "list-row" }, [
    el("span", { class: "name" }, [(m.harness || "unknown") + " · " + m.name]),
    el("span", { class: "muted mono", style: "font-size:12px" }, [m.source]),
  ]));
  if (!mrows.length) mrows.push(el("div", { class: "empty" }, ["No marketplaces."]));
  c.appendChild(el("details", { class: "card acc", style: "margin-top:16px" }, [
    el("summary", { class: "hd acc-sum" }, ["Marketplaces", el("small", { style: "display:inline" }, [`${markets.length}`])]),
    el("div", { class: "bd" }, mrows),
  ]));

  // Native extensions/add-ons (e.g. Pi extensions) — read-only.
  const exts = DATA.extensions || [];
  const erows = exts.map((e) => el("div", { class: "list-row" }, [
    el("span", null, [
      el("span", { class: "name", style: e.broken ? "opacity:.7" : "" }, [e.name]),
      el("div", { class: "muted mono", style: "font-size:12px" }, [e.harness + " · " + e.kind + " · " + (e.scope || "global") + (e.isSymlink ? " · symlink" : "")]),
    ]),
    el("span", { class: "row-actions" }, [e.broken ? badge("broken link", "red") : badge(e.scope || "global", "solid"), badge(e.kind, "")]),
  ]));
  if (!erows.length) erows.push(el("div", { class: "empty" }, ["No extensions installed (e.g. Pi's ~/.pi/agent/extensions is empty)."]));
  c.appendChild(el("details", { class: "card acc", style: "margin-top:16px" }, [
    el("summary", { class: "hd acc-sum" }, ["Extensions", el("small", { style: "display:inline" }, [`${exts.length} · Pi TypeScript add-ons`])]),
    el("div", { class: "bd" }, erows),
  ]));

  // Pi package marketplace (pi.dev/packages, via npm) — search + install.
  const input = el("input", { class: "inp", placeholder: "search Pi packages…", style: "width:280px", value: PI_MARKET.q });
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") doPiSearch(input.value.trim()); });
  const results = el("div", null, [piMarketBody()]);
  c.appendChild(el("details", { class: "card acc", style: "margin-top:16px" }, [
    el("summary", { class: "hd acc-sum" }, ["Pi marketplace", el("small", { style: "display:inline" }, ["pi.dev/packages · npm keyword pi-package"])]),
    el("div", { class: "bd" }, [
      el("div", { class: "toolbar", style: "margin-bottom:10px" }, [input, btn("Search", () => doPiSearch(input.value.trim()), "primary"), btn("Browse popular", () => doPiSearch(""))]),
      results,
    ]),
  ]));
}

const PI_MARKET = { q: "", results: null, loading: false };
function doPiSearch(query) {
  PI_MARKET.q = query;
  PI_MARKET.loading = true;
  if (SECTION === "plugins") show("plugins");
  return fetch(q("/api/pi_search") + "&q=" + encodeURIComponent(query))
    .then((r) => r.json())
    .then((d) => { PI_MARKET.loading = false; PI_MARKET.results = d.results || []; if (SECTION === "plugins") show("plugins"); })
    .catch((e) => { PI_MARKET.loading = false; toast("Pi search failed: " + e.message, false); });
}
function piMarketBody() {
  if (PI_MARKET.loading) return el("div", { class: "empty" }, ["Searching npm…"]);
  if (PI_MARKET.results == null) return el("div", { class: "empty" }, ["Search the Pi package marketplace, or “Browse popular”."]);
  if (!PI_MARKET.results.length) return el("div", { class: "empty" }, [`No Pi packages for "${PI_MARKET.q}".`]);
  const wrap = el("div");
  PI_MARKET.results.forEach((p) => {
    const head = el("div", { style: "display:flex;align-items:center;justify-content:space-between;gap:10px" }, [
      el("span", null, [el("span", { class: "name" }, [p.name]), el("span", { class: "k" }, [p.kind]), el("span", { class: "muted mono", style: "font-size:11px;margin-left:6px" }, [p.version])]),
      READONLY ? null : btn("Install", () => post("/api/pi_install", { name: p.name }, "Install " + p.name), "primary"),
    ]);
    const desc = el("div", { class: "muted", style: "font-size:12px;margin:2px 0 4px" }, [p.description || ""]);
    const cmd = el("div", { class: "muted mono", style: "font-size:11px" }, ["$ " + p.install + (p.repoUrl ? "   ·   " + p.repoUrl : "")]);
    wrap.appendChild(el("div", { style: "padding:9px 0;border-top:1px solid hsl(var(--border))" }, [head, desc, cmd]));
  });
  return wrap;
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
  const row = el("div", { class: "health-row" }, [
    el("span", { class: cls }, [mark]),
    el("span", null, [h.message]),
    h.action ? el("span", { class: "health-action" }, [btn(h.action.type === "preview" ? "Preview" : "Open", () => runAction(h.action))]) : null,
  ]);
  return row;
}
function health(c) {
  c.appendChild(pageHead("Health", "Your tools, secrets, and what's out of sync."));
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

// The trust lens — where a server/skill came from, secrets it needs, what gets
// written, and safety signals. Mirrors `agentstack explain` on the CLI.
function explainModal(name) {
  fetch(q("/api/explain") + "&name=" + encodeURIComponent(name))
    .then((r) => r.json())
    .then((d) => {
      if (d.error) return toast("Explain: " + d.error, false);
      const modal = el("div", { class: "modal" }, [
        el("div", { class: "mhd" }, [el("span", null, ["Explain · " + name]), btn("✕", closeModal, "icon")]),
        el("div", { class: "mbd" }, [el("pre", { class: "mono explain-text" }, [d.text.trim()])]),
      ]);
      document.getElementById("modal").appendChild(el("div", { class: "overlay", onclick: (e) => e.target.classList.contains("overlay") && closeModal() }, [modal]));
    })
    .catch((e) => toast("Explain: " + e.message, false));
}
function openOperationConfirm(plan) {
  const body = el("div", { class: "mbd" }, [
    el("div", { class: "muted", style: "font-size:13px;margin-bottom:10px" }, [plan.detail || ""]),
    el("div", { class: "op-list" }, (plan.items || []).map((item) => el("div", { class: "op-item mono" }, [item]))),
  ]);
  const footer = el("div", { class: "mft" }, [
    btn("Cancel", closeModal),
    btn(plan.confirm || "Continue", () => { closeModal(); plan.run && plan.run(); }, "primary"),
  ]);
  const modal = el("div", { class: "modal" }, [
    el("div", { class: "mhd" }, [el("span", null, [plan.title || "Confirm operation"]), btn("✕", closeModal, "icon")]),
    body, footer,
  ]);
  document.getElementById("modal").appendChild(el("div", { class: "overlay", onclick: (e) => e.target.classList.contains("overlay") && closeModal() }, [modal]));
}
function openPreview(scope, all) {
  fetch(q("/api/diff") + "&scope=" + scope + (all ? "&all=1" : ""))
    .then((r) => r.json())
    .then((data) => {
      const body = el("div", { class: "mbd" });
      const changed = (data.targets || []).filter((t) => t.changed);
      if (!changed.length) body.appendChild(el("div", { class: "empty" }, ["No changes — everything is already in sync."]));
      changed.forEach((t) => {
        const tag = all && !t.selectedByManifest ? el("span", { class: "badge amber", style: "margin-left:8px" }, ["non-default"]) : null;
        body.appendChild(el("div", { class: "diff-target" }, [`${t.display} · ${t.path}`, tag]));
        body.appendChild(colorizeDiff(t.diff));
      });
      // In "all" mode, apply exactly the drifted targets (incl. non-default);
      // otherwise apply the manifest's default set for this scope.
      const applyBody = all ? { scope, targets: changed.map((t) => t.id) } : { scope };
      const label = all ? "Apply all (" + changed.length + ")" : "Apply " + scope;
      const footer = el("div", { class: "mft" }, [
        btn("Cancel", closeModal),
        changed.length ? btn(label, () => { closeModal(); post("/api/apply", applyBody, label); }, "primary") : null,
      ]);
      const title = all ? `Preview · all installed targets · ${scope}` : `Preview · ${scope} scope`;
      const modal = el("div", { class: "modal" }, [
        el("div", { class: "mhd" }, [el("span", null, [title]), btn("✕", closeModal, "icon")]),
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
      if (data.needsInit) {
        document.getElementById("mode").textContent = "setup";
        document.getElementById("nav").innerHTML = "";
        renderScopeSwitch();
        renderPending();
        renderWelcome(data);
        return;
      }
      document.getElementById("mode").textContent = READONLY ? "read-only" : "read-write";
      renderNav();
      renderScopeSwitch();
      show(SECTION);
      refreshPending();
      refreshHistory();
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
function renderWelcome(data) {
  const c = document.getElementById("content");
  c.innerHTML = "";
  c.appendChild(pageHead("Welcome to agentstack", "No manifest yet in " + data.meta.dir));

  const detected = data.adapters.filter((a) => a.installed || a.configPresent);
  const rows = (detected.length ? detected : data.adapters).map((a) =>
    el("div", { class: "list-row" }, [
      el("span", { class: "name" }, [a.display]),
      a.installed ? badge("installed", "green") : a.configPresent ? badge("config found", "amber") : badge("not detected", ""),
    ])
  );
  c.appendChild(el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Detected agent CLIs", el("small", null, ["Initialize imports their MCP servers into one manifest and lifts secrets to your keychain."])]),
    el("div", { class: "bd" }, rows),
  ]));

  // Where the tools disagree today — the reason to unify.
  const mcpDetected = data.adapters.filter((a) => a.detected && a.mcp);
  const union = data.serverUnion || [];
  if (union.length && mcpDetected.length) {
    const disagree = union.filter((name) => {
      const have = mcpDetected.filter((a) => (a.servers || []).includes(name)).length;
      return have > 0 && have < mcpDetected.length;
    });
    const head = el("tr", null, [el("th", null, ["server"])]);
    mcpDetected.forEach((a) => head.appendChild(el("th", { class: "cell" }, [a.display])));
    const body = el("tbody");
    union.forEach((name) => {
      const tr = el("tr", null, [el("td", null, [el("span", { class: "name" }, [name])])]);
      mcpDetected.forEach((a) => {
        const on = (a.servers || []).includes(name);
        tr.appendChild(el("td", { class: "cell" }, [el("div", { class: on ? "on" : "off" }, [on ? "✓" : "–"])]));
      });
      body.appendChild(tr);
    });
    const note = disagree.length
      ? badge(plural(disagree.length, "server") + " not in every tool", "amber")
      : badge("your tools already agree", "green");
    c.appendChild(el("div", { class: "card", style: "margin-top:16px" }, [
      el("div", { class: "hd", style: "display:flex;align-items:center;gap:10px" }, [
        "What your tools have today",
        el("small", null, [disagree.length ? "Initialize makes this one source of truth" : "Initialize brings them under one manifest"]),
      ]),
      el("div", { class: "bd" }, [
        el("div", { style: "margin-bottom:10px" }, [note]),
        el("div", { class: "table-wrap" }, [el("table", null, [el("thead", null, [head]), body])]),
      ]),
    ]));
  }

  if (READONLY) {
    c.appendChild(el("div", { class: "muted", style: "margin-top:14px" }, ["Dashboard is read-only — run `agentstack init` in your terminal, or relaunch without --read-only."]));
    return;
  }
  if (!detected.length) {
    c.appendChild(el("div", { class: "muted", style: "margin-top:14px" }, ["No supported CLIs detected on this machine."]));
    return;
  }
  c.appendChild(el("div", { style: "margin-top:16px" }, [
    btn("Scan my CLIs & initialize ›", () =>
      post("/api/init", {}, "Initialized — review your servers, then Apply"), "primary"),
  ]));
}

document.getElementById("refresh").addEventListener("click", () => load());
document.getElementById("palette-btn").addEventListener("click", () => openPalette());
load();
