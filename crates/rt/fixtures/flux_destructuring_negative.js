/*#rt type: () => number | $ === 1 */
function destructuring() {
  const [value] = [1];
  return value;
}

console.log(destructuring());
