/*#rt type: (x: number) => number | $ > 0 */
function missingNegativeGuard(x) {
  if (x > 0) {
    return x;
  }
  return x;
}

console.log(missingNegativeGuard(1));
