/** @returns {number} */
function lied() {
  return JSON.parse('"not a number"');
}

/*#rt type: number */
const accepted = lied();
