/*#own type: (resource: unique Resource) => void */
function shadowedParameterIsNotACapture(resource) {
  const identity = (resource) => resource;
  identity({});
  void resource;
}
