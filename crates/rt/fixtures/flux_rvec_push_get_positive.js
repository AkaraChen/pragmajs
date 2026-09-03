// Port of flux-rs tests/tests/pos/surface/test02.rs (RVec::push then get; drop &mut)
/*#rt type: () => number[2] */
function vecPush() {
  const v = [0];
  v.pop();
  v.push(1);
  v.push(2);
  const x = v[1];
  return v.length;
}

console.log(vecPush());
