// Affine weakening: values may be dropped without an explicit consume.

/*#own type: () => affine File */
function openFile() {
  return { fd: 1 };
}

/*#own type: (f: affine File) => void */
function process(f) {
  // implicit destructor at end of scope
}

process(openFile());
