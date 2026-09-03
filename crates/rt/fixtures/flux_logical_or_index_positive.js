// Port of flux-rs tests/tests/pos/surface/binop.rs (logical or on bool indexes)
/*#rt type: (a: boolean[false], b: boolean[true]) => boolean[true] */
function logicalOrFt(a, b) {
  return a || b;
}

/*#rt type: (a: boolean[false], b: boolean[false]) => boolean[false] */
function logicalOrFf(a, b) {
  return a || b;
}

console.log(logicalOrFt(false, true), logicalOrFf(false, false));
