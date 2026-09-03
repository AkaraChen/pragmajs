pub fn runtime_block() -> &'static str {
    r#"const __rt = {
  assert(cond, message, context) {
    if (!cond) {
      const err = new Error(message);
      err.name = "RefinementTypeError";
      err.context = context;
      throw err;
    }
  }
};"#
}
