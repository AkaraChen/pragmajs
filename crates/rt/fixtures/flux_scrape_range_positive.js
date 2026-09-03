// Port of flux-rs tests/tests/pos/surface/scrape00.rs
/*#rt type: (lo: number[lo] | lo >= 0, hi: number[hi] | lo <= hi) => number[hi - lo] */
function rangeLen(lo, hi) {
  let i = lo;
  let res = 0;
  while (i < hi) {
    res = res + 1;
    i = i + 1;
  }
  return res;
}

console.log(rangeLen(2, 6));
