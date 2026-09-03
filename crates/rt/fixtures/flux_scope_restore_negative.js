/*#rt type: (value: number | value > 0) => number | $ > 0 */
function positive(value) {
  return value;
}

const value = -1;
{
  const value = 1;
  console.log(value);
}
positive(value);
