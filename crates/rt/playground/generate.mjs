#!/usr/bin/env node
/**
 * Snapshot `pragmajs check --target ecmascript` for every fixtures/flux_*.js
 * into playground/catalog.json. The playground is static: it does not run Z3.
 *
 * Usage (from this crate): node playground/generate.mjs
 */
import { spawn } from "node:child_process";
import { readdir, readFile, writeFile, access } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const crate = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.resolve(crate, "../..");
const fixturesDir = path.join(crate, "fixtures");
const binary = path.join(root, "target", "debug", "pragmajs");
const outPath = path.join(crate, "playground", "catalog.json");
const CONCURRENCY = 4;

const TITLES = {
  flux_arg_path_positive: "Indexed path increment",
  flux_assert_index_positive: "Singleton five / incr / boolean[true]",
  flux_assert_index_negative: "boolean[true] assert fails",
  flux_bool_not_index_positive: "boolean[!x] index",
  flux_bool_not_index_negative: "boolean[!x] with the wrong witness",
  flux_countdown_positive: "Countdown to number[0]",
  flux_countdown_negative: "Countdown without n >= 0",
  flux_dense_empty_pop_negative: "pop on empty dense array",
  flux_dense_index_positive: "Dense index in bounds",
  flux_dense_oob_negative: "Dense index out of bounds",
  flux_dense_param_positive: "DenseArray[n] parameter length",
  flux_dense_param_negative: "DenseArray[n] length mismatch",
  flux_dense_push_pop_positive: "push / pop update length",
  flux_double_positive: "double x+x with 0 < x",
  flux_exists_bound_positive: "Exists-style bound on x",
  flux_fib_loop_positive: "Fibonacci while-loop",
  flux_fib_loop_negative: "Fibonacci postcondition too strong",
  flux_inc_dec_positive: "inc / dec pre-post",
  flux_inc_dec_negative: "inc claiming $ < x",
  flux_index_param_positive: "boolean[0 < n] from a branch",
  flux_index_param_negative: "boolean[0 < n] on a non-positive",
  flux_index_singleton_positive: "number[10] singleton",
  flux_index_singleton_negative: "singleton index mismatch",
  flux_literals_hex_positive: "Hex / octal / binary in specs",
  flux_logical_not_positive: "logical not on bool indexes",
  flux_logical_not_negative: "false || true is not boolean[false]",
  flux_logical_or_index_positive: "logical or on bool indexes",
  flux_loop01_positive: "Count-up Houdini 0 <= res",
  flux_loop01_negative: "Count-up with a wrong post",
  flux_loop_dense_empty_pop_negative: "pop after a draining loop",
  flux_loop_dense_index_positive: "Walk dense array with i < length",
  flux_loop_dense_index_negative: "Index past a dense walk",
  flux_loop_factorial_positive: "Factorial loop postcondition",
  flux_loop_factorial_negative: "Factorial postcondition too strong",
  flux_min_index_positive: "min-index walk (no struct)",
  flux_min_index_negative: "min-index claiming $ < 0",
  flux_min_positive: "min as a predicate, not an if-index",
  flux_min_negative: "min that returns the max",
  flux_neq_positive: "!== on indexed numbers",
  flux_neq_negative: "!== that does not hold",
  flux_not_pred_positive: "!(x > 0) as a predicate",
  flux_not_pred_negative: "!(x > 0) that does not hold",
  flux_rvec_literal_positive: "Empty length + literal index",
  flux_rvec_oob_negative: "v[2] on a length-2 dense array",
  flux_rvec_push_get_positive: "push twice, return length 2",
  flux_rvec_push_get_negative: "get past the last push",
  flux_scrape_range_positive: "res == hi - lo via v == i - lo",
  flux_scrape_range_negative: "range length claimed as hi-lo+1",
  flux_unary_neg_positive: "y === -x",
  flux_unary_neg_negative: "y === -x with the wrong witness",
};

const NOTES = {
  flux_countdown_positive:
    "Adapted from loop00.rs: dropped toss()/i32::MAX; kept countdown-to-zero on a non-negative index.",
  flux_countdown_negative:
    "Adapted from loop00.rs: n is not required non-negative, so the countdown need not finish at 0.",
  flux_min_index_positive:
    "Dropped struct Bob. Loop-head heap havoc forgets a sz = xs.length snapshot, so the test is i < xs.length and the post is $ >= 0, not $ < 3.",
  flux_min_positive:
    "Flux writes min as an if-expression in the index. refinejs has no ?: in the subset, so the claim is a predicate on $.",
  flux_rvec_push_get_positive:
    "Bare [] is DenseArray<unknown>. The port seeds [0], pops, then pushes. Element values are not tracked; the return is v.length as number[2].",
  flux_scrape_range_negative:
    "The flux-rs negative disables scrape quals. The JS analogue keeps scrape and uses a wrong post (hi-lo+1) so the failure is a definite refinement error.",
  flux_loop_dense_empty_pop_negative:
    "Pins fail-closed heap havoc: after while (xs.length > 0) xs.pop(), a following pop is rejected.",
  flux_literals_hex_positive:
    "Predicate lexer accepts 0x / 0o / 0b, and Int === uses the logical integer sort so $ === n + 0xa proves.",
  flux_dense_param_positive:
    "Call-site array-literal length is pushed into assumptions so DenseArray[n] parameters prove.",
};

