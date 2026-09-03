const cwd = Deno.cwd();

/*#rt type: number | argc >= 0 */
const argc = Deno.args.length;

console.log(cwd, argc);
