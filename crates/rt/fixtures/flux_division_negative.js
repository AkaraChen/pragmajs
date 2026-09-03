/*#rt type: (value: number | value === 1) => number | $ === 0 */
function integerDivisionWouldBeUnsound(value) {
  return value / 2;
}

console.log(integerDivisionWouldBeUnsound(1));
