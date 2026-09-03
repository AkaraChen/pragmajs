// Port of flux-rs RVec[n] parameter indexes (ex2_min_index_loop / rvec length)
/*#rt type: (n: number[n], xs: DenseArray<number>[n]) => number[n] */
function denseLen(n, xs) {
  return xs.length;
}

console.log(denseLen(2, [1, 2]));
