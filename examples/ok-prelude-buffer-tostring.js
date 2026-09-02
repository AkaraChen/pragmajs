// Node prelude instance method: Buffer#toString is &readonly and must not consume.

/*#own type: () => unique Buffer */
function make() {
  return Buffer.from("hi");
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: unique Buffer) => void */
function process(buf) {
  buf.toString();
  consume(buf);
}

process(make());

