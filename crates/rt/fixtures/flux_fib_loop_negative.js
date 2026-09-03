// Port of flux-rs tests/tests/neg/surface/fib_loop.rs
/*#rt type: (n: number[n] | 0 < n) => number | 1 < $ */
function fibLoopTooBig(n) {
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

console.log(fibLoopTooBig(1));
