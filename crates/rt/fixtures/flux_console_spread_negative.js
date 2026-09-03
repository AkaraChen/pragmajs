/*#rt type: () => number | $ === 1 */
function invalidSpread() {
  console.log(...1);
  return 1;
}

console.log(invalidSpread());
