namespace N {
  /*#own type: () => void */
  export function make() {}

  export function installLocal() {
    /*#own type: () => unique Buffer */
    function make() {
      return Buffer.from("local");
    }
  }
}

N.make();
