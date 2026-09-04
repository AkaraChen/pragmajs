/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  // forgot to consume buf
}

process(make());

/*#rt type: (x: number) => number | $ > 0 */
function incorrectlyPositive(x) {
  return x;
}

console.log(incorrectlyPositive(1));
