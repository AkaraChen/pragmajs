import { basename as base, resolve } from "node:path";

const file = base("/tmp/readme.md");

/*#rt type: number | bytes >= 0 */
const bytes = Buffer.byteLength(file);

const cwd = process.cwd();
const resolved = resolve();
console.log(file, bytes, cwd, resolved);
