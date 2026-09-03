/*#rt type: Array<number> */
const doubled = [1, 2, 3].map(value => value + 1);

/*#rt type: number | count >= 0 */
const count = doubled.length;

/*#rt type: boolean */
const includesTwo = doubled.includes(2);

const pushed = [1];
/*#rt type: number | pushedLength === 2 */
const pushedLength = pushed.push(2);

/*#rt type: number */
const sum = [1, 2].reduce((accumulator, value) => accumulator + value, 0);

/*#rt type: number */
const sumWithoutInitial = [1, 2].reduce((accumulator, value) => accumulator + value);

/*#rt type: Array<number> */
const parenthesized = [1, 2].map((value => value + 1));

/*#rt type: number */
const initial = [1].reduce(accumulator => accumulator, 0);

/*#rt type: (accumulator: number, value: number) => number */
function add(accumulator, value) {
  return accumulator + value;
}

/*#rt type: number */
const namedSum = [1, 2].reduce(add, 0);

console.log(
  count,
  includesTwo,
  pushedLength,
  sum,
  sumWithoutInitial,
  parenthesized,
  initial,
  namedSum,
);
