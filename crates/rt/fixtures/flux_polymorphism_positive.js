/*#rt type: forall p. (x: number | p(x)) => number | p($) */
function preserve(x) {
  return x;
}

/*#rt type: (x: number | x === x) => number | $ > 0 */
function preserveAfterNarrowing(x) {
  if (x > 0) {
    return preserve(x);
  }
  return 1;
}

/*#rt type: number | seed >= 7 */
const seed = 7;
/*#rt type: number | preserved >= 7 */
const preserved = preserve(seed);
/*#rt type: number | literalPreserved === 7 */
const literalPreserved = preserve(7);

console.log(preserved, literalPreserved, preserveAfterNarrowing(-1));
