// Port of flux-rs tests/tests/pos/surface/loop01.rs (negative postcondition)
/*#rt type: () => number | $ < 0 */
function countUpNegative() {
  let i = 0;
  let res = 0;
  while (i < 100) {
    i = i + 1;
    res = res + 1;
  }
  return res;
}

console.log(countUpNegative());
