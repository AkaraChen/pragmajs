/*#rt type: (x: number | x === 9007199254740991) => number | $ > x + 1 */
function unsafeIncrement(x) {
  return (x + 1) + 1;
}

console.log(unsafeIncrement(9007199254740991));
