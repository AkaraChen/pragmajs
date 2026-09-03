// Austral Rule 5: unique defined outside a loop cannot be consumed inside it.

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
  while (true) {
    consume(buf);
  }
}

process(make());
