// Port of flux-rs tests/tests/pos/surface/arg_syntax.rs (`exists`)
/*#rt type: (x: number[x] | x > 0 && x < 10) => number | $ > 0 && $ < 11 */
function exists(x) {
  return x + 1;
}

console.log(exists(4));
