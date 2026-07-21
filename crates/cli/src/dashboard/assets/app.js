// agentstack dashboard — vanilla JS, no framework. A read-only lens over a
// snapshot (/api/state): browse your stack, preview diffs, run doctor, watch
// runs and audited calls. Every change happens through the CLI — where a
// control would live, the dashboard shows the command to copy instead.
const token = new URLSearchParams(location.search).get("token") || "";
let DATA = null;
let SECTION = location.hash.slice(1) || "overview";
let SCOPE = "global"; // active scope for the matrix views, diff preview, pending bar
let PENDING = null; // { scope, targets } from /api/diff, drives the pending bar
let HISTORY = []; // recent apply events from /api/history
let HISTORY_LOADED = false;
let OPEN_SERVER = null;
let SORT_BY_COST = false; // servers table: manifest order vs context-cost desc

const SECTIONS = [
  { id: "overview", label: "Overview" },
  { id: "runs", label: "Runs", count: (d) => (d.runs || []).length },
  { id: "discover", label: "Discover" },
  { id: "servers", label: "Servers", count: (d) => d.servers.length },
  { id: "skills", label: "Skills", count: (d) => d.skills.length },
  { id: "settings", label: "Settings", count: (d) => (d.settingsAdapters || []).length },
  { id: "hooks", label: "Hooks", count: (d) => (d.hooks || []).length },
  { id: "extensions", label: "Extensions", count: (d) => (d.extensions || []).length },
  { id: "instructions", label: "Instructions", count: (d) => d.instructions.length },
  { id: "secrets", label: "Secrets", count: (d) => d.secrets.length },
  { id: "activity", label: "Activity" },
  { id: "proxy", label: "Proxy", count: (d) => ((d.proxy || {}).capabilities || []).length },
  { id: "insights", label: "Insights", count: (d) => ((d.optimize || {}).recommendations || []).length },
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

// A copyable CLI command. The dashboard reads; the terminal writes. This is the
// app's mono style plus click-to-copy — the equivalent of any control we removed.
function cmd(text) {
  const e = el("code", { class: "cmd", title: "Click to copy" }, [text]);
  e.addEventListener("click", () => {
    if (navigator.clipboard) navigator.clipboard.writeText(text).then(() => toast("Copied: " + text, true), () => {});
  });
  return e;
}
// `add skill` only accepts spelled paths (./dir, ../dir, /abs, ~/dir) —
// anything else parses as owner/repo. Discovered-skill sources are paths.
function spellPath(p) {
  return /^(\.\.?\/|\/|~\/)/.test(p) ? p : "./" + p;
}
// A labelled hint row: "To do X, run: <cmd>".
function cmdHint(prefix, command) {
  return el("div", { class: "cmd-hint muted" }, [prefix ? prefix + " " : null, cmd(command)]);
}

function runAction(action, fallbackSection) {
  if (!action) return fallbackSection ? show(fallbackSection) : null;
  if (action.type === "preview") return openPreview(action.scope || "global", !!action.all);
  // Everything else (section, or any legacy write action) just navigates.
  return show(action.section || fallbackSection || "overview");
}

function actionButton(model, cls) {
  if (!model) return null;
  const action = model.action || model;
  // The read-only dashboard has no write actions — a stray post action becomes
  // a plain navigation to its section rather than a dead button.
  return btn(model.label || "Open", () => runAction(action, action.section), cls);
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
const VIEWS = { overview, runs, discover, servers, skills, settings, hooks, extensions, instructions, secrets, activity, proxy: proxyPanel, insights, health };
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
  if (!DATA || DATA.needsInit) return;
  const seg = el("div", { class: "seg", role: "group", "aria-label": "Scope" });
  [["global", "Global"], ["project", "Project"]].forEach(([id, label]) => {
    seg.appendChild(el("button", {
      class: "seg-btn" + (SCOPE === id ? " active" : ""),
      title: id === "project" ? "View this project directory only" : "View your whole machine",
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
// The command that reconciles the current scope's drift.
function applyCmd(scope) {
  return scope === "project" ? "agentstack apply --scope project --write" : "agentstack apply --write";
}
function refreshPending() {
  if (!DATA || DATA.needsInit) { PENDING = null; renderPending(); return Promise.resolve(); }
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
  if (!DATA || DATA.needsInit) return;
  // An active session (started from the CLI) takes over the bar — informational.
  if (DATA.session) {
    const s = DATA.session;
    const loads = s.loads || [];
    const sub = s.scope + " scope" +
      (loads.length ? " · " + plural(loads.length, "skill") + " pulled" : "") + " · reverts when it ends";
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
        cmd("agentstack session end"),
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
        btn("Review →", () => openPreview(SCOPE)),
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
        btn("Activity", () => show("activity")),
      ]),
    ]));
  }
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

/* ---------- activity (apply history) ---------- */
function activity(c) {
  c.appendChild(pageHead("Activity", "Every apply is backed up. Restore your tools to how they were before any change with `agentstack restore`."));

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
        el("span", { style: "margin-left:auto" }, [cmd("agentstack session end")]),
      ]),
      el("div", { class: "bd" }, rows),
    ]));
  }
  if (!HISTORY.length) {
    c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [el("div", { class: "empty" }, ["No applies yet. When you apply changes from the CLI they show up here."])])]));
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
      h.undone ? badge("undone", "") : cmd("agentstack restore"),
    ]),
  ]));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, rows)]));
}

