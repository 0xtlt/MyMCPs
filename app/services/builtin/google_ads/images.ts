/**
 * What an uploaded file is, read from its first bytes. Google Ads takes JPEG,
 * PNG, and GIF images, and judges them by their size in pixels.
 */
export type ImageInfo = {
  format: 'jpeg' | 'png' | 'gif'
  width: number
  height: number
}

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])

function pngInfo(bytes: Buffer): ImageInfo | null {
  if (bytes.length < 24 || !bytes.subarray(0, 8).equals(PNG_SIGNATURE)) return null
  if (bytes.toString('latin1', 12, 16) !== 'IHDR') return null
  return { format: 'png', width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) }
}

function gifInfo(bytes: Buffer): ImageInfo | null {
  const signature = bytes.toString('latin1', 0, 6)
  if (bytes.length < 10 || (signature !== 'GIF87a' && signature !== 'GIF89a')) return null
  return { format: 'gif', width: bytes.readUInt16LE(6), height: bytes.readUInt16LE(8) }
}

/** The frame headers that carry the size: every start of frame but the tables among them. */
function isStartOfFrame(marker: number) {
  return marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc
}

function jpegInfo(bytes: Buffer): ImageInfo | null {
  if (bytes.length < 4 || bytes[0] !== 0xff || bytes[1] !== 0xd8) return null

  let offset = 2
  while (offset + 9 <= bytes.length) {
    if (bytes[offset] !== 0xff) return null
    const marker = bytes[offset + 1]
    // Padding, and markers that stand alone.
    if (marker === 0xff) {
      offset += 1
      continue
    }
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd8)) {
      offset += 2
      continue
    }

    const length = bytes.readUInt16BE(offset + 2)
    if (length < 2) return null
    if (isStartOfFrame(marker)) {
      return {
        format: 'jpeg',
        height: bytes.readUInt16BE(offset + 5),
        width: bytes.readUInt16BE(offset + 7),
      }
    }
    offset += 2 + length
  }
  return null
}

/** `null` for anything but a JPEG, PNG, or GIF image with a size. */
export function imageInfo(bytes: Buffer): ImageInfo | null {
  const info = pngInfo(bytes) ?? gifInfo(bytes) ?? jpegInfo(bytes)
  return info && info.width > 0 && info.height > 0 ? info : null
}

/**
 * The shapes Google Ads takes images in, with the smallest size it accepts
 * for each. An image has a shape when its ratio is within 1% of it, as Google
 * measures it.
 */
export const IMAGE_SHAPES = [
  { name: 'landscape', label: 'landscape (1.91:1)', ratio: 1.91, minWidth: 600, minHeight: 314 },
  { name: 'square', label: 'square (1:1)', ratio: 1, minWidth: 300, minHeight: 300 },
  { name: 'wide_logo', label: 'wide logo (4:1)', ratio: 4, minWidth: 512, minHeight: 128 },
  { name: 'portrait', label: 'portrait (4:5)', ratio: 0.8, minWidth: 480, minHeight: 600 },
] as const

/** A square image this small is still a logo, but not a marketing image. */
export const SQUARE_LOGO_MIN_PIXELS = 128

export type ImageShape = (typeof IMAGE_SHAPES)[number]

/** The shape an image has, whatever its size. `null` when Google Ads has no use for it. */
export function imageShape({ width, height }: Pick<ImageInfo, 'width' | 'height'>) {
  const ratio = width / height
  return IMAGE_SHAPES.find((shape) => Math.abs(ratio / shape.ratio - 1) <= 0.01) ?? null
}
