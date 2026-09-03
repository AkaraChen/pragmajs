// Port of flux-rs tests/tests/pos/surface/literals00.rs
/*#rt type: (n: number[n]) => number | $ === n + 0xa */
function hex00(n) {
  return n + 10;
}

/*#rt type: (n: number[n]) => number | $ === n + 0xA */
function hex01(n) {
  return n + 10;
}

/*#rt type: (n: number[n]) => number | $ === n + 0x400 */
function hex02(n) {
  return n + 1000 + 0x18;
}

/*#rt type: (n: number[n]) => number | $ === n + 0o12 */
function octal00(n) {
  return n + 10;
}

/*#rt type: (n: number[n]) => number | $ === n + 0b1010 */
function binary0(n) {
  return n + 10;
}

console.log(hex00(1), hex01(1), hex02(1), octal00(1), binary0(1));
