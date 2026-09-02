// Node prelude: Buffer.from returns unique Buffer; forgetting it is unique-forget.

/*#own type: () => void */
function main() {
  const buf = Buffer.from("hi");
}
