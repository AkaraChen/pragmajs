/*#rt type: (n: number) => boolean[0 < n] */
function isPos(n) {
  if (0 < n) {
    return true;
  }
  return false;
}

console.log(isPos(3), isPos(-1));
