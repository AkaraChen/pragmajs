// Adapted from flux-rs tests/tests/neg/surface/scrape00.rs
// (wrong post: length is hi-lo, not hi-lo+1; the flux file disables scrape instead)
/*#rt type: (lo: number[lo] | lo >= 0, hi: number[hi] | lo <= hi) => number[(hi - lo) + 1] */
function rangeLenOffByOne(lo, hi) {
  let i = lo;
  let res = 0;
  while (i < hi) {
    res = res + 1;
    i = i + 1;
  }
  return res;
}

console.log(rangeLenOffByOne(2, 6));
