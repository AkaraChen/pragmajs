/*#own type: (value: unique Resource) => void */
function consume(value) {
  void value;
}

/*#own type: (value: unique Resource) => void */
function optionallyConsume(value) {
  consume?.(value);
}
