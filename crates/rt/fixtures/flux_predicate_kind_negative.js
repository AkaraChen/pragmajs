/*#rt type: forall p. (x: number | p(x), b: boolean | p(b)) => number | p($) */
function mixedPredicateDomain(x, b) {
  return x;
}

console.log(mixedPredicateDomain(1, true));
