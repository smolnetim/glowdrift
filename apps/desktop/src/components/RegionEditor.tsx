import { type KeyboardEvent, type PointerEvent, useEffect, useId, useRef } from "react";
import { cssVars } from "../lib/format";
import type { NormRect } from "../lib/ipc";
import { rgbCss, subscribeLiveColor } from "../lib/liveColor";
import {
  HANDLES,
  type Handle,
  REGION_MIN_SIZE,
  moveRegion,
  regionFromCorners,
  resizeRegion,
  resizeRegionBy,
} from "../lib/region";

/** Id of the box that is the capture region (the whole-region colour). */
export const REGION_BOX_ID = "region";

/** A rectangle on the preview, in fractions of the monitor. */
export interface EditorBox {
  id: string;
  label: string;
  rect: NormRect;
  /** Position of this box's colour in the engine's zone colours; undefined = the whole-region colour. */
  colorIndex?: number;
}

type Drag =
  | { kind: "move"; id: string; startX: number; startY: number; origin: NormRect }
  | { kind: "resize"; id: string; handle: Handle; startX: number; startY: number; origin: NormRect }
  | { kind: "draw"; id: string; startX: number; startY: number; moved: boolean };

/** Pointer travel (as a fraction of the stage) before a background drag draws a new rectangle. */
const DRAW_THRESHOLD = 0.005;
const NUDGE = 0.01;

/** A dot showing a box's live colour; written straight to the DOM so colour updates never re-render. */
export function LiveSwatch({ colorIndex }: { colorIndex: number | undefined }) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(
    () =>
      subscribeLiveColor((color) => {
        const el = ref.current;
        if (!el) return;
        const zone = colorIndex === undefined ? undefined : color.zones?.[colorIndex];
        el.style.backgroundColor = colorIndex === undefined ? color.smoothed : zone ? rgbCss(zone) : "transparent";
      }),
    [colorIndex],
  );
  return <span ref={ref} className="box-swatch" aria-hidden="true" />;
}

interface RegionEditorProps {
  /** The first box is the capture region; boxes after it are drawn on top in order. */
  boxes: EditorBox[];
  selectedId: string;
  /** Image shown behind the boxes; the stage keeps `aspectRatio` either way. */
  imageUrl: string | null;
  aspectRatio: number;
  busy: boolean;
  /** Accessible name of the capture region box. */
  regionLabel?: string;
  onSelect: (id: string) => void;
  onChange: (id: string, rect: NormRect) => void;
}

