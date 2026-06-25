// agentstack dashboard — vanilla JS, no framework. Fetches the read-only
// snapshot and renders the cross-harness matrix + panels.
const token = new URLSearchParams(location.search).get("token") || "";

function el(tag, attrs, children) {
  const e = document.createElement(tag);
  if (attrs) for (const k in attrs) {
    if (k === "class") e.className = attrs[k];
    else if (k === "html") e.innerHTML = attrs[k];
    else e.setAttribute(k, attrs[k]);
  }
  (children || []).forEach((c) => e.appendChild(typeof c === "string" ? document.createTextNode(c) : c));
  return e;
}
const badge = (text, kind) => el("span", { class: "badge " + (kind || "") }, [text]);

let READONLY = false;

function toast(msg, ok) {
  const t = el("div", { class: "toast " + (ok ? "ok" : "err") }, [msg]);
  document.body.appendChild(t);
  setTimeout(() => t.remove(), 3200);
}

// POST a mutation, then refresh the snapshot.
function post(path, bodyObj, label) {
  return fetch(path + "?token=" + encodeURIComponent(token), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(bodyObj || {}),
  })
    .then((r) => r.json().then((d) => ({ ok: r.ok, d })))
    .then(({ ok, d }) => {
      if (!ok || d.error) throw new Error(d.error || "request failed");
      toast((label || "Done") + " ✓", true);
      load();
    })
    .catch((e) => toast((label || "Action") + ": " + e.message, false));
}

function btn(label, onClick) {
  const b = el("button", { class: "btn" }, [label]);
  b.addEventListener("click", onClick);
  return b;
}

function card(title, subtitle, bodyNodes) {
  const hd = el("div", { class: "hd" }, [el("h2", null, [title])]);
  if (subtitle) hd.appendChild(el("p", null, [subtitle]));
  return el("div", { class: "card" }, [hd, el("div", { class: "bd" }, bodyNodes)]);
}

function statCard(label, value, sub) {
  return el("div", { class: "card stat" }, [
    el("div", { class: "label" }, [label]),
    el("div", { class: "value" }, [String(value)]),
    el("div", { class: "sub" }, [sub || ""]),
  ]);
}

function matrix(data) {
  const head = el("tr", null, [el("th", null, ["capability"])]);
  data.adapters.forEach((a) => head.appendChild(el("th", { class: "cell" }, [a.display])));
  head.appendChild(el("th", null, ["type"]));
  const thead = el("thead", null, [head]);

  const rows = data.servers.map((s) => {
    const tr = el("tr", null, [
      el("td", null, [el("span", { class: "name" }, [s.name]), el("span", { class: "cap-kind" }, ["mcp"])]),
    ]);
    data.adapters.forEach((a) => {
      const cell = s.cells.find((c) => c.adapter === a.id) || {};
      let mark, tag = "";
      if (cell.global && cell.project) { mark = "✓"; tag = "both"; }
      else if (cell.global) { mark = "✓"; tag = "global"; }
      else if (cell.project) { mark = "✓"; tag = "project"; }
      else { mark = "–"; }
      const td = el("td", { class: "cell" }, []);
      td.appendChild(el("div", { class: mark === "✓" ? "on" : "off" }, [mark]));
      if (tag) td.appendChild(el("div", { class: "scope-tag" }, [tag]));
      tr.appendChild(td);
    });
    tr.appendChild(el("td", null, [badge(s.type, "solid")]));
    return tr;
  });

  if (rows.length === 0) rows.push(el("tr", null, [el("td", { colspan: data.adapters.length + 2 }, ["no servers in manifest"])]));
  return el("table", null, [thead, el("tbody", null, rows)]);
}

function secretSetter(name) {
  const input = el("input", { type: "password", placeholder: "value", class: "inp" });
  const b = btn("Set", () => {
    if (input.value) post("/api/secret", { name, value: input.value }, "Set " + name);
  });
  return el("span", { class: "setter" }, [input, b]);
}

function secretsPanel(data) {
  const rows = data.secrets.map((s) => {
    const right = s.resolved
      ? badge("resolved", "green")
      : READONLY
      ? badge("missing", "red")
      : secretSetter(s.name);
    return el("div", { class: "list-row" }, [el("span", { class: "mono" }, [s.name]), right]);
  });
  if (rows.length === 0) rows.push(el("div", { class: "muted" }, ["no secrets referenced"]));
  const resolved = data.secrets.filter((s) => s.resolved).length;
  return card("Secrets", `${resolved}/${data.secrets.length} resolved on this machine`, rows);
}

