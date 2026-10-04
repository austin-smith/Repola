/** ICNS elements are unordered. Keep Tauri's rendition bytes and give the container a stable order. */
export function canonicalizeIcns(contents) {
  if (contents.length < 8 || contents.toString("ascii", 0, 4) !== "icns" || contents.readUInt32BE(4) !== contents.length) {
    throw new Error("Invalid ICNS container.");
  }
  const elements = [];
  for (let offset = 8; offset < contents.length;) {
    if (offset + 8 > contents.length) throw new Error("Truncated ICNS element.");
    const length = contents.readUInt32BE(offset + 4);
    if (length < 8 || offset + length > contents.length) throw new Error("Invalid ICNS element length.");
    elements.push(contents.subarray(offset, offset + length));
    offset += length;
  }
  elements.sort((left, right) => Buffer.compare(left.subarray(0, 4), right.subarray(0, 4)));
  return Buffer.concat([contents.subarray(0, 8), ...elements]);
}
