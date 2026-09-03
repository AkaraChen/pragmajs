#!/usr/bin/env node
/**
 * Inventory + prelude generator driven by Corsa (tsgo) through @corsa-bind/napi.
 * Does not use the TypeScript 5 (Strada) compiler API.
 */
"use strict";

const fs = require("fs");
const path = require("path");

const UNIQUE = new Set([
  "Buffer", "NonSharedBuffer", "AllowSharedBuffer", "SlowBuffer", "Uint8Array",
  "FileHandle", "Dir", "ReadStream", "WriteStream", "FSWatcher", "StatWatcher",
  "ChildProcess", "Socket", "Server", "ClientRequest", "IncomingMessage",
  "ServerResponse", "Agent", "Hash", "Hmac", "Cipher", "Decipher", "Sign",
  "Verify", "KeyObject", "X509Certificate", "Gzip", "Gunzip", "Deflate",
  "Inflate", "Interface", "REPLServer", "MessagePort", "Worker", "Blob", "File",
  "ReadableStream", "WritableStream", "TransformStream", "Request", "Response",
  "FormData", "Headers", "BunFile", "Subprocess", "FsFile", "HttpServer",
  "BufferView",
  "HttpClient", "DynamicLibrary", "FsWatcher", "Command", "Child", "TcpConn",
  "TlsConn", "UnixConn", "UdpConn", "Listener", "Conn", "Rid", "SQL",
  "RedisClient", "HTMLRewriter", "ServerWebSocket", "Readable", "Writable",
  "Duplex", "Transform", "DatabaseSync", "StatementSync",
]);

const CONSUME_SELF = /^(close|destroy|releaseLock|free)$/;
const MUT_SELF =
  /^(write|fill|copy|end|cork|uncork|pause|resume|setEncoding|swap16|swap32|swap64|copyWithin|sort|reverse|set|push|unshift|shift|pop|splice|appendFile|truncate|chmod|chown|utimes|writeFile|writev|sync|datasync)$/;

function isIdent(s) {
  return typeof s === "string" && /^[A-Za-z_][A-Za-z0-9_]*$/.test(s);
}
function sanitizeIdent(s, fb) {
  s = String(s || "").replace(/[^\w]/g, "");
  if (!s) return fb;
  if (!/^[A-Za-z_]/.test(s)) s = "p" + s;
  return s;
}
function unwrapTs(t) {
  t = String(t || "any").replace(/\s+/g, " ").trim();
  t = t.replace(/^readonly\s+/, "");
  let prev;
  do {
    prev = t;
    t = t.replace(/^Promise<(.+)>$/, "$1").trim();
  } while (t !== prev);
  if (t.startsWith("typeof ")) t = t.slice(7).trim();
  t = t.replace(/\[\]$/, "").replace(/<.*$/, "");
  t = t.split("|")[0].split("&")[0].trim();
  t = t.replace(/[^A-Za-z0-9_]/g, "");
  if (!t || t === "undefined" || t === "null" || t === "never") return "void";
  return t;
}
function isUniqType(t) {
  return (
    UNIQUE.has(t) ||
    t === "Buffer" ||
    /BufferView|Uint8Array|ChildProcess/.test(t) ||
    /(?:Stream|Handle|Server|Socket|Watcher|Conn|Listener)$/.test(t)
  );
}

function ownKind(tsText, role, paramName, meth) {
  const t = unwrapTs(tsText);
  if (t === "void") return "void";
  if (role === "param" && /^(path|file|filename|src|dest|oldp|newp|target)$/i.test(paramName || "")) {
    return `copy ${isIdent(t) ? t : "Path"}`;
  }
  if (role === "param" && /^(fd|rid)$/i.test(paramName || "")) {
    const fd = t === "number" ? "Fd" : t;
    if (meth && (CONSUME_SELF.test(meth) || /^close/i.test(meth))) return `unique ${fd}`;
    return `&readonly ${fd}`;
  }
  if (isUniqType(t)) {
    if (role === "param") {
      if (meth && CONSUME_SELF.test(meth) && paramName === "this") return `unique ${t}`;
      if (meth && (MUT_SELF.test(meth) || /^write/i.test(meth))) return `&mut ${t}`;
      if (meth && /^read/i.test(meth) && /Buffer|Uint8Array|ArrayBuffer/.test(t)) return `&mut ${t}`;
      return `&readonly ${t}`;
    }
    return `unique ${t}`;
  }
  return `copy ${isIdent(t) ? t : "any"}`;
}

