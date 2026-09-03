// Adapted from Austral Rule 1: unique values cannot appear zero times.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own
 * type: (buf: unique Buffer) => void
 */
function process(buf) {
  // forgot to consume buf
}

process(make());
