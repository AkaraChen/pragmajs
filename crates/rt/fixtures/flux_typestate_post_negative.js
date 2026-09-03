/*#rt type: (open: boolean | open === true) => boolean | $ === false */
function brokenClose(open) {
  return true;
}

/*#rt type: boolean | door === true */
const door = true;
console.log(brokenClose(door));
