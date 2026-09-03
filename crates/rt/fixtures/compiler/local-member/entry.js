/** @type {ExternalBox} */
const box = {
  /** @returns {number} */
  lied() {
    return JSON.parse('"not a number"');
  },
};

/*#rt type: number */
const accepted = box.lied();
