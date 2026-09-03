/*#rt type: (x: number) => number | $ === x */
function fakeIdentity(x) {
  x = 0;
  return x;
}

console.log(fakeIdentity(7));
