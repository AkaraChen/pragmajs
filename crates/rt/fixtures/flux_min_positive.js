// Port of flux-rs tests/tests/pos/surface/if-then-else.rs
// (index `if a < b { a } else { b }` becomes a predicate; no if-expressions in indexes)
/*#rt type: (a: number[a], b: number[b]) => number | ($ === a || $ === b) && $ <= a && $ <= b */
function min(a, b) {
  if (a <= b) {
    return a;
  }
  return b;
}

console.log(min(3, 5), min(8, 1));
