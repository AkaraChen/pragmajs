// Port of flux-rs tests/tests/neg/surface/test00.rs
/*#rt type: (x: number[x]) => number | $ < x */
function inc(x) {
  return x + 1;
}

console.log(inc(3));