/* ---------- discover (browse providers) ---------- */
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

function trustBadges(t) {
  const out = [];
  if (t.namespaced) out.push(badge("verified namespace", "green"));
  if (t.runsCode) out.push(badge("runs code", "amber"));
  if (t.needsSecret) out.push(badge("needs secret", ""));
  return out;
}

function discover(c) {
  c.appendChild(pageHead("Discover", "Browse capabilities across the catalog and the official MCP Registry. Add one with `agentstack add from <id>` in your terminal, then review the pending change here."));

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
      r.installed ? badge("in stack", "green") : cmd("agentstack add from " + r.addId),
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
    el("div", { class: "sub" }, [sub || " "]),
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
    el("div", { class: "stack-col" }, [stackSummaryCard(installed), bridgeCard()]),
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
    body.push(el("div", { class: "empty compact" }, ["Stack is ready. Review any drift before applying it from the CLI."]));
  } else {
    actions.slice(0, 6).forEach((a) => body.push(nextActionRow(a)));
  }
  body.push(el("div", { class: "toolbar tight", style: "margin-top:12px" }, [
    btn("Review changes →", () => openPreview(SCOPE)),
    btn("Review all tools", () => openPreview(SCOPE, true)),
  ]));
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
  return el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Stack summary", el("small", null, [plural(installed, "detected harness", "detected harnesses")])]),
    el("div", { class: "bd" }, [
      summaryLine("Default targets", targets),
      summaryLine("Extensions", plural((DATA.extensions || []).length, "extension")),
      summaryLine("Instructions", plural(DATA.instructions.length, "fragment")),
      summaryLine("Hooks", plural((DATA.hooks || []).length, "hook")),
      summaryLine("Mode", "read-only"),
    ]),
  ]);
}

/* Zero-files bridge: connected harnesses + this project's trust state. A
   read-only mirror of `doctor`'s bridge section — granting trust is a terminal
   act (`agentstack trust .`), done from the CLI. */
function bridgeCard() {
  const b = DATA.bridge || { harnesses: [], trust: "untrusted" };
  const connected = b.harnesses.filter((h) => h.connected);
  const trustBadge =
    b.trust === "trusted" ? badge("trusted", "green") :
    b.trust === "changed" ? badge("manifest changed", "amber") :
    badge("untrusted", "");
  const body = [
    el("div", { class: "summary-line" }, [
      el("span", { class: "muted" }, ["This project"]),
      trustBadge,
    ]),
    el("div", { class: "summary-line" }, [
      el("span", { class: "muted" }, ["Harnesses connected"]),
      el("span", { class: "mono" }, [`${connected.length}/${b.harnesses.length}`]),
    ]),
  ];
  if (connected.length) {
    body.push(el("div", { style: "display:flex;flex-wrap:wrap;gap:6px;margin-top:8px" },
      connected.map((h) => badge(h.display, "solid"))));
  }
  const hint =
    !connected.length ? "Register the gateway once: agentstack gateway connect --all --write" :
    b.trust === "trusted" ? "This repo's stack loads automatically in connected harnesses — no per-repo files." :
    b.trust === "changed" ? "Manifest edited since trusted — review it, then: agentstack trust ." :
    "Bridge sessions here get control-plane tools only until: agentstack trust .";
  body.push(el("div", { class: "muted", style: "font-size:12px;margin-top:8px" }, [hint]));
  return el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Zero-files bridge", el("small", null, ["connect once · trust per repo"])]),
    el("div", { class: "bd" }, body),
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
    return el("div", { class: "list-row" }, [
      el("span", { class: "name" }, [p.name]),
      el("span", { class: "row-actions" }, [meta, cmd("agentstack use " + p.name + " --write")]),
    ]);
  });
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No profiles yet. Define one in the manifest to load a set of skills and servers together."]));
  return el("div", { class: "card" }, [el("div", { class: "hd", style: "display:flex;align-items:center" }, ["Profiles"]), el("div", { class: "bd" }, rows)]);
}

