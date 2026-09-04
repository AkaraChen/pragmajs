namespace N {
  /*#own type: () => unique Buffer */
  export function make() {
    return Buffer.from("namespace");
  }

  export function installLocal() {
    /*#own type: () => void */
    function make() {}
  }
}

N.make();
