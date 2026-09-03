const values = [1];
const button = document.createElement("button");
button.addEventListener("click", () => console.log(values.push(2)));
button.click();

/*#rt type: number | count === 1 */
const count = values.length;
