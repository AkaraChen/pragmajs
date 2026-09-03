// Port of flux-rs tests/tests/pos/surface/arg_syntax.rs (`path`)
/*#rt type: (x: number[x]) => number[x + 1] */
function path(x) {
  return x + 1;
}

console.log(path(4));
