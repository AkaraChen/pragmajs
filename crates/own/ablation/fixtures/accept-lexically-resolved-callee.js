/*#own type: (value: copy Resource) => void */
function touch(value) {}

function unrelatedScope() {
  /*#own type: (value: unique Resource) => void */
  function touch(value) {
    void value;
  }
  touch({});
}

/*#own type: (resource: unique Resource) => void */
function inspectThenConsume(resource) {
  touch(resource);
  void resource;
}
