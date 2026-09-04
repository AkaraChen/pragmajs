/*#own type: (value: unique Resource) => void */
function passToUnknownThenReuse(value) {
  unknown(value);
  void value;
}
