/*#own type: (buf: unique Buffer) => void */
function forget(buf) {}

/*#rt type: (x: number) => number | $ > 0 */
function incorrectlyPositive(x) {
  return x;
}

console.log(incorrectlyPositive(1));
