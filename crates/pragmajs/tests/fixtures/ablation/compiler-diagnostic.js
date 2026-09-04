// @ablation-compiler-error
/*#rt type: () => number | $ > 0 */
function incorrectlyPositive() {
  return 0;
}

incorrectlyPositive();
