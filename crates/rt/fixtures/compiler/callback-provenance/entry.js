const identityBoxes = externalBoxes.map(box => box);

/*#rt type: number */
const identityAccepted = externalUnwrap(identityBoxes);

const localBoxes = [1].map(() => ({
  /** @returns {number} */
  lied() {
    return JSON.parse('\"not a number\"');
  },
}));

/*#rt type: number */
const localRejected = externalUnwrap(localBoxes);

const capturedBoxes = externalBoxes;
const capturedLocalBox = {
  /** @returns {number} */
  lied() {
    return JSON.parse('\"not a number\"');
  },
};
[1].map(() => capturedBoxes.push(capturedLocalBox));

/*#rt type: number */
const capturedRejected = externalUnwrap(capturedBoxes);
