import { useMemo, useState, type CSSProperties, type SyntheticEvent } from "react";
import { ImageIcon } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Slider } from "@/components/ui/slider";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";
import { formatBytes } from "../domain/format";
import type { ImageComparison, ImagePreview, ImageVersion } from "../ipc/types";

type ComparisonMode = "2-up" | "swipe" | "onion" | "difference";
interface ImageDimensions { width: number; height: number }
const modes = [
  { value: "2-up", label: "2-up" },
  { value: "swipe", label: "Swipe" },
  { value: "onion", label: "Onion Skin" },
  { value: "difference", label: "Difference" },
] as const;
const checkerboard = "bg-[repeating-conic-gradient(var(--muted)_0%_25%,transparent_0%_50%)] bg-size-[16px_16px]";

function usePreview(version: ImageVersion) {
  const preview = version.kind === "preview" ? version.preview : null;
  const mimeType = preview?.mimeType;
  const base64 = preview?.base64;
  const source = useMemo(() => mimeType && base64 ? "data:" + mimeType + ";base64," + base64 : "", [mimeType, base64]);
  const [decoded, setDecoded] = useState<{ source: string; dimensions: ImageDimensions | null; failed: boolean } | null>(null);
  return {
    source,
    dimensions: decoded?.source === source ? decoded.dimensions : null,
    failed: decoded?.source === source && decoded.failed,
    onLoad: (event: SyntheticEvent<HTMLImageElement>) => {
      const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
      if (width > 0 && height > 0) setDecoded({ source, dimensions: { width, height }, failed: false });
    },
    onError: () => setDecoded({ source, dimensions: null, failed: true }),
  };
}
type PreviewState = ReturnType<typeof usePreview>;

function ImagePlaceholder({ title, description }: { title: string; description?: string }) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyTitle>{title}</EmptyTitle>
        {description ? <EmptyDescription>{description}</EmptyDescription> : null}
      </EmptyHeader>
    </Empty>
  );
}

// Both versions share this frame and scale; a resized image is never stretched to match the other.
function frameStyle(frame: ImageDimensions): CSSProperties {
  return {
    width: `min(100%, ${frame.width}px, ${65 * frame.width / frame.height}vh)`,
    aspectRatio: `${frame.width} / ${frame.height}`,
  };
}

function imageStyle(dimensions: ImageDimensions, frame: ImageDimensions): CSSProperties {
  return { width: `${100 * dimensions.width / frame.width}%`, height: `${100 * dimensions.height / frame.height}%` };
}

function Preview({ image, title, frame }: { image: PreviewState; title: string; frame?: ImageDimensions }) {
  if (image.failed) return <ImagePlaceholder title="Image could not be displayed" />;
  const img = <img src={image.source} alt={title + " image"} onLoad={image.onLoad} onError={image.onError}
    className={frame ? "absolute left-0 top-0" : "max-h-[65vh] max-w-full object-contain"}
    style={frame && image.dimensions ? imageStyle(image.dimensions, frame) : undefined} />;
  return frame ? <div className="relative overflow-hidden" style={frameStyle(frame)}>{img}</div> : img;
}

function ImageDetails({ preview, image }: { preview: ImagePreview; image: PreviewState }) {
  return <span className="text-xs text-muted-foreground tabular-nums">
    {image.dimensions ? `${image.dimensions.width} × ${image.dimensions.height} px · ` : ""}{formatBytes(preview.byteLength)}
  </span>;
}

function ImageSide({ version, image, side, frame }: { version: ImageVersion; image: PreviewState; side?: "Before" | "After"; frame?: ImageDimensions }) {
  const title = side ?? (version.kind === "preview" ? version.preview.label : "Image");
  return (
    <figure className="flex min-w-0 flex-col border bg-card">
      <figcaption className="flex min-h-16 flex-wrap items-center justify-between gap-2 border-b px-3 py-2 text-sm">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <ImageIcon className="size-4 text-muted-foreground" aria-hidden="true" />
          <strong>{title}</strong>
          {side && version.kind === "preview" ? <span className="text-xs text-muted-foreground">{version.preview.label}</span> : null}
        </div>
        {version.kind === "preview" ? <ImageDetails preview={version.preview} image={image} /> : null}
      </figcaption>
      <div className={cn("grid min-h-64 flex-1 place-items-center overflow-hidden p-4", checkerboard)}>
        {version.kind === "preview" ? <Preview image={image} title={title} frame={frame} />
          : version.kind === "missing" ? <ImagePlaceholder title="File not present" />
          : version.kind === "tooLarge" ? <ImagePlaceholder title="Image too large" description="This image exceeds the 4 MiB preview limit." />
          : <ImagePlaceholder title="Preview unavailable" description="This file’s format cannot be previewed." />}
      </div>
    </figure>
  );
}

