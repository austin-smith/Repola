import { readFile } from "node:fs/promises";
import { PNG } from "pngjs";
import artwork from "../src-tauri/assets/brand/artwork.json" with { type: "json" };

export const iconArtwork = artwork;
const brand = new URL("../src-tauri/assets/brand/", import.meta.url);

async function readLayer(name) {
  const image = PNG.sync.read(await readFile(new URL(name, brand)));
  if (image.width !== artwork.canvasSize || image.height !== artwork.canvasSize) {
    throw new Error(`Icon layer ${name} must use the shared ${artwork.canvasSize}-pixel canvas.`);
  }
  return image;
}

/** Every channel uses the same foreground pixels at the same coordinates. */
export async function composeIconArtwork(channel) {
  if (!Object.hasOwn(artwork.backgrounds, channel)) throw new Error(`Unknown icon channel: ${channel}`);
  const [foreground, background] = await Promise.all([readLayer(artwork.foreground), readLayer(artwork.backgrounds[channel])]);
  const output = { width: background.width, height: background.height, data: Buffer.from(background.data) };
  for (let offset = 0; offset < output.data.length; offset += 4) {
    const alpha = foreground.data[offset + 3];
    if (alpha > 0) {
      // The mark sits inside the opaque tile; its edge matte is composited in place.
      if (background.data[offset + 3] !== 255) throw new Error("The shared R must stay inside the opaque icon background.");
      for (let color = 0; color < 3; color++) {
        output.data[offset + color] = Math.floor((foreground.data[offset + color] * alpha + background.data[offset + color] * (255 - alpha) + 127) / 255);
      }
    } else if (output.data[offset + 3] === 0) {
      output.data.fill(0, offset, offset + 4);
    }
  }
  return output;
}

/** Remove the shared macOS inset for Windows, Linux, and browser icons. */
export function universalArtwork(image) {
  const { x, y, size } = artwork.tile;
  const output = new PNG({ width: size, height: size });
  PNG.bitblt(image, output, x, y, size, size, 0, 0);
  return output;
}

export function encodeArtwork(image) {
  return PNG.sync.write(image, { colorType: 6, bitDepth: 8, filterType: 4 });
}
