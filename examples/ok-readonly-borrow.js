// Adapted from Austral 007-borrowing: &readonly (immutable borrow) then consume.

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
  read(buf);
  consume(buf);
}

process(make());
