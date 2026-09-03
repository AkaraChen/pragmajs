// Affine still forbids use-after-move.

/*#own type: () => affine File */
function openFile() {
  return { fd: 1 };
}

/*#own type: (f: affine File) => void */
function closeFile(f) {
  void f;
}

/*#own type: (f: affine File) => void */
function process(f) {
  closeFile(f);
  void f;
}

process(openFile());
