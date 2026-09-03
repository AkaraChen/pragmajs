/*#rt type: (__rt: number | __rt > 0) => number | $ > 0 */
function reservedRuntimeBinding(__rt) {
  return __rt;
}

console.log(reservedRuntimeBinding(1));
