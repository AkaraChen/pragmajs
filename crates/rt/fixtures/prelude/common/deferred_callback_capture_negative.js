/*#rt type: (value: number | value > 0) => void */
function requirePositive(value) {}

/*#rt type: number | current > 0 */
let current = 1;
setTimeout(() => requirePositive(current), 0);
