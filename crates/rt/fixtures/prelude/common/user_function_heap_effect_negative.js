/*#rt type: (values: Array<number>) => void */
function removeLast(values) {
  values.pop();
}

const values = [1];
removeLast(values);

/*#rt type: number | staleLength === 1 */
const staleLength = values.length;
