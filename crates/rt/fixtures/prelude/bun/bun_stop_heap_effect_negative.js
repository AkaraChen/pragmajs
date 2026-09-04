/*#rt type: (xs: DenseArray<number>[1], server: Bun.Server) => number */
function popAfterStop(xs, server) {
  server.stop();
  return xs.pop();
}
