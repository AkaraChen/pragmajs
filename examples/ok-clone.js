// Clone escape hatch: explicit duplication without consuming the original.

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
  /*#own clone buf as copy */
  const copy = buf;
  consume(buf);
  consume(copy);
}

process(make());
