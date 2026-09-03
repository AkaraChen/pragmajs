const cwd = process.cwd();
const timer = setTimeout(() => console.log(cwd), 1);
clearTimeout(timer);

const bytes = Buffer.byteLength("deno");
console.log(bytes);
