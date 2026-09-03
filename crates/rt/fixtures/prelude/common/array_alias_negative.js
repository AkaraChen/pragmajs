/*#rt type: (n: number | n === 1) => number */
function mustBeOne(n) {
  return n;
}

const numbers = [1];

/*#rt type: Array<unknown> */
const wider = numbers;

wider.push(true);
numbers.map(mustBeOne);
