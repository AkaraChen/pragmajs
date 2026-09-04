/*#own type: () => void */
function make() {}

namespace N {
  /*#own type: () => unique Buffer */
  export function make() {
    return Buffer.from("namespace");
  }
}

make();
