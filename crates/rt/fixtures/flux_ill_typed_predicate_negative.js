/*#rt type: (value: boolean | value && 1) => boolean */
function illTypedPredicate(value) {
  return value;
}

console.log(illTypedPredicate(true));
