// Austral mutable borrow `&mut` / `&!`, then consume.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: &mut Buffer) => void */
function write(buf) {}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  write(buf);
  consume(buf);
}

process(make());
