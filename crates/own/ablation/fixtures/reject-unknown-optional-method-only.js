/*#own type: (value: unique Resource, holder: copy Holder) => void */
function maybeConsume(value, holder) {
  holder.consume?.(value);
}
