/*#rt type: (open: boolean | open === true) => boolean | $ === false */
function close(open) {
  return false;
}

/*#rt type: boolean | door === true */
let door = true;
door = close(door);

/*#rt type: boolean | closed === false */
const closed = door;
console.log(closed);
