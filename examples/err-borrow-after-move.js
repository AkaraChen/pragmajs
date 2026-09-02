// Austral Rule 8: borrowing cannot happen after consumption.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: &readonly Buffer) => void */
function read(buf) {}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  consume(buf);
  read(buf);
}

process(make());
