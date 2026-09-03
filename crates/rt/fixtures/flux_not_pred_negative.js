// Port of flux-rs tests/tests/neg/surface/operators.rs (!(x > 0) fails)
/*#rt type: (x: number[x] | !(x > 0)) => void */
function testNot(x) {}

testNot(1);
