// Port of flux-rs tests/tests/pos/surface/fib_loop.rs
/*#rt type: (n: number[n] | 0 < n) => number | 0 < $ */
function fibLoop(n) {
  let k = n;
  let i = 1;
  let j = 1;
  while (k > 2) {
    const tmp = i + j;
    j = i;
    i = tmp;
    k = k - 1;
  }
  return i;
}

console.log(fibLoop(7));
