/*#rt type: (open: boolean | open === true) => boolean | $ === false */
function close(open) {
  return false;
}

/*#rt type: boolean | door === false */
const door = false;
close(door);
