// Port of flux-rs tests/tests/pos/surface/ex2_min_index_loop.rs (neg post: index is not negative)
/*#rt type: () => number | $ < 0 */
function minIndexWrong() {
  const xs = [3, 1, 2];
  let res = 0;
  let i = 0;
  while (i < xs.length) {
    if (xs[i] < xs[res]) {
      res = i;
    }
    i = i + 1;
  }
  return res;
}

console.log(minIndexWrong());
