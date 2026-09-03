/*#rt type: () => number | $ === 1 */
function constantBranch() {
  if (true) {
    return 1;
  }
}

console.log(constantBranch());