function skillsPanel(data) {
  const rows = data.skills.map((s) =>
    el("div", { class: "list-row" }, [
      el("span", null, [el("span", { class: "name" }, [s.name]), el("span", { class: "cap-kind" }, [s.source])]),
      s.installed ? badge("installed", "green") : badge("not installed", "amber"),
    ])
  );
  if (rows.length === 0) rows.push(el("div", { class: "muted" }, ["no skills in manifest"]));
  return card("Skills", `${data.skills.length} defined`, rows);
}

function statsPanel(data) {
  const max = Math.max(1, ...data.stats.map((s) => s.activations));
  const rows = data.stats.slice(0, 8).map((s) =>
    el("div", { class: "list-row" }, [
      el("span", { class: "name" }, [s.name]),
      el("div", { class: "bar-track" }, [el("div", { class: "bar", style: `width:${Math.round((s.activations / max) * 100)}%` }, [])]),
      el("span", { class: "muted" }, [String(s.activations)]),
    ])
  );
  if (rows.length === 0) rows.push(el("div", { class: "muted" }, ["no activations yet"]));
  return card("Usage", "activations (most used first)", rows);
}

function profilesPanel(data) {
  const rows = data.profiles.map((p) => {
    const meta = el("span", { class: "muted" }, [
      `${p.servers.length} server(s) · ${p.skills.length} skill(s)`,
    ]);
    const right = READONLY
      ? meta
      : el("span", { class: "row-actions" }, [
          meta,
          btn("activate ›", () =>
            post("/api/use", { profile: p.name, scope: "global" }, "Activate " + p.name)
          ),
        ]);
    return el("div", { class: "list-row" }, [el("span", { class: "name" }, [p.name]), right]);
  });
  if (rows.length === 0) rows.push(el("div", { class: "muted" }, ["no profiles"]));
  return card("Profiles", null, rows);
}

function toolbar() {
  return el("div", { class: "toolbar" }, [
    btn("Apply → global", () => post("/api/apply", { scope: "global" }, "Apply (global)")),
    btn("Apply → project", () => post("/api/apply", { scope: "project" }, "Apply (project)")),
    btn("Install", () => post("/api/install", {}, "Install")),
  ]);
}

function render(data) {
  READONLY = !!data.readOnly;
  document.getElementById("dir").textContent = data.meta.dir;
  document.getElementById("ver").textContent = "v" + data.meta.version;
  document.getElementById("mode").textContent = READONLY ? "read-only" : "read-write";
  const app = document.getElementById("app");
  app.innerHTML = "";

  const installed = data.adapters.filter((a) => a.installed).length;
  const secretsOk = data.secrets.filter((s) => s.resolved).length;
  app.appendChild(el("div", { class: "grid cols-4" }, [
    statCard("Harnesses", installed, `${data.adapters.length} known`),
    statCard("Servers", data.servers.length, "MCP"),
    statCard("Skills", data.skills.length, ""),
    statCard("Secrets", `${secretsOk}/${data.secrets.length}`, "resolved"),
  ]));

  if (!READONLY) app.appendChild(toolbar());

  app.appendChild(el("div", { class: "section-title" }, ["Cross-harness matrix"]));
  app.appendChild(el("div", { class: "card" }, [el("div", { class: "bd" }, [matrix(data)])]));

  app.appendChild(el("div", { class: "section-title" }, ["Capabilities"]));
  app.appendChild(el("div", { class: "grid cols-2" }, [secretsPanel(data), skillsPanel(data)]));
  app.appendChild(el("div", { class: "grid cols-2", style: "margin-top:16px" }, [profilesPanel(data), statsPanel(data)]));
}

function load() {
  return fetch("/api/state?token=" + encodeURIComponent(token))
    .then((r) => r.json())
    .then((data) => {
      if (data.error) throw new Error(data.error);
      render(data);
    })
    .catch((e) => {
      document.getElementById("app").innerHTML = "";
      document
        .getElementById("app")
        .appendChild(el("div", { class: "error" }, ["Failed to load: " + e.message]));
    });
}

load();
