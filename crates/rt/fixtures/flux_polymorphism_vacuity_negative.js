/*#rt type: forall p. (x: number | p(x)) => number | $ > 0 */
function unsound(x) {
  return x;
}

/*#rt type: number | seed < 0 */
const seed = -1;
console.log(unsound(seed));
