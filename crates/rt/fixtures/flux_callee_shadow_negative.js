/*#rt type: (x: number) => number | $ === x */
function identity(x) {
  return x;
}

/*#rt type: () => number | $ === 1 */
function shadowedCall() {
  const identity = true;
  return identity(1);
}

console.log(shadowedCall());
