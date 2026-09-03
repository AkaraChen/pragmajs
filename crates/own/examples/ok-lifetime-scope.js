// Lexical region: borrow dies at end of block; owner can then be consumed.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (view: &readonly Buffer) => void */
function read(view) {}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  {
    /*#own borrow buf as view: &readonly Buffer */
    const view = buf;
    read(view);
  }
  consume(buf);
}

process(make());
