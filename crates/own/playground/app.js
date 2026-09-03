import init, { check } from "./pkg/ownershipjs.js";

const EXAMPLES = {
  "unique move (ok)": `/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  consume(buf);
}

process(make());
`,
  "unique forget": `/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  // forgot to consume buf
}

process(make());
`,
  "console.log does not consume": `/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  console.log(buf);
  consume(buf);
}

process(make());
`,
  "Buffer#toString": `/*#own type: () => unique Buffer */
function make() {
  return Buffer.from("hi");
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  buf.toString();
  consume(buf);
}

process(make());
`,
  "FileHandle#close": `/*#own type: (fh: unique FileHandle) => void */
function process(fh) {
  fh.close();
}
`,
  "borrow then consume": `/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: &readonly Buffer) => void */
function read(buf) {}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  read(buf);
  consume(buf);
}

process(make());
`,
};

const sourceEl = document.getElementById("source");
const runtimeEl = document.getElementById("runtime");
const filenameEl = document.getElementById("filename");
const exampleEl = document.getElementById("example");
const outEl = document.getElementById("out");
const statusEl = document.getElementById("status");

for (const name of Object.keys(EXAMPLES)) {
  const opt = document.createElement("option");
  opt.value = name;
  opt.textContent = name;
  exampleEl.appendChild(opt);
}

sourceEl.value = EXAMPLES["unique move (ok)"];

function run() {
  const source = sourceEl.value;
  const runtime = runtimeEl.value;
  const filename = filenameEl.value;
  let diags;
  try {
    diags = JSON.parse(check(filename, source, runtime));
  } catch (e) {
    statusEl.className = "status err";
    statusEl.textContent = "checker crashed";
    outEl.textContent = String(e);
    return;
  }
  if (!diags.length) {
    statusEl.className = "status ok";
    statusEl.textContent = "no diagnostics";
    outEl.innerHTML = '<span class="empty">ok</span>';
    return;
  }
  statusEl.className = "status err";
  statusEl.textContent = diags.length === 1 ? "1 error" : `${diags.length} errors`;
  outEl.innerHTML = diags
    .map(
      (d) =>
        `<div class="diag">${filename}:${d.line}:${d.col}: error[${escapeHtml(d.kind)}]: ${escapeHtml(d.message)}</div>`
    )
    .join("");
}

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

let t = 0;
function schedule() {
  clearTimeout(t);
  t = setTimeout(run, 120);
}

sourceEl.addEventListener("input", schedule);
runtimeEl.addEventListener("change", run);
filenameEl.addEventListener("change", run);
exampleEl.addEventListener("change", () => {
  sourceEl.value = EXAMPLES[exampleEl.value] || "";
  run();
});

sourceEl.addEventListener("keydown", (e) => {
  if (e.key !== "Tab") return;
  e.preventDefault();
  const start = sourceEl.selectionStart;
  const end = sourceEl.selectionEnd;
  sourceEl.value = sourceEl.value.slice(0, start) + "  " + sourceEl.value.slice(end);
  sourceEl.selectionStart = sourceEl.selectionEnd = start + 2;
  schedule();
});

statusEl.textContent = "loading wasm…";
init()
  .then(() => {
    statusEl.textContent = "ready";
    run();
  })
  .catch((e) => {
    statusEl.className = "status err";
    statusEl.textContent = "failed to load wasm";
    outEl.textContent = String(e);
  });
