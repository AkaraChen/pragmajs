function outer() {
  /*#rt type: (x: number) => number | $ > 0 */
  function uncheckedNested(x) {
    return -1;
  }

  return uncheckedNested(1);
}

console.log(outer());
