const KEYWORDS = new Set([
  "break",
  "case",
  "catch",
  "const",
  "continue",
  "debugger",
  "default",
  "delete",
  "do",
  "else",
  "false",
  "finally",
  "for",
  "function",
  "if",
  "in",
  "instanceof",
  "let",
  "new",
  "null",
  "return",
  "switch",
  "this",
  "throw",
  "true",
  "try",
  "typeof",
  "undefined",
  "var",
  "void",
  "while",
  "with",
]);

const rail = document.querySelector("#rail");
const listEl = document.querySelector("#list");
const stage = document.querySelector("#stage");
const stats = document.querySelector("#stats");
const search = document.querySelector("#q");
const menu = document.querySelector("#menu");
const scrim = document.querySelector("#scrim");

let catalog = null;
let activeId = null;

function escapeHtml(text) {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function highlightLine(line) {
  const spec = line.indexOf("/*#rt");
  if (spec !== -1) {
    const end = line.indexOf("*/", spec);
    const before = tokenize(line.slice(0, spec));
    const mid = `<span class="tok-spec">${escapeHtml(
      end === -1 ? line.slice(spec) : line.slice(spec, end + 2),
    )}</span>`;
    const after = end === -1 ? "" : tokenize(line.slice(end + 2));
    return before + mid + after;
  }
  const comment = line.match(/^(\s*)(\/\/.*)$/);
  if (comment) {
    return `${escapeHtml(comment[1])}<span class="tok-cmt">${escapeHtml(comment[2])}</span>`;
  }
  return tokenize(line);
}

function tokenize(text) {
  const re =
    /("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\/\/.*|\b\d[\da-fA-Fxo.]*\b|\b[A-Za-z_$][\w$]*\b)/g;
  let last = 0;
  let out = "";
  for (const match of text.matchAll(re)) {
    out += escapeHtml(text.slice(last, match.index));
    const token = match[0];
    let cls = "";
    if (token.startsWith("//")) cls = "tok-cmt";
    else if (token.startsWith('"') || token.startsWith("'")) cls = "tok-str";
    else if (KEYWORDS.has(token)) cls = "tok-kw";
    else if (/^\d/.test(token)) cls = "tok-num";
    out += cls
      ? `<span class="${cls}">${escapeHtml(token)}</span>`
      : escapeHtml(token);
    last = match.index + token.length;
  }
  return out + escapeHtml(text.slice(last));
}

function diagnosticLines(output) {
  const lines = new Set();
  for (const match of output.matchAll(/:(\d+):\d+:/g)) {
    lines.add(Number(match[1]));
  }
  return lines;
}

function filters() {
  const polarity = document.querySelector('input[name="polarity"]:checked').value;
  const origin = document.querySelector('input[name="origin"]:checked').value;
  const q = search.value.trim().toLowerCase();
  return { polarity, origin, q };
}

function visibleCases() {
  const { polarity, origin, q } = filters();
  return catalog.cases.filter((item) => {
    if (polarity !== "all" && item.polarity !== polarity) return false;
    if (origin !== "all" && item.origin !== origin) return false;
    if (!q) return true;
    const hay = [
      item.id,
      item.title,
      item.file,
      item.fluxSource ?? "",
      item.note ?? "",
      item.group,
    ]
      .join(" ")
      .toLowerCase();
    return hay.includes(q);
  });
}

function renderList() {
  const items = visibleCases();
  const counts = catalog.counts;
  stats.textContent = `${items.length} shown · ${counts.fluxRs} flux-rs · ${counts.refinejs} native · ${counts.positive} pos · ${counts.negative} neg`;

  const groups = new Map();
  for (const item of items) {
    const bucket = groups.get(item.group) ?? [];
    bucket.push(item);
    groups.set(item.group, bucket);
  }

  const html = [];
  for (const [group, bucket] of groups) {
    html.push(`<div class="group-label">${escapeHtml(group)}</div>`);
    for (const item of bucket) {
      const origin =
        item.origin === "flux-rs" ? "flux-rs" : "native";
      html.push(`
        <button type="button" class="case${item.id === activeId ? " active" : ""}" data-id="${item.id}">
          <span class="pill ${item.polarity}">${item.polarity.slice(0, 3)}</span>
          <span class="name">${escapeHtml(item.title)}</span>
          <span class="meta">${escapeHtml(item.file)} · ${origin}</span>
        </button>
      `);
    }
  }
  listEl.innerHTML = html.join("") || `<p class="empty" style="padding:1rem">No cases match.</p>`;
}

function renderCase(item) {
  const hits = diagnosticLines(item.output);
  const sourceLines = item.source.replace(/\n$/, "").split("\n");
  const code = sourceLines
    .map((line, index) => {
      const n = index + 1;
      const hit = hits.has(n) ? " hit" : "";
      return `<div class="line${hit}" id="L${n}"><span class="ln">${n}</span><span>${highlightLine(line) || "&nbsp;"}</span></div>`;
    })
    .join("");

  const outputHtml = escapeHtml(item.output).replace(
    /(^|\n)([^:\n]+:)(\d+)(:\d+:)/g,
    (_, lead, file, line, rest) =>
      `${lead}<button type="button" data-line="${line}">${file}${line}${rest}</button>`,
  );

  const flux = item.fluxSource
    ? `<span class="file">${escapeHtml(item.fluxSource)}</span>`
    : "";
  const status = item.ok
    ? `<span class="ok">verified</span>`
    : `<span class="bad">rejected</span>`;
  const expected = item.matched
    ? "matches expected polarity"
    : "does not match expected polarity";

  stage.innerHTML = `
    <div class="head">
      <h2>${escapeHtml(item.title)}</h2>
      <span class="pill ${item.polarity}">${item.polarity}</span>
      <span class="file">${escapeHtml(item.file)}</span>
      ${flux}
    </div>
    ${item.note ? `<p class="note">${escapeHtml(item.note)}</p>` : ""}
    <div class="grid">
      <section class="pane">
        <header>
          <span>source</span>
          <button type="button" class="copy" data-copy="source">copy</button>
        </header>
        <pre class="code">${code}</pre>
      </section>
      <section class="pane">
        <header>
          <span>${status} · ${expected}</span>
        </header>
        <pre class="diag">${outputHtml}</pre>
      </section>
    </div>
  `;

  stage.querySelector("[data-copy=source]")?.addEventListener("click", async () => {
    await navigator.clipboard.writeText(item.source);
  });
  stage.querySelectorAll("[data-line]").forEach((button) => {
    button.addEventListener("click", () => {
      document.querySelector(`#L${button.dataset.line}`)?.scrollIntoView({
        block: "center",
      });
    });
  });
}

function select(id, pushHash = true) {
  const item = catalog.cases.find((entry) => entry.id === id);
  if (!item) return;
  activeId = id;
  if (pushHash) {
    history.replaceState(null, "", `#${id}`);
  }
  renderList();
  renderCase(item);
  closeRail();
}

function closeRail() {
  rail.classList.remove("open");
  scrim.hidden = true;
}

function openRail() {
  rail.classList.add("open");
  scrim.hidden = false;
}

function currentVisible() {
  return visibleCases();
}

function neighbor(delta) {
  const items = currentVisible();
  if (!items.length) return;
  const index = items.findIndex((item) => item.id === activeId);
  const next = items[(index + delta + items.length) % items.length];
  select(next.id);
}

search.addEventListener("input", () => {
  renderList();
});
document.querySelectorAll('input[name="polarity"], input[name="origin"]').forEach(
  (input) => {
    input.addEventListener("change", () => {
      renderList();
      const items = currentVisible();
      if (items.length && !items.some((item) => item.id === activeId)) {
        select(items[0].id);
      }
    });
  },
);
listEl.addEventListener("click", (event) => {
  const button = event.target.closest("[data-id]");
  if (button) select(button.dataset.id);
});
menu.addEventListener("click", () => {
  if (rail.classList.contains("open")) closeRail();
  else openRail();
});
scrim.addEventListener("click", closeRail);
window.addEventListener("keydown", (event) => {
  if (event.target === search) return;
  if (event.key === "j" || event.key === "ArrowDown") {
    event.preventDefault();
    neighbor(1);
  } else if (event.key === "k" || event.key === "ArrowUp") {
    event.preventDefault();
    neighbor(-1);
  } else if (event.key === "/") {
    event.preventDefault();
    search.focus();
  }
});

const loaded = await fetch("./catalog.json").then((response) => {
  if (!response.ok) throw new Error("catalog.json missing — run node playground/generate.mjs");
  return response.json();
});
catalog = loaded;
const fromHash = location.hash.replace(/^#/, "");
const first =
  catalog.cases.find((item) => item.id === fromHash) ??
  catalog.cases.find((item) => item.origin === "flux-rs") ??
  catalog.cases[0];
renderList();
if (first) select(first.id, false);
