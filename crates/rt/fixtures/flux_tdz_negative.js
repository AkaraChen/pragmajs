/*#rt type: number | outer === 1 */
const outer = 1;

{
  /*#rt type: number | copy === 1 */
  const copy = outer;
  let outer = 2;
  console.log(copy, outer);
}
