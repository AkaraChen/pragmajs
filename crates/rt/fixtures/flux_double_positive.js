// Port of flux-rs tests/tests/pos/surface/test06.rs
/*#rt type: (x: number[x] | 0 < x) => number | x < $ */
function double(x) {
  return x + x;
}

console.log(double(3));
