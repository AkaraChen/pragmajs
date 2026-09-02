// `&readonly` and `&mut` of the same value in one expression conflict.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (r: &readonly Buffer, w: &mut Buffer) => void */
function mix(r, w) {}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  mix(/*#own &readonly */ buf, /*#own &mut */ buf);
  consume(buf);
}

process(make());
