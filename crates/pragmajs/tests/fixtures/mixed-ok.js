/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  consume(buf);
}

/*#rt
 * type: (n: number | n > 0) => number | $ > 0
 */
function sqrt(n) {
  return Math.sqrt(n);
}

/*#rt type: number | x > 0 */
const x = 9;