/* ---------- runs ---------- */
function fmtUptime(secs) {
  secs = Math.max(0, secs | 0);
  if (secs < 60) return secs + "s";
  if (secs < 3600) return Math.floor(secs / 60) + "m";
  return Math.floor(secs / 3600) + "h" + String(Math.floor((secs % 3600) / 60)).padStart(2, "0") + "m";
}
// The trust footprint of a run (or of everything, when runId is null): which
// tools its agent actually called through the gateway, with denials. Digests
// only travel over the wire — never argument values.
function callsModal(runId, label) {
  fetch(q("/api/calls") + (runId ? "&run=" + encodeURIComponent(runId) : ""))
    .then((r) => r.json())
    .then((d) => {
      const calls = d.calls || [];
      const byTool = {};
      calls.forEach((e) => {
        const k = e.server + "__" + e.tool;
        byTool[k] = byTool[k] || { ok: 0, error: 0, denied: 0 };
        byTool[k][e.outcome] = (byTool[k][e.outcome] || 0) + 1;
      });
      const names = Object.keys(byTool).sort();
      const rows = names.map((k) => el("tr", null, [
        el("td", null, [el("span", { class: "mono" }, [k])]),
        el("td", null, [String(byTool[k].ok)]),
        el("td", null, [String(byTool[k].error)]),
        el("td", null, [byTool[k].denied ? el("span", { style: "color:hsl(0 72% 50%)" }, [String(byTool[k].denied)]) : "0"]),
      ]));
      const bd = names.length
        ? el("div", { class: "table-wrap" }, [el("table", null, [
            el("thead", null, [el("tr", null, ["tool", "ok", "err", "denied by policy"].map((h) => el("th", null, [h])))]),
            el("tbody", null, rows),
          ])])
        : el("div", { class: "empty" }, [runId
            ? "No tool calls logged for this run yet. Calls appear as its agent uses servers through `agentstack mcp`."
            : "No tool calls logged yet. Calls appear as agents use servers through `agentstack mcp`."]);
      const modal = el("div", { class: "modal" }, [
        el("div", { class: "mhd" }, [el("span", null, ["Tool calls · " + (label || "all runs")]), btn("✕", closeModal, "icon")]),
        el("div", { class: "mbd" }, [
          bd,
          el("div", { class: "muted", style: "margin-top:10px;font-size:12px" }, ["Audit log records argument digests only — never values. Full log: ~/.agentstack/audit/calls.jsonl (`agentstack report calls`)."]),
        ]),
      ]);
      document.getElementById("modal").appendChild(el("div", { class: "overlay", onclick: (e) => e.target.classList.contains("overlay") && closeModal() }, [modal]));
    })
    .catch((e) => toast("Calls: " + e.message, false));
}
function runs(c) {
  const list = DATA.runs || [];
  c.appendChild(pageHead("Runs", "Live agent processes agentstack launched. Start one with `agentstack run <harness>`; stop any of them with `agentstack kill <id>`."));
  c.appendChild(el("div", { class: "toolbar", style: "margin-bottom:14px" }, [
    btn("All tool calls", () => callsModal(null, null)),
  ]));
  if (!list.length) {
    c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [
      el("div", { class: "empty" }, ["No live runs. Launch one from your terminal, e.g."]),
      el("pre", { class: "mono", style: "margin-top:8px" }, ["agentstack run claude-code --profile design"]),
    ])]));
    return;
  }
  const head = el("tr", null, ["harness", "pid", "uptime", "profile", "can reach", "directory", ""].map((h) => el("th", null, [h])));
  const body = el("tbody");
  list.forEach((r) => {
    const footprint = [];
    (r.servers || []).forEach((s) => footprint.push(badge(s, "solid")));
    (r.skills || []).forEach((s) => footprint.push(badge(s)));
    if (!footprint.length) footprint.push(el("span", { class: "muted" }, ["—"]));
    const profCell = r.profile
      ? el("span", null, [r.profile, r.revertsOnExit ? el("span", { class: "k" }, ["reverts on exit"]) : null])
      : el("span", { class: "muted" }, ["(current config)"]);
    body.appendChild(el("tr", null, [
      el("td", null, [el("span", { class: "name" }, [r.display || r.harness])]),
      el("td", null, [el("span", { class: "mono" }, [String(r.pid)])]),
      el("td", null, [fmtUptime(r.uptimeSecs)]),
      el("td", null, [profCell]),
      el("td", null, [el("div", { style: "display:flex;flex-wrap:wrap;gap:4px" }, footprint)]),
      el("td", null, [el("span", { class: "mono muted" }, [r.cwd])]),
      el("td", null, [el("div", { style: "display:flex;gap:6px;align-items:center" }, [
        btn("Calls", () => callsModal(r.id, r.display || r.harness)),
        cmd("agentstack kill " + r.id),
      ])]),
    ]));
  });
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd", style: "padding:6px 8px" }, [el("div", { class: "table-wrap" }, [el("table", null, [el("thead", null, [head]), body])])])]));
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

// One matrix cell shared by the servers & skills tables. Read-only: it shows
// where a capability is on (this scope, the other, or inherited) and carries an
// aria-label so the ✓/– glyph isn't the only signal.
function statusCell(cell, opts) {
  // The visible state follows the ACTIVE scope. In project view, a server that's
  // only on globally shows a faded ✓ ("inherited") — active here, not set at the
  // project level.
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
  td.setAttribute("aria-label", opts.label + ": " + stateText);
  if (opts.disabledTitle) td.title = opts.disabledTitle;
  return td;
}

