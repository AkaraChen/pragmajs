/*#rt type: forall p. (x: boolean | p(x), y: boolean | y === x) => boolean | p($) */
function preserveAcrossEquality(x, y) {
  return y;
}

/*#rt type: boolean | seed === true */
const seed = true;
console.log(preserveAcrossEquality(seed, seed));
