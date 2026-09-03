/*#rt type: () => number | $ === 1 */
function* generatedValue() {
  yield 1;
  return 1;
}

console.log(generatedValue());
