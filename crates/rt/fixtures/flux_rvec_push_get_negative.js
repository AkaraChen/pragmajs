// Port of flux-rs tests/tests/neg/surface/test02.rs (get past last push)
/*#rt type: () => number[2] */
function vecPushOob() {
  const v = [0];
  v.pop();
  v.push(1);
  v.push(2);
  return v[2];
}

console.log(vecPushOob());
