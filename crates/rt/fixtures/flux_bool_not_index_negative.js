// Port of flux-rs tests/tests/neg/surface/operators.rs (bool[!x] fails)
/*#rt type: (x: boolean[x], y: boolean[!x]) => void */
function testBoolNot(x, y) {}

testBoolNot(true, true);
