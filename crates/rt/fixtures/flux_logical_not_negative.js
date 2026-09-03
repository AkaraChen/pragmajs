// Port of flux-rs tests/tests/neg/surface/binop.rs (logical not / or index)
/*#rt type: (a: boolean[false], b: boolean[true]) => boolean[false] */
function logicalOrFtWrong(a, b) {
  return a || b;
}

console.log(logicalOrFtWrong(false, true));
