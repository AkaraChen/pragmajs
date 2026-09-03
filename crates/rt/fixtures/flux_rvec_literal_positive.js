// Port of flux-rs tests/tests/pos/surface/rvec00.rs (empty length + literal index)
/*#rt type: number | emptyLen === 0 */
const emptyLen = [].length;

/*#rt type: () => number[1] */
function sumPair() {
  const v = [0, 1];
  return v[0] + v[1];
}

console.log(emptyLen, sumPair());
