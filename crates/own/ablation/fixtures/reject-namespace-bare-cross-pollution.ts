namespace A {
  /*#own type: () => unique Buffer */
  export function make() {
    return Buffer.from("a");
  }

  make();
}

namespace B {
  /*#own type: () => void */
  export function make() {}
}
