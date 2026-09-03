/*#rt type: () => number | $ >= 0 */
function walkDense() {
  const xs = [1, 2, 3];
  let i = 0;
  let total = 0;
  for (; i < xs.length; i++) {
    total = total + xs[i];
  }
  return i;
}

console.log(walkDense());
