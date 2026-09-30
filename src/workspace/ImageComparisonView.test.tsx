import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FileDiff, ImageComparison, ImagePreview } from "../ipc/types";
import { FileDiffFallback } from "./FileDiffFallback";

const before: ImagePreview = {
  mimeType: "image/png",
  base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGO44+YGAANqAWmzbfR3AAAAAElFTkSuQmCC",
  byteLength: 69,
  label: "HEAD",
};
const after: ImagePreview = { ...before, base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGPQqLgDAAJIAX2aqSu/AAAAAElFTkSuQmCC", label: "Working copy" };
const comparison: ImageComparison = {
  before: { kind: "preview", preview: before },
  after: { kind: "preview", preview: after },
};
const diff: FileDiff = {
  binary: true, submodule: false, truncated: false, patch: "", hunks: [], stagedHunks: [], unstagedHunks: [], image: comparison,
};

function loadImage(name: "Before image" | "After image", width: number, height: number) {
  const image = screen.getByRole("img", { name });
  Object.defineProperties(image, {
    naturalWidth: { configurable: true, value: width },
    naturalHeight: { configurable: true, value: height },
  });
  fireEvent.load(image);
}

function loadBoth() {
  loadImage("Before image", 1200, 630);
  loadImage("After image", 1200, 630);
}

describe("image comparison", () => {
  beforeEach(() => {
    // JSDOM has no layout. Base UI's edge-aligned slider needs track and thumb measurements.
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return new DOMRect(0, 0, this.dataset.slot === "slider-thumb" ? 12 : 400, 16);
    });
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("renders both versions with their source labels and exact image data", () => {
    render(<FileDiffFallback diff={diff} />);
    expect(screen.getByRole("region", { name: "Image comparison" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Before image" })).toHaveAttribute("src", "data:image/png;base64," + before.base64);
    expect(screen.getByRole("img", { name: "After image" })).toHaveAttribute("src", "data:image/png;base64," + after.base64);
    expect(screen.getByText("HEAD")).toBeInTheDocument();
    expect(screen.getByText("Working copy")).toBeInTheDocument();
    expect(screen.queryByText(/Pixel-level|Raster preview/)).not.toBeInTheDocument();
  });

  it.each([
    ["before", "Working copy"],
    ["after", "HEAD"],
    ["before", "Selected commit"],
    ["after", "Parent commit"],
  ] as const)("shows a single %s-missing preview labeled %s", (side, label) => {
    const presentSide = side === "before" ? "after" : "before";
    const preview = presentSide === "after" ? after : before;
    render(<FileDiffFallback diff={{ ...diff, image: {
      ...comparison,
      [side]: { kind: "missing" },
      [presentSide]: { kind: "preview", preview: { ...preview, label } },
    } }} />);
    expect(screen.getByRole("region", { name: "Image preview" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Image comparison" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("figure")).toHaveLength(1);
    expect(screen.getAllByRole("img")).toHaveLength(1);
    expect(screen.getByRole("img", { name: label + " image" })).toHaveAttribute("src", "data:image/png;base64," + preview.base64);
    expect(screen.getAllByText(label)).toHaveLength(1);
    expect(screen.queryByText(/^(Before|After|File not present)$/)).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Image comparison mode" })).not.toBeInTheDocument();
  });

  it.each([
    ["tooLarge", "Image too large"],
    ["unsupported", "Preview unavailable"],
  ] as const)("explains an unavailable %s side without hiding the other image", (kind, title) => {
    render(<FileDiffFallback diff={{ ...diff, image: { ...comparison, after: { kind } } }} />);
    expect(screen.getByRole("region", { name: "Image comparison" })).toBeInTheDocument();
    expect(screen.getByText(title)).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Before image" })).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "After image" })).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Image comparison mode" })).not.toBeInTheDocument();
  });

  it("enables overlay modes only after both image dimensions are known", () => {
    render(<FileDiffFallback diff={diff} />);
    expect(screen.getByRole("button", { name: "Swipe" })).toBeDisabled();
    loadImage("Before image", 1200, 630);
    expect(screen.getByRole("button", { name: "Swipe" })).toBeDisabled();
    loadImage("After image", 1200, 630);
    expect(screen.getByRole("button", { name: "Swipe" })).toBeEnabled();
    expect(screen.getAllByText("1200 × 630 px · 69 B")).toHaveLength(2);
  });

  it("moves the swipe boundary with the keyboard, including both endpoints", async () => {
    render(<FileDiffFallback diff={diff} />);
    loadBoth();
    fireEvent.click(screen.getByRole("button", { name: "Swipe" }));
    const slider = await screen.findByRole("slider", { name: "Swipe position" });
    expect(slider).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByRole("group", { name: "Swipe image comparison" })).toBeInTheDocument();
    const labels = screen.getByText("Split 50%").parentElement;
    expect(labels?.firstElementChild).toHaveTextContent("Before");
    expect(labels?.lastElementChild).toHaveTextContent("After");
    expect(screen.getByRole("img", { name: "After image" }).parentElement).toHaveStyle({ clipPath: "inset(0 0 0 50%)" });
    fireEvent.keyDown(slider, { key: "End" });
    expect(slider).toHaveAttribute("aria-valuenow", "100");
    expect(screen.getByRole("img", { name: "After image" }).parentElement).toHaveStyle({ clipPath: "inset(0 0 0 100%)" });
    fireEvent.keyDown(slider, { key: "Home" });
    expect(slider).toHaveAttribute("aria-valuenow", "0");
    expect(screen.getByRole("img", { name: "After image" }).parentElement).toHaveStyle({ clipPath: "inset(0 0 0 0%)" });
  });

  it("adjusts onion skin opacity independently of the swipe position", async () => {
    render(<FileDiffFallback diff={diff} />);
    loadBoth();
    fireEvent.click(screen.getByRole("button", { name: "Onion Skin" }));
    const slider = await screen.findByRole("slider", { name: "After image opacity" });
    expect(screen.getByRole("img", { name: "After image" }).parentElement).toHaveStyle({ opacity: "0.5" });
    fireEvent.keyDown(slider, { key: "Home" });
    expect(screen.getByRole("img", { name: "After image" }).parentElement).toHaveStyle({ opacity: "0" });
    fireEvent.keyDown(slider, { key: "End" });
    expect(screen.getByRole("img", { name: "After image" }).parentElement).toHaveStyle({ opacity: "1" });
    fireEvent.click(screen.getByRole("button", { name: "Swipe" }));
    expect(await screen.findByRole("slider", { name: "Swipe position" })).toHaveAttribute("aria-valuenow", "50");
    fireEvent.click(screen.getByRole("button", { name: "Onion Skin" }));
    expect(await screen.findByRole("slider", { name: "After image opacity" })).toHaveAttribute("aria-valuenow", "100");
  });

  it("aligns different image dimensions at the same scale in difference mode", () => {
    render(<FileDiffFallback diff={diff} />);
    loadImage("Before image", 100, 100);
    loadImage("After image", 200, 50);
    fireEvent.click(screen.getByRole("button", { name: "Difference" }));
    expect(screen.getByRole("group", { name: "Difference image comparison" })).toHaveStyle({ aspectRatio: "200 / 100" });
    const beforeImage = screen.getByRole("img", { name: "Before image" });
    const afterImage = screen.getByRole("img", { name: "After image" });
    expect(beforeImage).toHaveStyle({ width: "50%", height: "100%" });
    expect(afterImage).toHaveStyle({ width: "100%", height: "50%" });
    expect(beforeImage.parentElement).toHaveStyle({ backgroundColor: "#fff" });
    expect(afterImage.parentElement).toHaveStyle({ backgroundColor: "#fff", mixBlendMode: "difference" });
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();
    expect(screen.getByText(/Unchanged pixels are black/)).toBeInTheDocument();
  });

  it("falls back to 2-up after an overlay decode failure and clears stale dimensions for new data", () => {
    const view = render(<FileDiffFallback diff={diff} />);
    loadBoth();
    fireEvent.click(screen.getByRole("button", { name: "Swipe" }));
    fireEvent.error(screen.getByRole("img", { name: "After image" }));
    expect(screen.getByText("Image could not be displayed")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Before image" })).toBeInTheDocument();
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Swipe" })).toBeDisabled();
    view.rerender(<FileDiffFallback diff={{ ...diff, image: { ...comparison, after: { kind: "preview", preview: { ...after, base64: before.base64 } } } }} />);
    expect(screen.getByRole("button", { name: "Swipe" })).toBeDisabled();
    expect(screen.queryByText("Image could not be displayed")).not.toBeInTheDocument();
    loadImage("After image", 300, 200);
    expect(screen.getByRole("group", { name: "Swipe image comparison" })).toHaveStyle({ aspectRatio: "1200 / 630" });
    expect(screen.getByRole("img", { name: "After image" })).toHaveStyle({ width: "25%" });
  });

  it("handles decoding failures and tries again when the image data changes", () => {
    const view = render(<FileDiffFallback diff={diff} />);
    fireEvent.error(screen.getByRole("img", { name: "After image" }));
    expect(screen.getByText("Image could not be displayed")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Before image" })).toBeInTheDocument();
    view.rerender(<FileDiffFallback diff={{ ...diff, image: { ...comparison, after: { kind: "preview", preview: { ...after, base64: before.base64 } } } }} />);
    expect(screen.getByRole("img", { name: "After image" })).toBeInTheDocument();
    expect(screen.queryByText("Image could not be displayed")).not.toBeInTheDocument();
  });

  it("preserves the generic binary and submodule fallbacks", () => {
    const view = render(<FileDiffFallback diff={{ ...diff, image: null }} />);
    expect(screen.getByText("Binary file changed")).toBeInTheDocument();
    view.rerender(<FileDiffFallback diff={{ ...diff, submodule: true }} />);
    expect(screen.getByText("Submodule commit changed")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Image comparison" })).not.toBeInTheDocument();
  });
});
