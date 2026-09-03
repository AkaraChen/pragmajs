// Port of flux-rs RVec[n] (length does not match index)
/*#rt type: (n: number[n], xs: DenseArray<number>[n]) => number[n] */
function denseLen(n, xs) {
  return xs.length;
}

console.log(denseLen(3, [1, 2]));