function servers(c) {
  const d = DATA;
  // Only CLIs that actually support MCP get a column (Pi, etc. have none).
  const cols = d.adapters.filter((a) => a.mcp !== false);
  c.appendChild(pageHead("Servers", "Where each server is enabled, per tool and scope. Click a name to see its config. Change the wiring from the CLI — `agentstack use <profile> --write` or edit the manifest, then `agentstack apply --write`."));

  const head = el("tr", null, [el("th", null, ["capability"])]);
  cols.forEach((a) => head.appendChild(el("th", { class: "cell" }, [a.display])));
  head.appendChild(el("th", null, ["type"]));
  head.appendChild(el("th", {
    class: "clickable",
    title: "Estimated tokens each server's tool list adds to every session (measured by `agentstack report usage --live`). Click to sort.",
    onclick: () => { SORT_BY_COST = !SORT_BY_COST; show("servers"); },
  }, ["context" + (SORT_BY_COST ? " ↓" : "")]));

  const body = el("tbody");
  if (!d.servers.length) body.appendChild(el("tr", null, [el("td", { colspan: cols.length + 3 }, [el("span", { class: "empty" }, ["No servers yet. Add one with `agentstack add from <id>` or from the Discover tab."])])]));
  const rows = SORT_BY_COST
    ? [...d.servers].sort((a, b) => ((b.footprint || {}).estTokens || 0) - ((a.footprint || {}).estTokens || 0))
    : d.servers;
  rows.forEach((s) => {
    const tr = el("tr", { class: "clickable" }, [
      el("td", { onclick: () => { OPEN_SERVER = OPEN_SERVER === s.name ? null : s.name; show("servers"); } },
        [el("span", { class: "name" }, [s.name]), el("span", { class: "k" }, ["mcp"])]),
    ]);
    cols.forEach((a) => {
      const cell = s.cells.find((x) => x.adapter === a.id) || {};
      tr.appendChild(statusCell(cell, { label: `${s.name} for ${a.display}` }));
    });
    tr.appendChild(el("td", null, [badge(s.type, "solid")]));
    tr.appendChild(el("td", null, [s.footprint
      ? el("span", { class: "k", title: s.footprint.tools + " tool(s) — estimated context cost per session" }, [s.footprint.label])
      : el("span", { class: "empty", title: "unmeasured — run `agentstack report usage --live`" }, ["—"])]));
    body.appendChild(tr);
    if (OPEN_SERVER === s.name) body.appendChild(serverDetail(s, cols.length + 3));
  });
  c.appendChild(scopeLegend());
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd", style: "padding:6px 8px" }, [el("div", { class: "table-wrap" }, [el("table", null, [el("thead", null, [head]), body])])])]));
}

function serverDetail(s, span) {
  const kv = [];
  const add = (k, v) => kv.push(el("div", { class: "key" }, [k]), el("div", { class: "mono" }, [v]));
  add("type", s.type);
  add("context cost", s.footprint
    ? s.footprint.label + " across " + s.footprint.tools + " tool(s) per session"
    : "unmeasured — run `agentstack report usage --live`");
  if (s.url) add("url", s.url);
  if (s.command) add("command", s.command);
  if (s.args && s.args.length) add("args", s.args.join(" "));
  if (s.cwd) add("cwd", s.cwd);
  (s.headers || []).forEach((h) => add("header." + h.key, h.value));
  (s.env || []).forEach((e) => add("env." + e.key, e.value));
  return el("tr", { class: "detail" }, [el("td", { colspan: span }, [el("div", { class: "bd" }, [
    el("div", { class: "kv" }, kv),
    el("div", { class: "toolbar", style: "margin-top:10px;align-items:center" }, [
      btn("Explain trust ⓘ", () => explainModal(s.name)),
      cmd("agentstack remove " + s.name),
    ]),
  ])])]);
}

/* ---------- skills ---------- */
function skills(c) {
  c.appendChild(pageHead("Skills", "Where each skill is enabled, per tool. Install and wiring happen from the CLI — `agentstack install`, then `agentstack use <profile> --write`."));
  const adapters = DATA.skillAdapters || [];

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
        disabledTitle: !s.installed ? "not installed — run `agentstack install`" : null,
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

  if (DATA.skills.some((s) => !s.installed)) {
    c.appendChild(cmdHint("Some skill sources aren't installed —", "agentstack install"));
  }

  // Skills present on disk in your CLIs but not yet in the manifest. Every entry
  // is shown — valid ones are adoptable; broken/non-skill ones show a status.
  const found = (DATA.discoveredSkills || []).filter((d) => !d.inManifest);
  if (found.length) {
    const adoptable = found.filter((d) => d.valid !== false);
    const hd = ["Detected on disk", el("small", null, [`${found.length} found · ${adoptable.length} manageable`])];
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
          ok ? cmd("agentstack add skill " + spellPath(d.source) + " --write") : null,
        ]),
      ]);
    });
    c.appendChild(el("div", { class: "card", style: "margin-top:16px" }, [
      el("div", { class: "hd", style: "display:flex;align-items:center" }, hd),
      el("div", { class: "bd" }, [
        el("div", { class: "muted", style: "font-size:12px;margin-bottom:8px" }, ["Register a discovered skill where it is with `agentstack add skill <path> --write`, or move it into the central library with `agentstack lib add ./<dir> --name <name> --write`."]),
        ...rows,
      ]),
    ]));
  }
}

