/*#rt type: (x: number | x === x) => number | $ === x */
function shadowParameter(x) {
  {
    let x = 1;
    return x;
  }
}

console.log(shadowParameter(2));
