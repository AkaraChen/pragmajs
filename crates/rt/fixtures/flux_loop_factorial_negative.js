/*#rt type: (n: number[n] | n >= 0) => number | $ >= 1 */
function factorialTooSmall(n) {
  let i = 0;
  let res = 0;
  while (i < n) {
    i = i + 1;
    res = res * i;
  }
  return res;
}

console.log(factorialTooSmall(4));