/** Preview of the whole monitor with draggable, resizable boxes; the selected box is raised. */
export function RegionEditor({
  boxes,
  selectedId,
  imageUrl,
  aspectRatio,
  busy,
  regionLabel = "Capture region",
  onSelect,
  onChange,
}: RegionEditorProps) {
  const stage = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const hintId = useId();
  /** True when the preview carries zone boxes; the lone capture region is edited without labels. */
  const zoned = boxes.some((b) => b.id !== REGION_BOX_ID);

  const rectOf = (id: string): NormRect | undefined => boxes.find((b) => b.id === id)?.rect;
  const region = rectOf(REGION_BOX_ID);

  /** Pointer position as fractions of the stage. */
  const fractions = (e: PointerEvent): { fx: number; fy: number } => {
    const box = stage.current?.getBoundingClientRect();
    if (!box || box.width === 0 || box.height === 0) return { fx: 0, fy: 0 };
    return { fx: (e.clientX - box.left) / box.width, fy: (e.clientY - box.top) / box.height };
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const { fx, fy } = fractions(e);
    const target = e.target as HTMLElement;
    const boxId = target.closest<HTMLElement>("[data-box]")?.dataset.box;
    const origin = boxId ? rectOf(boxId) : undefined;
    const handle = target.closest<HTMLElement>("[data-handle]")?.dataset.handle as Handle | undefined;
    if (boxId && origin) {
      onSelect(boxId);
      drag.current = handle
        ? { kind: "resize", id: boxId, handle, startX: fx, startY: fy, origin }
        : { kind: "move", id: boxId, startX: fx, startY: fy, origin };
    } else {
      drag.current = { kind: "draw", id: selectedId, startX: fx, startY: fy, moved: false };
    }
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const active = drag.current;
    if (!active) return;
    const { fx, fy } = fractions(e);
    const dx = fx - active.startX;
    const dy = fy - active.startY;
    if (active.kind === "move") {
      onChange(active.id, moveRegion(active.origin, dx, dy));
    } else if (active.kind === "resize") {
      onChange(active.id, resizeRegion(active.origin, active.handle, dx, dy));
    } else {
      if (!active.moved && Math.hypot(dx, dy) < DRAW_THRESHOLD) return;
      active.moved = true;
      onChange(active.id, regionFromCorners(active.startX, active.startY, fx, fy));
    }
  };

  const endDrag = (e: PointerEvent<HTMLDivElement>) => {
    drag.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>, box: EditorBox) => {
    if (e.target !== e.currentTarget) return;
    const delta = {
      ArrowLeft: [-NUDGE, 0],
      ArrowRight: [NUDGE, 0],
      ArrowUp: [0, -NUDGE],
      ArrowDown: [0, NUDGE],
    }[e.key];
    if (!delta) return;
    e.preventDefault();
    const [dx = 0, dy = 0] = delta;
    onChange(box.id, e.shiftKey ? resizeRegionBy(box.rect, dx, dy) : moveRegion(box.rect, dx, dy));
  };

  return (
    <div className="region-editor">
      <div className="region-frame">
        <div
          ref={stage}
          className="region-stage"
          style={cssVars({ "--aspect": aspectRatio })}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
        >
          <div className="region-clip" aria-hidden="true">
            {imageUrl && <img className="region-image" src={imageUrl} alt="" draggable={false} />}
            {!zoned && region && (
              <div
                className="region-dim"
                style={cssVars({ "--x": region.x, "--y": region.y, "--w": region.width, "--h": region.height })}
              />
            )}
          </div>
          {busy && <div className="region-busy">Updating preview</div>}
          {boxes.map((box) => {
            const selected = box.id === selectedId;
            const isRegion = box.id === REGION_BOX_ID;
            return (
              <div
                key={box.id}
                data-box={box.id}
                className={`region-rect${isRegion ? "" : " region-rect--zone"}${selected ? " is-selected" : ""}`}
                role="group"
                tabIndex={0}
                aria-label={isRegion ? regionLabel : `Zone for ${box.label}`}
                aria-describedby={hintId}
                style={cssVars({
                  "--x": box.rect.x,
                  "--y": box.rect.y,
                  "--w": box.rect.width,
                  "--h": box.rect.height,
                })}
                onFocus={() => onSelect(box.id)}
                onKeyDown={(e) => onKeyDown(e, box)}
              >
                {zoned && (
                  <span className="box-label">
                    <LiveSwatch colorIndex={box.colorIndex} />
                    <span className="box-name">{box.label}</span>
                  </span>
                )}
                {selected &&
                  HANDLES.map((handle) => (
                    <span key={handle} data-handle={handle} className={`region-handle region-handle--${handle}`} />
                  ))}
              </div>
            );
          })}
        </div>
      </div>
      <p className="region-hint" id={hintId}>
        {zoned
          ? "Drag a box to move it and drag the handles of the selected box to resize it. With a box focused, arrow keys move it and Shift with arrow keys resizes it. "
          : "Drag the frame to move it, drag its handles to resize, or drag on the picture to draw a new one. With the frame focused, arrow keys move it and Shift with arrow keys resizes it. "}
        Smallest size is {Math.round(REGION_MIN_SIZE * 100)}%.
      </p>
    </div>
  );
}