function retKind(tsText, thisOwn, meth) {
  const k = ownKind(tsText, "ret");
  if (meth && /^(clone|duplicate|copy)$/i.test(meth)) return k;
  if (thisOwn && thisOwn.startsWith("&") && k.startsWith("unique ")) {
    const rt = k.slice("unique ".length);
    const thisT = thisOwn.replace(/^&(?:readonly|mut)\s+/, "");
    if (rt === thisT) return `copy ${rt}`;
    if (
      thisT === "Buffer" &&
      meth &&
      /^(slice|subarray|valueOf)$/i.test(meth) &&
      (rt === "Buffer" || rt === "Uint8Array" || /Buffer/.test(rt))
    ) {
      return `copy ${rt}`;
    }
  }
  return k;
}
function receiverOwn(typeName, method) {
  const t = isIdent(typeName) ? typeName : "any";
  if (CONSUME_SELF.test(method) && (UNIQUE.has(t) || /Handle|File|Stream|Conn|Socket|Server|Watcher/.test(t))) {
    return `unique ${t}`;
  }
  if (MUT_SELF.test(method) || /^write[A-Z]/.test(method)) return `&mut ${t}`;
  return `&readonly ${t}`;
}

/** JS callee prefix for a `declare module` specifier: keep underscores. */
function jsPrefix(specifier) {
  return specifier.replace(/^node:/, "").replace(/\//g, ".");
}

function declaredModules(nodeRoot) {
  const bySpec = new Map();
  const re = /declare\s+module\s+["']([^"']+)["']/g;
  for (const file of collectDts(nodeRoot)) {
    if (/[/\\]ts5\.[67][/\\]/.test(file) || /[/\\]web-globals[/\\]/.test(file)) continue;
    const text = fs.readFileSync(file, "utf8");
    let m;
    const local = new RegExp(re.source, "g");
    while ((m = local.exec(text))) {
      if (!bySpec.has(m[1])) bySpec.set(m[1], file);
    }
  }
  return bySpec;
}

function typeNamesIn(file) {
  const text = fs.readFileSync(file, "utf8");
  const names = new Set();
  const re =
    /\b(?:export\s+)?(?:declare\s+)?(?:abstract\s+)?(?:class|interface)\s+([A-Z][A-Za-z0-9_]*)/g;
  let m;
  while ((m = re.exec(text))) names.add(m[1]);
  return [...names];
}

function collectDts(root) {
  const out = [];
  function walk(d) {
    if (!fs.existsSync(d)) return;
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      if (e.name === "docs" || e.name === "vendor") continue;
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith(".d.ts")) out.push(p);
    }
  }
  walk(root);
  return out;
}

function utf16(text, index) {
  return text.slice(0, index).length;
}

