const fs = require("node:fs");

const callbackResult = fs.readFile("input.txt", () => {});
console.log(callbackResult);
