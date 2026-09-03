// Port of flux-rs tests/tests/pos/surface/operators.rs (x !== y)
/*#rt type: (x: number[x], y: number[y] | x !== y) => void */
function testNeq(x, y) {}

testNeq(0, 1);
console.log(true);
