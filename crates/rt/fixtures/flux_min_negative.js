// Port of flux-rs tests/tests/neg/surface/if-then-else.rs
/*#rt type: (a: number[a], b: number[b]) => number | ($ === a || $ === b) && $ <= a && $ <= b */
function minWrong(a, b) {
  if (a <= b) {
    return b;
  }
  return a;
}

console.log(minWrong(3, 5));
