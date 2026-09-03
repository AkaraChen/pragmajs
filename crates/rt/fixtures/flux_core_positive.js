/*#rt type: (x: number | x === x) => number | $ >= 0 */
function absolute(x) {
  if (x >= 0) {
    return x;
  }
  return -x;
}

/*#rt type: (x: number | x === x) => number | $ > 0 */
function guardedPositive(x) {
  if (x <= 0) {
    return 1;
  }
  return x;
}

/*#rt type: (b: boolean) => boolean | $ !== b */
function flip(b) {
  return !b;
}

/*#rt type: number | input > 0 */
const input = 9;
/*#rt type: number | result > 0 */
const result = guardedPositive(absolute(input));
/*#rt type: boolean | flag === true */
const flag = flip(false);

console.log(result, flag);
