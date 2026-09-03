// Port of flux-rs tests/tests/pos/surface/operators.rs (y === -x)
/*#rt type: (x: number[x], y: number[y] | y === -x) => void */
function testNeg(x, y) {}

testNeg(1, -1);
console.log(true);
