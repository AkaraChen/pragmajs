/*#rt type: (x: number | x > 0) => number | $ > 0 */
function mismatchedParameter(y) {
  return y;
}

console.log(mismatchedParameter(1));
