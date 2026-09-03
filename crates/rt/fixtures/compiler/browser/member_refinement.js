const flattened = [1, 2, 3].flatMap(value => [value, value]);

/*#rt type: number | count >= 0 */
const count = flattened.length;

const canvas = document.createElement("canvas");

/*#rt type: number */
const width = canvas.width;

const context = canvas.getContext("2d");
console.log(count, width, context);
