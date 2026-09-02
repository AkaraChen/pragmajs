// Unique created and consumed inside a loop is fine (each iteration is distinct).

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: () => void */
function process() {
  while (true) {
    /*#own let buf: unique Buffer */
    const buf = make();
    consume(buf);
  }
}

process();
