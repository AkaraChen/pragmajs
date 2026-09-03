// Port of flux-rs tests/tests/pos/surface/index00.rs (negative: assert boolean[true] fails)
/*#rt type: (b: boolean[true]) => void */
function assertTrue(b) {}

const x = 1;
const y = 2;
assertTrue(x === y);
