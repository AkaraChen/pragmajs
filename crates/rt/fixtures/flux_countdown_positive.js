// Adapted from flux-rs tests/tests/pos/surface/loop00.rs
// (dropped toss()/i32::MAX; kept countdown-to-zero on a non-negative index)
/*#rt type: (n: number[n] | n >= 0) => number[0] */
function downToZero(n) {
  let i = n;
  while (i >= 1) {
    i = i - 1;
  }
  return i;
}

console.log(downToZero(4));
