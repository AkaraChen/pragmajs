/*#rt type: () => number | $ > 0 */
function escapedTemporaryName() {
  const \u005f\u005frt_return = 1;
  return \u005f\u005frt_return;
}

const \u005f\u005frt_v = 1;
/*#rt type: number | value > 0 */
const value = \u005f\u005frt_v;

console.log(escapedTemporaryName(), value);
