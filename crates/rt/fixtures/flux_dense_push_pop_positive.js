const xs = [1, 2];
xs.push(3);
/*#rt type: number | afterPush === 3 */
const afterPush = xs.length;
const last = xs.pop();
/*#rt type: number | afterPop === 2 */
const afterPop = xs.length;
console.log(last, afterPush, afterPop);
