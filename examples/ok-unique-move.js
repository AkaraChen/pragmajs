// Adapted from Austral 006-linearity: unique (linear) value consumed exactly once.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own
 * type: (buf: unique Buffer) => void
 */
function process(buf) {
  consume(buf);
}

process(make());
