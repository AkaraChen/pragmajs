// Port of flux-rs tests/tests/neg/surface/operators.rs (y === -x fails)
/*#rt type: (x: number[x], y: number[y] | y === -x) => void */
function testNeg(x, y) {}

testNeg(1, 1);
