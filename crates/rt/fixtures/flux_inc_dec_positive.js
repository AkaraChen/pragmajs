// Port of flux-rs tests/tests/pos/surface/test00.rs
/*#rt type: (x: number[x]) => number | $ > x */
function inc(x) {
  return x + 1;
}

/*#rt type: (x: number[x]) => number | $ < x */
function dec(x) {
  return x - 1;
}

console.log(inc(3), dec(3));
