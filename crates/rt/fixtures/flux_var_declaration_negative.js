/*#rt type: (x: number) => number | $ === x */
function varDeclaration(x) {
  var value = x;
  return value;
}

console.log(varDeclaration(1));
