// Port of flux-rs tests/tests/pos/surface/index00.rs
/*#rt type: (b: boolean[true]) => void */
function assertTrue(b) {}

/*#rt type: () => number[5] */
function five() {
  const x = 2;
  const y = 3;
  return x + y;
}

/*#rt type: (n: number[n]) => number[n + 1] */
function incr(n) {
  return n + 1;
}

/*#rt type: () => number[6] */
function testIncr() {
  const a = five();
  const b = incr(a);
  assertTrue(b === 6);
  return b;
}

console.log(testIncr());
