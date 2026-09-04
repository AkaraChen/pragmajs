/*#own type: (resource: unique Resource) => void */
function actualCapture(resource) {
  const consumeLater = () => void resource;
  consumeLater();
  void resource;
}
