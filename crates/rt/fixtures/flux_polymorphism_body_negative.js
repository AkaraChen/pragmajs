/*#rt type: forall p. (x: number | p(x)) => number | p($) */
function doesNotPreserve(x) {
  return x + 1;
}

/*#rt type: number | seed >= 7 */
const seed = 7;
console.log(doesNotPreserve(seed));
