// Adapted from Austral: a unique value cannot be consumed twice in one expression.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (a: unique Buffer, b: unique Buffer) => void */
function pair(a, b) {
  void a;
  void b;
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  pair(buf, buf);
}

process(make());
