// Austral Rule 9 / Rule 11: no overlapping `&mut` in one expression.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (a: &mut Buffer, b: &mut Buffer) => void */
function both(a, b) {}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  both(/*#own &mut */ buf, /*#own &mut */ buf);
  consume(buf);
}

process(make());
