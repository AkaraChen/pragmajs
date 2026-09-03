// Node prelude: FileHandle#close consumes unique this.

/*#own type: (fh: unique FileHandle) => void */
function process(fh) {
  fh.close();
}
