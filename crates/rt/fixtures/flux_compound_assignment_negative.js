/*#rt type: (value: number | value > 0) => number | $ > 0 */
function positive(value) {
  return value;
}

/*#rt type: number | value > 0 */
let value = 1;
value -= 2;
positive(value);
