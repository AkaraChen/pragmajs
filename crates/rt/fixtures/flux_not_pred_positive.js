// Port of flux-rs tests/tests/pos/surface/operators.rs (!(x > 0))
/*#rt type: (x: number[x] | !(x > 0)) => void */
function testNot(x) {}

testNot(0);
console.log(true);