/* ---------- settings ---------- */
function getPath(obj, dotted) {
  return dotted.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

function settings(c) {
  c.appendChild(pageHead("Settings", "Each tool's current settings, read from its real config file, and which keys agentstack manages. Edit `[settings.<tool>]` in the manifest, then `agentstack apply --write`."));
  const adapters = DATA.settingsAdapters || [];
  if (!adapters.length) {
    c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [el("div", { class: "empty" }, ["No CLIs with a managed settings file yet."])])]));
    return;
  }
  adapters.forEach((a, i) => c.appendChild(settingsCard(a, i === 0)));
}

function settingsCard(a, open) {
  // Show the CLI's live settings file with manifest-managed keys overriding
  // (top-level ownership) — the effective view without any editing.
  const effective = JSON.parse(JSON.stringify(a.live || {}));
  Object.entries(a.current || {}).forEach(([k, v]) => { effective[k] = JSON.parse(JSON.stringify(v)); });
  const fields = a.fields || [];
  const managedKeys = new Set(Object.keys(a.current || {}));

  const body = [el("div", { class: "muted mono", style: "font-size:12px;margin-bottom:10px" }, [a.path])];

  // Group fields by their `group`, rendering each as a read-only value row.
  const groups = {};
  fields.forEach((f) => { (groups[f.group || "Other"] = groups[f.group || "Other"] || []).push(f); });
  Object.keys(groups).forEach((g) => {
    body.push(el("div", { class: "section-title", style: "margin:14px 0 6px" }, [g]));
    groups[g].forEach((f) => body.push(settingRow(f, effective, managedKeys)));
  });

  // Keys present in the file but not in our catalog.
  const known = new Set(fields.map((f) => f.key.split(".")[0]));
  const extras = Object.keys(effective).filter((k) => !known.has(k));
  if (extras.length) {
    body.push(el("div", { class: "muted", style: "margin-top:12px;font-size:12px" }, [
      "Other keys in the file (no control): " + extras.join(", "),
    ]));
  }

  body.push(el("div", { class: "section-title", style: "margin:14px 0 6px" }, ["Effective settings"]));
  const pre = el("pre", { class: "mono", style: "background:hsl(var(--muted));padding:10px;border-radius:8px;font-size:12px;overflow:auto;max-height:220px" });
  pre.textContent = Object.keys(effective).length ? JSON.stringify(effective, null, 2) : "(nothing set)";
  body.push(pre);
  body.push(cmdHint("Manage these keys by editing [settings." + a.id + "] in the manifest, then", "agentstack apply --write"));

  const managedCount = managedKeys.size;
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

function settingRow(f, effective, managedKeys) {
  const label = f.label || f.key;
  const managed = managedKeys.has(f.key.split(".")[0]);
  const raw = getPath(effective, f.key);
  const value = raw === undefined ? "—" : typeof raw === "object" ? JSON.stringify(raw) : String(raw);
  return el("div", { style: "display:flex;align-items:center;gap:12px;padding:5px 0;border-bottom:1px solid hsl(var(--border))" }, [
    el("div", { style: "width:300px" }, [
      el("span", { class: "name" }, [label]),
      el("div", { class: "muted mono", style: "font-size:11px" }, [f.key]),
    ]),
    el("span", { class: "mono", style: "min-width:120px" }, [value]),
    managed ? badge("managed", "solid") : el("span", { class: "muted", style: "font-size:11px" }, ["from tool"]),
    f.help ? el("span", { class: "muted", style: "font-size:11px;flex:1" }, [f.help]) : null,
  ]);
}

/* ---------- hooks ---------- */
function hooks(c) {
  c.appendChild(pageHead("Hooks", "Commands run at lifecycle events (PreToolUse, SessionStart, …), declared in the manifest and compiled into each harness's native hooks config on apply."));
  const list = DATA.hooks || [];
  const rows = list.map((h) => el("div", { class: "list-row" }, [
    el("span", null, [
      el("span", { class: "name" }, [h.name]),
      el("div", { class: "muted mono", style: "font-size:12px" }, [h.event + (h.matcher ? " · " + h.matcher : "") + " → " + h.command]),
    ]),
    el("span", { class: "row-actions" }, (h.targets || ["*"]).map((t) => badge(t, "solid"))),
  ]));
  if (!rows.length) rows.push(el("div", { class: "empty" }, ["No hooks yet. Add [hooks.*] in the manifest, then `agentstack apply --write`."]));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, rows)]));
}