class CorsaGen {
  constructor(client, snapshot, project) {
    this.client = client;
    this.snapshot = snapshot;
    this.project = project;
    this.callables = new Map();
    this.stop = [];
    this.seenTypes = new Set();
    this.seenNames = new Set();
  }
  sid(v) {
    if (v == null) return null;
    if (typeof v === "object") return String(v.id ?? v.handle);
    return String(v);
  }
  addStop(name, reason) {
    this.stop.push({ name, reason });
  }
  addFn(name, params, ret, thisOwn) {
    if (this.callables.has(name)) return;
    const parts = [];
    if (thisOwn) parts.push(`this: ${thisOwn}`);
    const meth = name.split(/[.#]/).pop();
    params.forEach((p, i) => {
      let k = ownKind(p.type, "param", p.name, meth);
      if (
        /^(compare|equals)$/.test(meth) &&
        k.startsWith("unique ") &&
        p.name !== "this"
      ) {
        k = "&readonly " + k.slice("unique ".length);
      }
      parts.push(`${sanitizeIdent(p.name, "p" + i)}: ${k}`);
    });
    this.callables.set(name, `${name} (${parts.join(", ")}) => ${retKind(ret, thisOwn, meth)}`);
  }
  typeOfSymbol(id) {
    return this.client.getTypeOfSymbol(this.snapshot, this.project, this.sid(id));
  }
  declaredType(id) {
    return this.client.getDeclaredTypeOfSymbol(this.snapshot, this.project, this.sid(id));
  }
  props(typeId) {
    const r = this.client.callJson("getPropertiesOfType", {
      snapshot: this.snapshot,
      project: this.project,
      type: this.sid(typeId),
    });
    return Array.isArray(r) ? r : [];
  }
  sigs(typeId, kind) {
    const r = this.client.callJson("getSignaturesOfType", {
      snapshot: this.snapshot,
      project: this.project,
      type: this.sid(typeId),
      kind,
    });
    return Array.isArray(r) ? r : [];
  }
  typeStr(typeId) {
    try {
      return this.client.typeToString(this.snapshot, this.project, this.sid(typeId));
    } catch {
      return "any";
    }
  }
  retOf(sig) {
    const r = this.client.callJson("getReturnTypeOfSignature", {
      snapshot: this.snapshot,
      project: this.project,
      signature: this.sid(sig.id ?? sig),
    });
    if (!r) return "void";
    if (r.intrinsicName) return r.intrinsicName;
    return this.typeStr(r.id ?? r);
  }
  paramsOf(sig) {
    const names = (sig.parameterSymbols || []).map((s) => s.name || "p");
    const texts = sig.parameterTypeTexts || [];
    return names.map((name, i) => ({
      name,
      type: Array.isArray(texts[i]) ? texts[i][0] : texts[i] || "any",
    }));
  }
  emitCall(name, typeId, thisOwn) {
    const call = this.sigs(typeId, 0);
    if (!call.length) return false;
    let best = call[0];
    let bestRet = this.retOf(best);
    for (const s of call) {
      const r = this.retOf(s);
      if (/Buffer|Uint8Array|FileHandle|Stream|Socket|Server|Request|Response|Blob|BunFile|FsFile/.test(r)) {
        best = s;
        bestRet = r;
        break;
      }
    }
    this.addFn(name, this.paramsOf(best), bestRet, thisOwn);
    return true;
  }
  walkType(typeId, prefix, mode, depth) {
    if (depth > 6 || !typeId) return;
    const tid = this.sid(typeId);
    const key = mode + ":" + prefix + ":" + tid;
    if (this.seenTypes.has(key)) return;
    this.seenTypes.add(key);
    let props;
    try {
      props = this.props(tid);
    } catch {
      return;
    }
    for (const p of props) {
      try {
        if (!p || !p.name) continue;
        if (!isIdent(p.name)) {
          this.addStop(
            `${prefix}${mode === "inst" ? "#" : prefix ? "." : ""}${p.name}`,
            "computed / symbol member"
          );
          continue;
        }
        const dotted =
          mode === "inst"
            ? `${prefix}#${p.name}`
            : prefix
              ? `${prefix}.${p.name}`
              : p.name;
        const t = this.typeOfSymbol(p.id);
        if (!t || !t.id) continue;
        const ctor = this.sigs(t.id, 1);
        if (ctor.length) {
          this.addStop(
            `${dotted} constructor`,
            "construct signature (`new`); NewExpression is not a prelude call"
          );
          try {
            const inst = this.declaredType(p.id);
            if (inst && inst.id) this.walkType(inst.id, p.name, "inst", depth + 1);
          } catch {
            /* ignore */
          }
          this.walkType(t.id, p.name, "static", depth + 1);
          continue;
        }
        if (this.emitCall(dotted, t.id, mode === "inst" ? receiverOwn(prefix, p.name) : null)) {
          continue;
        }
        const flags = p.flags || 0;
        if (flags & (32 | 64)) {
          try {
            const inst = this.declaredType(p.id);
            if (inst && inst.id) this.walkType(inst.id, p.name, "inst", depth + 1);
          } catch {
            /* ignore */
          }
        }
        const objectish = (t.flags & 1048576) !== 0;
        const tooDeep = dotted.split(".").length >= 3;
        if (mode !== "inst" && depth < 4 && objectish && !tooDeep) {
          this.walkType(t.id, dotted, "ns", depth + 1);
        }
      } catch {
        continue;
      }
    }
  }
}

function writeProbe(dir, kind, typesRoot) {
  fs.mkdirSync(dir, { recursive: true });
  const lines = [];
  const markers = [];
  let seq = 0;
  function add(expr, meta) {
    const id = "p" + seq++;
    markers.push({ id, expr, ...meta });
    lines.push(`export const ${id}: ${expr};`);
  }
  let tsconfig;
  if (kind === "node") {
    const mods = declaredModules(path.join(typesRoot, "node"));
    for (const [spec, file] of mods) {
      const prefix = jsPrefix(spec);
      add(`typeof import(${JSON.stringify(spec)})`, {
        kind: "ns",
        prefix,
        spec,
      });
      for (const tn of typeNamesIn(file)) {
        if (tn === "Buffer" || tn.endsWith("Constructor")) continue;
        add(`import(${JSON.stringify(spec)}).${tn}`, {
          kind: "inst",
          prefix: tn,
          spec,
        });
      }
    }
    add("Buffer", { kind: "inst", prefix: "Buffer", spec: "Buffer" });
    add("typeof Buffer", { kind: "static", prefix: "Buffer", spec: "Buffer" });
    add("typeof console", { kind: "ns", prefix: "console", spec: "console" });
    add("typeof process", { kind: "ns", prefix: "process", spec: "process" });
    add("typeof JSON", { kind: "ns", prefix: "JSON", spec: "JSON" });
    add("typeof process.stdout", { kind: "ns", prefix: "process.stdout", spec: "process.stdout" });
    add("typeof process.stderr", { kind: "ns", prefix: "process.stderr", spec: "process.stderr" });
    for (const g of [
      "setTimeout",
      "setInterval",
      "setImmediate",
      "clearTimeout",
      "clearInterval",
      "clearImmediate",
      "queueMicrotask",
      "fetch",
      "atob",
      "btoa",
    ]) {
      add(`typeof ${g}`, { kind: "fn", prefix: g, spec: g });
    }
    tsconfig = {
      compilerOptions: {
        target: "ES2022",
        module: "commonjs",
        types: ["node"],
        skipLibCheck: true,
        noEmit: true,
      },
      include: ["probe.ts"],
    };
  } else if (kind === "bun") {
    add("typeof Bun", { kind: "ns", prefix: "Bun", spec: "bun" });
    add("typeof console", { kind: "ns", prefix: "console", spec: "console" });
    for (const f of collectDts(path.join(typesRoot, "bun"))) {
      for (const tn of typeNamesIn(f)) {
        if (tn.startsWith("_") || tn === "Bun") continue;
        add(`import("bun").${tn}`, { kind: "inst", prefix: tn, spec: "bun" });
      }
    }
    tsconfig = {
      compilerOptions: {
        target: "ES2022",
        module: "esnext",
        types: ["bun-types"],
        skipLibCheck: true,
        noEmit: true,
      },
      include: ["probe.ts"],
    };
  } else {
    lines.unshift('/// <reference path="./lib.deno.ns.d.ts" />');
    add("typeof Deno", { kind: "ns", prefix: "Deno", spec: "Deno" });
    add("typeof console", { kind: "ns", prefix: "console", spec: "console" });
    add("typeof fetch", { kind: "fn", prefix: "fetch", spec: "fetch" });
    add("typeof setTimeout", { kind: "fn", prefix: "setTimeout", spec: "setTimeout" });
    tsconfig = {
      compilerOptions: {
        target: "ES2022",
        module: "esnext",
        skipLibCheck: true,
        noEmit: true,
        lib: ["es2022"],
      },
      include: ["probe.ts", "lib.deno.ns.d.ts"],
    };
  }
  fs.writeFileSync(path.join(dir, "probe.ts"), lines.join("\n") + "\n");
  fs.writeFileSync(path.join(dir, "tsconfig.json"), JSON.stringify(tsconfig, null, 2));
  return { text: lines.join("\n") + "\n", markers };
}

function extractRuntime(kind, typesRoot, tsgo, workRoot) {
  const dir = path.join(workRoot, "probe-" + kind);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  const nm = path.join(dir, "node_modules");
  fs.mkdirSync(path.join(nm, "@types"), { recursive: true });
  if (kind === "node") {
    fs.symlinkSync(path.join(typesRoot, "node"), path.join(nm, "@types", "node"));
  } else if (kind === "bun") {
    fs.symlinkSync(path.join(typesRoot, "bun"), path.join(nm, "bun-types"));
    fs.mkdirSync(path.join(nm, "@types"), { recursive: true });
  } else {
    fs.copyFileSync(path.join(typesRoot, "deno", "lib.deno.ns.d.ts"), path.join(dir, "lib.deno.ns.d.ts"));
  }
  const { text, markers } = writeProbe(dir, kind, typesRoot);
  const { CorsaApiClient } = require("@corsa-bind/napi");
  const client = CorsaApiClient.spawn({
    executable: tsgo,
    cwd: dir,
    mode: "msgpack",
    requestTimeoutMs: 120000,
  });
  try {
    client.initialize();
    const snap = client.updateSnapshot({ openProject: path.join(dir, "tsconfig.json") });
    if (!snap.projects || !snap.projects[0]) throw new Error("no Corsa project for " + kind);
    const gen = new CorsaGen(client, snap.snapshot, snap.projects[0].id);
    const file = path.join(dir, "probe.ts");
    for (const m of markers) {
      const needle = `const ${m.id}:`;
      const at = text.indexOf(needle);
      const pos = utf16(text, at < 0 ? text.indexOf(m.id) : at + "const ".length);
      const t = client.getTypeAtPosition(gen.snapshot, gen.project, file, pos);
      if (!t || !t.id) {
        gen.addStop(m.spec || m.id, `Corsa could not resolve import(${JSON.stringify(m.spec || m.expr)})`);
        continue;
      }
      if (m.kind === "inst") {
        gen.walkType(t.id, m.prefix, "inst", 0);
      } else if (m.kind === "fn") {
        if (!gen.emitCall(m.prefix, t.id, null)) {
          gen.walkType(t.id, m.prefix, "ns", 0);
        }
      } else {
        gen.walkType(t.id, m.prefix, m.kind === "static" ? "static" : "ns", 0);
      }
    }
    return gen;
  } finally {
    try {
      client.close();
    } catch {
      /* ignore */
    }
  }
}

function methOf(name) {
  if (name.includes("#")) return name.split("#").pop();
  const parts = name.split(".");
  return parts[parts.length - 1];
}

function splitParams(s) {
  return s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
}

function rewriteParamKind(ty, pname, meth) {
  if (/^(fd|rid)$/i.test(pname)) {
    const t = ty.replace(/^(unique|affine|copy|&readonly|&mut)\s+/, "") || "Fd";
    const fd = t === "number" ? "Fd" : t;
    if (CONSUME_SELF.test(meth) || /^close/i.test(meth)) return `unique ${fd}`;
    return `&readonly ${fd}`;
  }
  const m = ty.match(/^(unique|affine|copy|&readonly|&mut)\s+(.+)$/);
  if (!m) return ty;
  const kind = m[1];
  const t = m[2];
  if (kind !== "unique") return ty;
  if (pname === "this") return ty;
  if (MUT_SELF.test(meth) || /^write/i.test(meth)) return `&mut ${t}`;
  if (/^read/i.test(meth) && /Buffer|Uint8Array|ArrayBuffer/.test(t)) return `&mut ${t}`;
  if (isUniqType(t) || t === "ArrayBufferView") return `&readonly ${t}`;
  return ty;
}

function rewriteRetKind(retSrc, thisOwn, meth) {
  if (meth && /^(clone|duplicate|copy)$/i.test(meth)) {
    const m = retSrc.match(/^(unique|affine|copy|&readonly|&mut)\s+(.+)$/);
    if (m && !/^(void|any|boolean|number|string|bigint|symbol)$/.test(m[2])) {
      return `unique ${m[2]}`;
    }
    return retSrc;
  }
  if (!thisOwn || !thisOwn.startsWith("&")) return retSrc;
  const thisT = thisOwn.replace(/^&(?:readonly|mut)\s+/, "");
  if (thisT === "Buffer" && meth && /^(filter|map)$/i.test(meth)) {
    const t = retSrc.replace(/^(unique|affine|copy|&readonly|&mut)\s+/, "");
    if (t === "Uint8Array" || /Buffer/.test(t)) return `unique ${t}`;
  }
  if (!retSrc.startsWith("unique ")) return retSrc;
  const rt = retSrc.slice("unique ".length);
  if (rt === thisT) return `copy ${rt}`;
  if (
    thisT === "Buffer" &&
    meth &&
    /^(slice|subarray|valueOf)$/i.test(meth) &&
    (rt === "Buffer" || rt === "Uint8Array" || /Buffer/.test(rt))
  ) {
    return `copy ${rt}`;
  }
  return retSrc;
}

function rewriteOwnLine(line, instanceThis) {
  const trimmed = line.trim();
  if (!trimmed || trimmed.startsWith("#")) return line;
  const open = trimmed.indexOf("(");
  const arrow = trimmed.lastIndexOf("=>");
  if (open < 0 || arrow < 0) return line;
  const close = trimmed.lastIndexOf(")", arrow);
  if (close < open) return line;
  const name = trimmed.slice(0, open).trim();
  const paramsSrc = trimmed.slice(open + 1, close).trim();
  const retSrc = trimmed.slice(arrow + 2).trim();
  const meth = methOf(name);
  const params = paramsSrc ? splitParams(paramsSrc) : [];
  const newParams = params.map((p) => {
    const colon = p.indexOf(":");
    if (colon < 0) return p;
    const pname = p.slice(0, colon).trim();
    const ty = p.slice(colon + 1).trim();
    return `${pname}: ${rewriteParamKind(ty, pname, meth)}`;
  });
  let thisOwn = null;
  if (newParams[0] && newParams[0].startsWith("this:")) {
    thisOwn = newParams[0].slice(newParams[0].indexOf(":") + 1).trim();
  } else {
    const proto = name.indexOf(".prototype.");
    if (proto > 0) {
      const typeName = name.slice(0, proto);
      thisOwn = receiverOwn(typeName, meth);
    } else if (
      instanceThis &&
      retSrc.startsWith("unique ") &&
      name.split(".").length >= 3
    ) {
      const rt = retSrc.slice("unique ".length);
      thisOwn = instanceThis.get(`${rt}#${meth}`)
        || instanceThis.get(`${rt}.prototype.${meth}`)
        || null;
    }
  }
  const newRet = rewriteRetKind(retSrc, thisOwn, meth);
  return `${name} (${newParams.join(", ")}) => ${newRet}`;
}

function collectInstanceThis(lines) {
  const map = new Map();
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const open = trimmed.indexOf("(");
    const arrow = trimmed.lastIndexOf("=>");
    if (open < 0 || arrow < 0) continue;
    const close = trimmed.lastIndexOf(")", arrow);
    if (close < open) continue;
    const name = trimmed.slice(0, open).trim();
    if (!name.includes("#") && !name.includes(".prototype.")) continue;
    const paramsSrc = trimmed.slice(open + 1, close).trim();
    const params = paramsSrc ? splitParams(paramsSrc) : [];
    if (params[0] && params[0].startsWith("this:")) {
      const thisOwn = params[0].slice(params[0].indexOf(":") + 1).trim();
      map.set(name, thisOwn);
      const meth = methOf(name);
      const typeName = name.includes("#")
        ? name.slice(0, name.indexOf("#"))
        : name.slice(0, name.indexOf(".prototype."));
      map.set(`${typeName}#${meth}`, thisOwn);
      map.set(`${typeName}.prototype.${meth}`, thisOwn);
    }
  }
  return map;
}

function rewriteOwnFile(file) {
  const text = fs.readFileSync(file, "utf8");
  const lines = text.split("\n");
  const instanceThis = collectInstanceThis(lines);
  const out = lines
    .map((line) => {
      if (!line.trim() || line.trim().startsWith("#")) return line;
      return rewriteOwnLine(line, instanceThis);
    })
    .join("\n");
  fs.writeFileSync(file, out.endsWith("\n") ? out : out + "\n");
}

function main() {
  if (process.argv[2] === "rewrite-existing") {
    const dir = process.argv[3];
    if (!dir) {
      console.error("usage: gen-prelude.cjs rewrite-existing <preludesDir>");
      process.exit(2);
    }
    for (const name of ["node.own", "bun.own", "deno.own"]) {
      const f = path.join(dir, name);
      if (fs.existsSync(f)) rewriteOwnFile(f);
    }
    return;
  }
  const typesRoot = process.argv[2];
  const outDir = process.argv[3];
  const reportDir = process.argv[4];
  const tsgo = process.argv[5] || process.env.TSGO || process.env.CORSA_BIN;
  if (!typesRoot || !outDir || !reportDir || !tsgo) {
    console.error("usage: gen-prelude.cjs <typesRoot> <outDir> <reportDir> <tsgo>");
    process.exit(2);
  }
  fs.mkdirSync(outDir, { recursive: true });
  fs.mkdirSync(reportDir, { recursive: true });
  const workRoot = path.join(reportDir, "corsa-work");
  fs.mkdirSync(workRoot, { recursive: true });

  const header = (title) =>
    `# ${title}\n# Generated via Corsa (tsgo) through @corsa-bind/napi. Overloads collapsed.\n# Instance methods are Type#method with implicit this.\n`;

  console.error("extracting node via Corsa…");
  const node = extractRuntime("node", typesRoot, tsgo, workRoot);
  console.error("node callables", node.callables.size, "stop", node.stop.length);
  console.error("extracting bun via Corsa…");
  const bun = extractRuntime("bun", typesRoot, tsgo, workRoot);
  console.error("bun callables", bun.callables.size, "stop", bun.stop.length);
  console.error("extracting deno via Corsa…");
  const deno = extractRuntime("deno", typesRoot, tsgo, workRoot);
  console.error("deno callables", deno.callables.size, "stop", deno.stop.length);

  {
    const specs = declaredModules(path.join(typesRoot, "node"));
    const names = [...node.callables.keys()];
    for (const [spec, file] of specs) {
      const prefix = jsPrefix(spec);
      const hit =
        names.some((k) => k === prefix || k.startsWith(prefix + ".")) ||
        typeNamesIn(file).some((tn) =>
          names.some((k) => k.startsWith(tn + "#") || k.startsWith(tn + "."))
        );
      const stopped = node.stop.some(
        (s) => String(s.name) === spec || String(s.name).includes(spec)
      );
      if (!hit && !stopped) {
        node.addStop(spec, "no identifier callables (constants or types only)");
      }
    }
  }

  function writeOwn(name, title, gen) {
    const raw = [...gen.callables.keys()].sort().map((k) => gen.callables.get(k));
    const instanceThis = collectInstanceThis(raw);
    const lines = raw.map((line) => rewriteOwnLine(line, instanceThis));
    fs.writeFileSync(path.join(outDir, name), header(title) + lines.join("\n") + "\n");
  }
  writeOwn("node.own", "Node.js builtin ownership prelude", node);
  writeOwn("bun.own", "Bun builtins (loaded on top of Node)", bun);
  writeOwn("deno.own", "Deno namespace + shared globals", deno);

  const inv = [];
  for (const [rt, g] of [
    ["node", node],
    ["bun", bun],
    ["deno", deno],
  ]) {
    const names = [...g.callables.keys()].sort();
    inv.push(`# ${rt} callables=${names.length} stop=${g.stop.length}`);
    inv.push(
      "js-names: " +
        names
          .filter((s) =>
            /child_process\.spawn|worker_threads|async_hooks|sqlite|toString|#close|Buffer\.from|fs\.readFile|console\.log|Bun\.file|Deno\.readFile|DatabaseSync/.test(
              s
            )
          )
          .slice(0, 50)
          .join(", ")
    );
    inv.push("first20: " + names.slice(0, 20).join(", "));
    inv.push("");
  }
  fs.writeFileSync(path.join(reportDir, "inventory.txt"), inv.join("\n"));

