// Adapted from flux-rs tests/tests/neg/surface/loop00.rs
// (n is not required non-negative, so the countdown need not finish at 0)
/*#rt type: (n: number[n]) => number[0] */
function downToZeroMaybeNegative(n) {
  let i = n;
  while (i >= 1) {
    i = i - 1;
  }
  return i;
}

console.log(downToZeroMaybeNegative(-1));
