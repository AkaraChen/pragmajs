/*#rt type: (x: number | x > 0) => number | $ > 0 */
function requiresPositive(x) {
  return x;
}

requiresPositive(-1);