/* ---------- extensions ---------- */
function extensions(c) {
  c.appendChild(pageHead("Extensions", "Native harness add-on code (e.g. Pi TypeScript extensions, OpenCode JS plugins) agentstack pins and delivers. Read-only."));

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
  c.appendChild(pageHead("Secrets", "Referenced ${REF}s and where they resolve on this machine. Values are never shown. Set a missing one with `agentstack secret set <REF>`."));
  const rows = DATA.secrets.map((s) => {
    const right = s.resolved
      ? badge("resolved · " + (s.source || "?"), "green")
      : el("span", { class: "row-actions" }, [badge("missing", "red"), cmd("agentstack secret set " + s.name)]);
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
    h.action ? el("span", { class: "health-action" }, [btn(h.action.type === "preview" ? "Preview" : "Open", () => runAction(h.action, h.action.section))]) : null,
  ]);
  return row;
}
function health(c) {
  c.appendChild(pageHead("Health", "Your tools, secrets, and what's out of sync."));
  c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, DATA.health.map(healthRow))]));
  // Full doctor — the same checks as `agentstack doctor`, rendered here so the
  // verify step never needs a terminal.
  c.appendChild(el("div", { class: "card", style: "margin-top:16px" }, [el("div", { class: "bd" }, [
    el("div", { class: "toolbar" }, [
      btn("Run doctor", runDoctor, "primary"),
      el("span", { class: "muted", style: "font-size:13px" }, ["Full check-up: manifest, adapters, secrets, drift, skills, content scan, reproducibility."]),
    ]),
    el("div", { id: "doctor-out" }),
  ])]));
  c.appendChild(el("div", { style: "margin-top:16px" }, [
    btn("Review changes → global", () => openPreview("global")),
  ]));
}
function runDoctor() {
  const out = document.getElementById("doctor-out");
  if (!out) return;
  out.innerHTML = "";
  out.appendChild(el("div", { class: "muted", style: "margin-top:10px" }, ["Running checks…"]));
  fetch(q("/api/doctor"))
    .then((r) => r.json())
    .then((d) => {
      if (d.error) { out.innerHTML = ""; return toast("Doctor: " + d.error, false); }
      out.innerHTML = "";
      const summary = d.errors + " error(s), " + d.warnings + " warning(s).";
      out.appendChild(el("div", { style: "margin-top:10px;font-weight:600" }, [summary]));
      (d.sections || []).forEach((s) => {
        if (!s.lines.length) return;
        out.appendChild(el("div", { class: "muted", style: "margin-top:12px;font-size:12px;text-transform:uppercase;letter-spacing:.04em" }, [s.title]));
        s.lines.forEach((l) => out.appendChild(healthRow({ level: l.level, message: l.msg })));
      });
    })
    .catch((e) => { out.innerHTML = ""; toast("Doctor: " + e.message, false); });
}

/* ---------- proxy (wire-cost report) ---------- */
// Right-aligned numeric cell/header — this is a metrics report, not a matrix,
// so the numbers line up on the right the way the CLI table does.
const numTh = (t, title) => el("th", { style: "text-align:right", title: title || null }, [t]);
const numTd = (t) => el("td", { style: "text-align:right" }, [typeof t === "string" ? t : String(t)]);

function proxyPanel(c) {
  c.appendChild(pageHead("Proxy", "The wire lens — what your loaded tools cost per turn, measured from real API traffic. Observe-only; nothing is injected."));
  const p = DATA.proxy || {};
  const caps = p.capabilities || [];
  if (!p.requests) {
    c.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [el("div", { class: "empty" }, [
      "No wire activity observed yet — start ",
      el("span", { class: "mono" }, ["agentstack proxy"]),
      " and point a harness at ",
      el("span", { class: "mono" }, ["ANTHROPIC_BASE_URL=http://127.0.0.1:8787"]),
      ", then reload.",
    ])])]));
    return;
  }

  c.appendChild(el("div", { class: "grid cols-3" }, [
    statCard("Tools / turn", p.totalTools || 0, "peak seen in one request"),
    statCard("Tokens / turn", p.totalLabel || "0", "weight of the tools block"),
    statCard("Requests", p.requests || 0, "observed on the wire"),
  ]));

  const head = el("tr", null, [
    el("th", null, ["capability"]),
    numTh("tools"),
    numTh("avg tokens/turn"),
    numTh("calls"),
    el("th", null, ["hint"]),
  ]);
  const body = el("tbody");
  caps.forEach((cap) => {
    const kind = cap.hint === "keep" ? "green" : cap.hint === "drop / lazy" ? "amber" : "";
    body.appendChild(el("tr", null, [
      el("td", null, [el("span", { class: "name" }, [cap.capability])]),
      numTd(cap.tools),
      numTd(cap.avgLabel),
      numTd(cap.calls),
      el("td", null, [badge(cap.hint, kind)]),
    ]));
  });
  c.appendChild(el("div", { class: "card", style: "margin-top:16px" }, [
    el("div", { class: "bd", style: "padding:6px 8px" }, [el("div", { class: "table-wrap" }, [el("table", null, [el("thead", null, [head]), body])])]),
  ]));
  c.appendChild(el("div", { class: "muted", style: "font-size:12px;margin-top:10px" }, [
    "Loaded vs called — a capability whose tools cost the most tokens/turn but were never called this window is the first candidate to drop or make lazy.",
  ]));
}