  const stop = ["# stop-report: callables Corsa saw that cannot be identifier prelude entries\n"];
  for (const [rt, g] of [
    ["node", node],
    ["bun", bun],
    ["deno", deno],
  ]) {
    stop.push(`## ${rt}\n`);
    const by = new Map();
    for (const s of g.stop) {
      if (!by.has(s.reason)) by.set(s.reason, []);
      by.get(s.reason).push(s.name);
    }
    for (const [reason, names] of by) {
      const uniq = [...new Set(names)];
      stop.push(`### ${reason}\ncount=${uniq.length}\n${uniq.slice(0, 80).join("\n")}`);
      if (uniq.length > 80) stop.push(`… +${uniq.length - 80} more`);
      stop.push("");
    }
  }
  fs.writeFileSync(path.join(reportDir, "stop-report.md"), stop.join("\n"));

  function cov(rt, g) {
    const names = [...g.callables.keys()];
    const prelude = new Set();
    for (const line of fs.readFileSync(path.join(outDir, rt + ".own"), "utf8").split("\n")) {
      const t = line.trim();
      if (!t || t.startsWith("#")) continue;
      const i = t.indexOf("(");
      if (i > 0) prelude.add(t.slice(0, i).trim());
    }
    const missing = names.filter((n) => !prelude.has(n));
    return { n: names.length, p: prelude.size, missing };
  }
  const cn = cov("node", node);
  const cb = cov("bun", bun);
  const cd = cov("deno", deno);
  const nodeSpecs = declaredModules(path.join(typesRoot, "node"));
  const nodeNames = [...node.callables.keys()];
  const specGaps = [];
  for (const [spec, file] of nodeSpecs) {
    const prefix = jsPrefix(spec);
    const hit =
      nodeNames.some((k) => k === prefix || k.startsWith(prefix + ".")) ||
      typeNamesIn(file).some((tn) =>
        nodeNames.some((k) => k.startsWith(tn + "#") || k.startsWith(tn + "."))
      );
    const stopped = node.stop.some(
      (s) => String(s.name) === spec || String(s.name).includes(spec)
    );
    if (!hit && !stopped) specGaps.push(spec);
  }
  const coverage = [
    `# coverage  inventory vs prelude (Corsa/tsgo via corsa-bind)`,
    `# JS names from declare module specifiers (underscores kept; node: stripped)`,
    `node inventory=${cn.n} prelude=${cn.p} missing=${cn.missing.length}`,
    `bun  inventory=${cb.n} prelude=${cb.p} missing=${cb.missing.length}`,
    `deno inventory=${cd.n} prelude=${cd.p} missing=${cd.missing.length}`,
    `declare-module specs=${nodeSpecs.size} uncovered=${specGaps.length} ${specGaps.slice(0, 20).join(", ") || "(none)"}`,
    `child_process.spawn=${nodeNames.includes("child_process.spawn")}`,
    `sqlite/test/sea/quic/ffi/vfs prefixes=` +
      ["sqlite", "test", "sea", "quic", "ffi", "vfs"]
        .map((p) => `${p}:${nodeNames.some((k) => k === p || k.startsWith(p + ".") || k.startsWith(p + "#"))}`)
        .join(" "),
    `missing node: ${cn.missing.slice(0, 15).join(", ") || "(none)"}`,
    `missing bun: ${cb.missing.slice(0, 15).join(", ") || "(none)"}`,
    `missing deno: ${cd.missing.slice(0, 15).join(", ") || "(none)"}`,
    "",
  ].join("\n");
  fs.writeFileSync(path.join(reportDir, "coverage.txt"), coverage);
  console.log(coverage);
}

main();
