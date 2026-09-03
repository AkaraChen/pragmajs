/*#rt type: (value: number | value === 2) => void */
function exactlyTwo(value) {}

const iterations = [0, 0];
const values = [0];
iterations.forEach(() => exactlyTwo(values.push(0)));