/* ---------- insights (analyze + optimize + stats, stacked) ---------- */
function insights(c) {
  c.appendChild(pageHead("Insights", "Read-only analysis of your stack. Mirrors `agentstack optimize`, `analyze`, and `stats` — recommendations, runtime call activity, and per-capability usage."));
  c.appendChild(el("div", { class: "section-title" }, ["Optimize"]));
  c.appendChild(optimizeCard());
  c.appendChild(el("div", { class: "section-title" }, ["Analyze"]));
  c.appendChild(analyzeCard());
  c.appendChild(el("div", { class: "section-title" }, ["Stats"]));
  c.appendChild(statsCard());
}

function optimizeCard() {
  const o = DATA.optimize || {};
  const recs = o.recommendations || [];
  const body = [];
  if (o.gatewayCalls === 0) {
    body.push(el("div", { class: "callout amber" }, [
      "The audit log is empty — recommendations are limited to static signals. Use the gateway (zero-files bridge or `agentstack run`) to collect runtime evidence.",
    ]));
  }
  if (!recs.length) {
    body.push(el("div", { class: "empty" }, ["Nothing to recommend — your stack looks lean."]));
  } else {
    recs.forEach((r) => body.push(optimizeRow(r)));
  }
  const safe = recs.filter((r) => r.safe_auto).length;
  const sub = recs.length
    ? plural(recs.length, "recommendation") + " · " + safe + " safe to auto-apply"
    : "no recommendations";
  return el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Recommendations", el("small", null, [sub])]),
    el("div", { class: "bd" }, body),
  ]);
}

function optimizeRow(r) {
  const kind = r.impact === "high" ? "red" : r.impact === "medium" ? "amber" : "";
  return el("div", { class: "list-row", style: "flex-direction:column;align-items:stretch;gap:4px" }, [
    el("div", { style: "display:flex;align-items:center;gap:8px" }, [
      badge(r.impact || "low", kind),
      el("span", { class: "k", style: "margin-left:0" }, [r.kind]),
      el("span", { class: "name" }, [r.title || r.kind]),
    ]),
    ...(r.evidence || []).map((e) => el("div", { class: "muted", style: "font-size:12px" }, ["• " + e])),
    r.action ? el("div", { class: "mono", style: "font-size:12px;white-space:pre-wrap;margin-top:2px;color:hsl(var(--foreground))" }, [r.action]) : null,
    el("div", { class: "muted", style: "font-size:12px" }, [
      (r.safe_auto ? "safe with --write — " : "needs review — ") + (r.safety || ""),
    ]),
  ]);
}

function analyzeCard() {
  const a = DATA.analyze || {};
  const calls = a.calls || {};
  const dw = a.dead_weight || {};
  const subLabel = (t) => el("div", { class: "muted", style: "margin:14px 0 8px;font-size:12px;text-transform:uppercase;letter-spacing:.04em" }, [t]);
  const body = [];

  // Call activity — brokered gateway calls from the runtime audit log.
  if (!calls.total) {
    body.push(el("div", { class: "empty" }, ["No brokered calls recorded yet — the runtime gateway logs them when you use `agentstack run` / `agentstack mcp`."]));
  } else {
    body.push(el("div", { class: "row-actions" }, [
      badge(plural(calls.total, "call"), "solid"),
      badge("ok " + (calls.ok || 0), "green"),
      calls.error ? badge("error " + calls.error, "red") : null,
      calls.denied ? badge("denied " + calls.denied, "amber") : null,
      el("span", { class: "muted", style: "font-size:12px" }, ["over " + (calls.span_days ? calls.span_days + "d" : "today")]),
    ]));
    const servers = calls.by_server || [];
    const tools = calls.by_tool || [];
    if (servers.length) {
      body.push(subLabel("Top servers"));
      servers.forEach((s) => body.push(el("div", { class: "list-row" }, [
        el("span", { class: "name" }, [s.server]),
        el("span", { class: "row-actions" }, [
          s.errors ? badge(plural(s.errors, "error/denied", "error/denied"), "amber") : null,
          el("span", { class: "muted" }, [plural(s.calls, "call")]),
        ]),
      ])));
    }
    if (tools.length) {
      body.push(subLabel("Top tools"));
      tools.forEach((t) => body.push(el("div", { class: "list-row" }, [
        el("span", { class: "mono", style: "font-size:12px" }, [t.tool]),
        el("span", { class: "muted" }, [plural(t.calls, "call")]),
      ])));
    }
  }

  // Library dead weight — capabilities carried but never used anywhere.
  const skills = dw.skills || [];
  const servers = dw.servers || [];
  body.push(subLabel("Library dead weight"));
  if (!skills.length && !servers.length) {
    body.push(el("div", { class: "empty compact" }, ["Nothing unused — or nothing installed in the central library yet."]));
  } else {
    skills.forEach((s) => body.push(el("div", { class: "list-row" }, [
      el("span", { class: "name" }, [s.name]),
      badge("skill · never activated", ""),
    ])));
    servers.forEach((s) => body.push(el("div", { class: "list-row" }, [
      el("span", null, [
        el("span", { class: "name" }, [s.name]),
        s.est_tokens != null ? el("span", { class: "k" }, ["~" + s.est_tokens + " tok/session"]) : null,
      ]),
      badge("server · never called", "amber"),
    ])));
  }

  return el("div", { class: "card" }, [
    el("div", { class: "hd" }, ["Call activity & dead weight", el("small", null, ["runtime gateway audit log + library"])]),
    el("div", { class: "bd" }, body),
  ]);
}

