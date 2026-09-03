// Austral Rule 3: consume in every branch.

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => void */
function consume(buf) {
  void buf;
}

/*#own type: (buf: unique Buffer, flag: copy boolean) => void */
function process(buf, flag) {
  if (flag) {
    consume(buf);
  } else {
    consume(buf);
  }
}

process(make(), true);
