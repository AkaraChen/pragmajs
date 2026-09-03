/*#rt type: (__rt_return: number | __rt_return > 0) => number | $ > 0 */
function hygienicIdentity(__rt_return) {
  return __rt_return;
}

const __rt_v = 1;
/*#rt type: number | value > 0 */
const value = __rt_v;

console.log(hygienicIdentity(value));