function statsCard() {
  const s = DATA.statsReport || {};
  const caps = s.capabilities || [];
  if (!caps.length) {
    return el("div", { class: "card" }, [el("div", { class: "bd" }, [el("div", { class: "empty" }, ["No usage recorded yet. Apply changes or start a profile to record activations."])])]);
  }
  const max = Math.max(1, ...caps.map((x) => x.activations || 0));
  const head = el("tr", null, [
    el("th", null, ["capability"]),
    numTh("activations"),
    el("th", null, ["context cost"]),
    numTh("live in", "how many target/scope slots it's rendered into"),
  ]);
  const body = el("tbody");
  caps.forEach((x) => {
    const cost = x.costLabel ? x.costLabel + " (" + (x.tools || 0) + " tools)" : "—";
    body.appendChild(el("tr", null, [
      el("td", null, [el("span", { class: "name" }, [x.name]), x.deadWeight ? badge("dead weight", "amber") : null]),
      el("td", { style: "text-align:right" }, [el("div", { style: "display:flex;align-items:center;gap:8px;justify-content:flex-end" }, [
        el("div", { class: "bar-track", style: "max-width:80px;margin:0" }, [el("div", { class: "bar", style: `width:${Math.round(((x.activations || 0) / max) * 100)}%` })]),
        el("span", { class: "muted" }, [String(x.activations || 0)]),
      ])]),
      x.costLabel ? el("td", null, [el("span", { class: "k", style: "margin-left:0" }, [cost])]) : el("td", null, [el("span", { class: "off" }, ["—"])]),
      numTd(x.liveSlots || 0),
    ]));
  });
  const note = s.anyMeasured ? null : el("div", { class: "muted", style: "font-size:12px;margin-top:8px" }, [
    "Context cost unmeasured — run `agentstack report usage --live` to measure each server's tools/list footprint.",
  ]);
  return el("div", { class: "card" }, [el("div", { class: "bd", style: "padding:6px 8px" }, [
    el("div", { class: "table-wrap" }, [el("table", null, [el("thead", null, [head]), body])]),
    note,
  ])]);
}

/* ---------- diff preview modal (read-only) ---------- */
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
// Read-only diff preview: shows exactly what an apply would change, with the
// command to run it. No apply happens from here.
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
      const footer = el("div", { class: "mft" }, [
        changed.length ? el("span", { class: "muted", style: "margin-right:auto;display:flex;align-items:center;gap:8px" }, ["Apply from the CLI:", cmd(applyCmd(scope))]) : null,
        btn("Close", closeModal, "primary"),
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
      document.getElementById("dir").textContent = data.meta.dir;
      if (data.needsInit) {
        document.getElementById("mode").textContent = "setup";
        document.getElementById("nav").innerHTML = "";
        renderScopeSwitch();
        renderPending();
        renderWelcome(data);
        return;
      }
      document.getElementById("mode").textContent = "read-only";
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
    el("div", { class: "hd" }, ["Detected agent CLIs", el("small", null, ["`agentstack init` imports their MCP servers into one manifest and lifts secrets to your keychain."])]),
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

  if (!detected.length) {
    c.appendChild(el("div", { class: "muted", style: "margin-top:14px" }, ["No supported CLIs detected on this machine."]));
    return;
  }
  c.appendChild(el("div", { style: "margin-top:16px" }, [
    cmdHint("Scan your CLIs and create a manifest with", "agentstack init"),
  ]));
}

document.getElementById("refresh").addEventListener("click", () => load());
document.getElementById("palette-btn").addEventListener("click", () => openPalette());
load();