function Overlay({ before, after, frame, mode, position }: {
  before: PreviewState; after: PreviewState; frame: ImageDimensions; mode: Exclude<ComparisonMode, "2-up">; position: number;
}) {
  const difference = mode === "difference";
  // Flatten transparency onto identical white mattes for difference blending, so equal transparent
  // pixels also become black. The matte is part of the image calculation, independent of app theme.
  const matte = difference ? { backgroundColor: "#fff" } : undefined;
  return (
    <div className="grid min-h-64 flex-1 place-items-center overflow-hidden p-4">
      <div className="relative isolate overflow-hidden ring-1 ring-border" style={frameStyle(frame)} role="group" aria-label={`${modes.find((item) => item.value === mode)?.label} image comparison`}>
        <div className={cn("absolute inset-0 bg-background", !difference && checkerboard)} style={matte}>
          <img src={before.source} alt="Before image" onLoad={before.onLoad} onError={before.onError}
            className="absolute left-0 top-0" style={imageStyle(before.dimensions ?? frame, frame)} />
        </div>
        <div className={cn("absolute inset-0 bg-background", !difference && checkerboard)} style={{
          ...matte,
          clipPath: mode === "swipe" ? `inset(0 0 0 ${position}%)` : undefined,
          opacity: mode === "onion" ? position / 100 : undefined,
          mixBlendMode: difference ? "difference" : undefined,
        }}>
          <img src={after.source} alt="After image" onLoad={after.onLoad} onError={after.onError}
            className="absolute left-0 top-0" style={imageStyle(after.dimensions ?? frame, frame)} />
        </div>
        {mode === "swipe" ? <div aria-hidden="true" className="pointer-events-none absolute inset-y-0 w-px bg-foreground" style={{ left: `${position}%` }} /> : null}
      </div>
    </div>
  );
}

export function ImageComparisonView({ comparison }: { comparison: ImageComparison }) {
  const before = usePreview(comparison.before);
  const after = usePreview(comparison.after);
  const [mode, setMode] = useState<ComparisonMode>("2-up");
  const [swipe, setSwipe] = useState(50);
  const [opacity, setOpacity] = useState(50);
  if (comparison.before.kind === "missing" || comparison.after.kind === "missing") {
    const showAfter = comparison.before.kind === "missing";
    return (
      <div className="grid min-h-full grid-cols-1 p-4" role="region" aria-label="Image preview">
        <ImageSide version={showAfter ? comparison.after : comparison.before} image={showAfter ? after : before} />
      </div>
    );
  }

  const hasPreviews = comparison.before.kind === "preview" && comparison.after.kind === "preview";
  const frame = before.dimensions && after.dimensions && !before.failed && !after.failed ? {
    width: Math.max(before.dimensions.width, after.dimensions.width),
    height: Math.max(before.dimensions.height, after.dimensions.height),
  } : undefined;
  const activeMode = hasPreviews && frame ? mode : "2-up";
  const position = activeMode === "swipe" ? swipe : opacity;
  const byteChange = comparison.before.kind === "preview" && comparison.after.kind === "preview"
    ? comparison.after.preview.byteLength - comparison.before.preview.byteLength : 0;
  return (
    <div className="flex min-h-full flex-col" role="region" aria-label="Image comparison">
      {hasPreviews ? <div className="flex flex-col items-center gap-3 border-b p-4">
        <ToggleGroup aria-label="Image comparison mode" value={[activeMode]} size="sm" variant="outline" spacing={0}
          className="grid w-full max-w-md grid-cols-4" onValueChange={(values) => {
            const next = modes.find((item) => item.value === values[0]);
            if (next) setMode(next.value);
          }}>
          {modes.map((item) => <ToggleGroupItem key={item.value} value={item.value} disabled={item.value !== "2-up" && !frame} className="min-w-0 px-2">{item.label}</ToggleGroupItem>)}
        </ToggleGroup>
        {activeMode === "swipe" || activeMode === "onion" ? <div className="flex w-full max-w-md flex-col gap-2">
          <div className="flex justify-between text-xs text-muted-foreground"><span>Before</span><span className="tabular-nums">{activeMode === "swipe" ? `Split ${position}%` : `After ${position}%`}</span><span>After</span></div>
          <Slider thumbLabel={activeMode === "swipe" ? "Swipe position" : "After image opacity"} value={[position]} min={0} max={100} step={1}
            onValueChange={(value) => {
              const next = Array.isArray(value) ? value[0] : value;
              if (next !== undefined) (activeMode === "swipe" ? setSwipe : setOpacity)(next);
            }} />
        </div> : null}
      </div> : null}
      {activeMode === "2-up" ? <div className="grid flex-1 grid-cols-2 gap-4 p-4">
        <ImageSide version={comparison.before} image={before} side="Before" frame={frame} />
        <ImageSide version={comparison.after} image={after} side="After" frame={frame} />
      </div> : frame ? <>
        <div className="flex flex-wrap justify-between gap-3 px-4 pt-4 text-sm">
          {comparison.before.kind === "preview" ? <div className="flex flex-col gap-1"><span><strong>Before</strong> · {comparison.before.preview.label}</span><ImageDetails preview={comparison.before.preview} image={before} /></div> : null}
          {comparison.after.kind === "preview" ? <div className="flex flex-col gap-1 text-right"><span><strong>After</strong> · {comparison.after.preview.label}</span><ImageDetails preview={comparison.after.preview} image={after} /></div> : null}
        </div>
        <Overlay before={before} after={after} frame={frame} mode={activeMode} position={position} />
      </> : null}
      {hasPreviews ? <div className="pb-4 text-center text-xs text-muted-foreground tabular-nums">
        {activeMode === "difference" ? "Unchanged pixels are black · " : ""}Size change: {byteChange > 0 ? "+" : byteChange < 0 ? "−" : ""}{formatBytes(Math.abs(byteChange))}
      </div> : null}
    </div>
  );
}
