function Math() {}

/*#rt type: () => number | $ > 0 */
function shadowedMath() {
  return Math.sqrt(1);
}

console.log(shadowedMath());
