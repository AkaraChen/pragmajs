/*#rt type: forall p. (x: number | p(x)) => number | p($) */
function preserve(x) {
  return x;
}

/*#rt type: number | seed < 0 */
const seed = -7;
/*#rt type: number | impossible > 0 */
const impossible = preserve(seed);

console.log(impossible);
