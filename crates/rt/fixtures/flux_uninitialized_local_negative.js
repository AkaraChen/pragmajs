/*#rt type: (x: number) => number | $ === x */
function uninitializedLocal(x) {
  let value;
  return x;
}

console.log(uninitializedLocal(1));
