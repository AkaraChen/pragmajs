// Port of flux-rs tests/tests/pos/surface/operators.rs (bool[!x])
/*#rt type: (x: boolean[x], y: boolean[!x]) => void */
function testBoolNot(x, y) {}

testBoolNot(true, false);
console.log(true);
