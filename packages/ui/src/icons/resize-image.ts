// Makes the picture of an uploaded page icon (ADR-0029, section 1): the picker redraws the
// chosen image in the webview, 256 x 256 pixels at most, and uploads that PNG instead of the
// original. An icon stays a few kilobytes, a photo's metadata (location, camera) never leaves
// the device inside an icon, and an animated GIF keeps its first frame only.

/** The longest side of an icon's picture, in pixels. */
export const MAX_ICON_SIDE = 256;

/** Turns the file the user chose into the file to upload; rejects with an `Error` when the picture cannot be read. */
export type ImageResizer = (file: File) => Promise<File>;

/** The size of a `width` x `height` picture shrunk to fit a `max` square, never enlarged, never empty. */
export function fitWithin(width: number, height: number, max: number = MAX_ICON_SIDE): { width: number; height: number } {
  if (!(width > 0) || !(height > 0)) {
    return { width: max, height: max };
  }
  const scale = Math.min(1, max / width, max / height);
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/** The name of the uploaded file: the original's name without its extension, and `.png`. */
export function pngName(name: string): string {
  const base = name.replace(/\.[^./\\]*$/, "").trim();
  return `${base === "" ? "icon" : base}.png`;
}

function decode(file: File): Promise<{ image: HTMLImageElement; release: () => void }> {
  const url = URL.createObjectURL(file);
  const release = (): void => {
    URL.revokeObjectURL(url);
  };
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => {
      resolve({ image, release });
    };
    image.onerror = () => {
      release();
      reject(new Error("the image cannot be decoded"));
    };
    image.src = url;
  });
}

function toBlob(canvas: HTMLCanvasElement, type: string, quality?: number): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => {
        if (blob === null) {
          reject(new Error("the image cannot be encoded"));
        } else {
          resolve(blob);
        }
      },
      type,
      quality,
    );
  });
}

function toPng(canvas: HTMLCanvasElement): Promise<Blob> {
  return toBlob(canvas, "image/png");
}

/**
 * Redraws the picture in a canvas, at most `MAX_ICON_SIDE` on each side, and returns it as a PNG
 * file. A webview without a 2D canvas (there is none in practice) gets the file as it is.
 */
export const downscaleToPng: ImageResizer = async (file) => {
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (context === null) {
    return file;
  }
  const { image, release } = await decode(file);
  try {
    const size = fitWithin(image.naturalWidth, image.naturalHeight);
    canvas.width = size.width;
    canvas.height = size.height;
    context.drawImage(image, 0, 0, size.width, size.height);
    const blob = await toPng(canvas);
    return new File([blob], pngName(file.name), { type: "image/png" });
  } finally {
    release();
  }
};

/**
 * The longest side of a cover's picture, in pixels (ADR-0040): a banner is at most a few hundred pixels tall and as
 * wide as the window, so 2560 pixels is as sharp as a large screen shows it. Unlike an icon's, a cover's picture is
 * kept at this size (a photo from a phone is several times larger).
 */
export const MAX_COVER_SIDE = 2560;

/** The JPEG quality of a cover's picture that had to be redrawn smaller. */
const COVER_JPEG_QUALITY = 0.9;

/** The name of the file of a redrawn cover: the original's name without its extension, and `.jpg` or `.png`. */
export function coverName(name: string, jpeg: boolean): string {
  const base = name.replace(/\.[^./\\]*$/, "").trim();
  return `${base === "" ? "cover" : base}.${jpeg ? "jpg" : "png"}`;
}

/**
 * The picture of an uploaded cover: the file as it is when it fits in `MAX_COVER_SIDE` on each side (or when its size
 * is not known: a vector picture), else redrawn smaller in the webview (a JPEG stays a JPEG, anything else becomes a
 * PNG; an animated GIF keeps its first frame). A webview without a 2D canvas (there is none in practice) gets the file
 * as it is.
 */
export const downscaleCover: ImageResizer = async (file) => {
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (context === null) {
    return file;
  }
  const { image, release } = await decode(file);
  try {
    const { naturalWidth: width, naturalHeight: height } = image;
    if (!(width > 0) || !(height > 0)) {
      return file;
    }
    const size = fitWithin(width, height, MAX_COVER_SIDE);
    if (size.width === width && size.height === height) {
      return file;
    }
    canvas.width = size.width;
    canvas.height = size.height;
    context.drawImage(image, 0, 0, size.width, size.height);
    const jpeg = file.type.toLowerCase() === "image/jpeg";
    const blob = jpeg ? await toBlob(canvas, "image/jpeg", COVER_JPEG_QUALITY) : await toPng(canvas);
    return new File([blob], coverName(file.name, jpeg), { type: jpeg ? "image/jpeg" : "image/png" });
  } finally {
    release();
  }
};
