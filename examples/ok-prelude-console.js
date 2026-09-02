// Node prelude: console.log is copy and must not consume a unique owner.

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
  console.log(buf);
  consume(buf);
}

process(make());
