/*#rt type: (x: number) => boolean | $ === true */
function nanIsNotSelfEqual(x) {
  const alias = x;
  return alias === alias;
}
