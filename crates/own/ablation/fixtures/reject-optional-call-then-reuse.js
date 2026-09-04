/*#own type: (value: affine Resource) => void */
function consume(value) {
  void value;
}

/*#own type: (value: affine Resource) => void */
function optionalConsumeThenReuse(value) {
  consume?.(value);
  consume(value);
}
