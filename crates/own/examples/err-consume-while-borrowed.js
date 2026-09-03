// Austral Rule 10: cannot consume a variable inside a borrow of it.

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
  /*#own borrow buf as view: &readonly Buffer */
  const view = buf;
  read(view);
  consume(buf);
}

process(make());
