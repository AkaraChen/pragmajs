/*#rt type: number | ticks >= 0 */
const ticks = Bun.nanoseconds();

const file = Bun.file(".");

/*#rt type: number | size >= 0 */
const size = file.size;

console.log(ticks, size);
