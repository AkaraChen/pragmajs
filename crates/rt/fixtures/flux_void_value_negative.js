/*#rt type: () => boolean | $ === true */
function voidAsBoolean() {
  return console.log() && true;
}

console.log(voidAsBoolean());