function groupOf(id) {
  if (/dense|rvec/.test(id)) return "Dense arrays";
  if (/loop|countdown|fib|scrape|factorial|min_index/.test(id)) return "Loops";
  if (/index|literals|exists|arg_path|assert_index|bool_not/.test(id)) {
    return "Indexed types";
  }
  if (
    /logical|unary|neq|not_pred|min_|inc_dec|double|assignment|boolean|nan|sqrt/.test(
      id,
    )
  ) {
    return "Operators";
  }
  if (/poly|typestate|core|hygiene|constant|path_/.test(id)) {
    return "Core refinements";
  }
  return "Subset and soundness";
}

function originOf(source) {
  const match = source.match(
    /flux-rs\s+(tests\/tests\/[^\s)]+\.rs)/,
  );
  if (match) {
    return { origin: "flux-rs", fluxSource: match[1] };
  }
  if (/Port of flux-rs|Adapted from flux-rs/.test(source)) {
    return { origin: "flux-rs", fluxSource: null };
  }
  return { origin: "refinejs", fluxSource: null };
}

function titleOf(id, source) {
  if (TITLES[id]) return TITLES[id];
  const comment = source
    .split("\n")
    .find((line) => line.startsWith("//"))
    ?.replace(/^\/\/\s*/, "")
    .replace(/^Port of flux-rs\s+/, "")
    .replace(/^Adapted from flux-rs\s+/, "");
  if (comment) return comment;
  return id.replace(/^flux_/, "").replace(/_/g, " ");
}

function stripLocalPaths(text) {
  return text.replaceAll(`${fixturesDir}/`, "").replaceAll(`${root}/`, "");
}

function runCheck(filePath) {
  return new Promise((resolve) => {
    const child = spawn(
      binary,
      ["check", "--target", "ecmascript", filePath],
      { cwd: root },
    );
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      resolve({
        ok: false,
        output: String(error),
        exitCode: -1,
      });
    });
    child.on("close", (code) => {
      const output = stripLocalPaths(
        [stdout.trim(), stderr.trim()].filter(Boolean).join("\n"),
      );
      resolve({
        ok: code === 0,
        output: output || "(no output)",
        exitCode: code ?? -1,
      });
    });
  });
}

async function mapLimit(items, limit, mapper) {
  const results = new Array(items.length);
  let next = 0;
  async function worker() {
    while (next < items.length) {
      const index = next++;
      results[index] = await mapper(items[index], index);
    }
  }
  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, () => worker()),
  );
  return results;
}

async function main() {
  try {
    await access(binary);
  } catch {
    console.error(`Missing ${binary}. Run cargo build first.`);
    process.exit(1);
  }

  const names = (await readdir(fixturesDir))
    .filter((name) => name.startsWith("flux_") && name.endsWith(".js"))
    .sort();

  const started = Date.now();
  const cases = await mapLimit(names, CONCURRENCY, async (name, index) => {
    const filePath = path.join(fixturesDir, name);
    const source = await readFile(filePath, "utf8");
    const id = name.replace(/\.js$/, "");
    const polarity = name.endsWith("_negative.js")
      ? "negative"
      : name.endsWith("_positive.js")
        ? "positive"
        : "other";
    const expectedOk = polarity === "positive";
    const { origin, fluxSource } = originOf(source);
    process.stderr.write(`[${index + 1}/${names.length}] ${name}\n`);
    const result = await runCheck(filePath);
    return {
      id,
      file: name,
      polarity,
      expectedOk,
      ok: result.ok,
      matched: result.ok === expectedOk,
      origin,
      fluxSource,
      group: groupOf(id),
      title: titleOf(id, source),
      note: NOTES[id] ?? null,
      source,
      output: result.output,
      exitCode: result.exitCode,
    };
  });

  const catalog = {
    generatedAt: new Date().toISOString(),
    command: "refinejs check --target ecmascript",
    note: "Precomputed snapshots. The playground does not run Z3 in the browser.",
    counts: {
      total: cases.length,
      positive: cases.filter((c) => c.polarity === "positive").length,
      negative: cases.filter((c) => c.polarity === "negative").length,
      fluxRs: cases.filter((c) => c.origin === "flux-rs").length,
      refinejs: cases.filter((c) => c.origin === "refinejs").length,
      mismatched: cases.filter((c) => !c.matched).length,
    },
    cases,
  };

  await writeFile(outPath, `${JSON.stringify(catalog, null, 2)}\n`);
  const elapsed = ((Date.now() - started) / 1000).toFixed(1);
  console.log(
    `Wrote ${cases.length} cases to playground/catalog.json in ${elapsed}s ` +
      `(${catalog.counts.mismatched} polarity mismatches).`,
  );
  if (catalog.counts.mismatched) {
    for (const item of cases.filter((c) => !c.matched)) {
      console.error(`mismatch ${item.file}: expected ok=${item.expectedOk}, got ${item.ok}`);
      console.error(item.output);
    }
    process.exit(1);
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
