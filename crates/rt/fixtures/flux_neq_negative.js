// Port of flux-rs tests/tests/neg/surface/operators.rs (x !== y fails)
/*#rt type: (x: number[x], y: number[y] | x !== y) => void */
function testNeq(x, y) {}

testNeq(0, 0);
