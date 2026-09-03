/*#rt type: (value: number | value > 0) => void */
function positiveOnly(value) {}

const values = [-1];
values.forEach(positiveOnly);
