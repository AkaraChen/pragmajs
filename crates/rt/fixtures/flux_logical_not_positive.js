// Port of flux-rs tests/tests/pos/surface/binop.rs (logical not)
/*#rt type: (a: boolean[true]) => boolean[false] */
function logicalNotT(a) {
  return !a;
}

/*#rt type: (a: boolean[false]) => boolean[true] */
function logicalNotF(a) {
  return !a;
}

console.log(logicalNotT(true), logicalNotF(false));
