// A borrow must not escape its lexical region (Austral unnamed/named region).

/*#own type: () => unique Buffer */
function make() {
  return { bytes: 0 };
}

/*#own type: (buf: unique Buffer) => unique Buffer */
function process(buf) {
  /*#own borrow buf as view: &readonly Buffer */
  const view = buf;
  return view;
}

process(make());
