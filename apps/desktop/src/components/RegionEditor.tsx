import { type KeyboardEvent, type PointerEvent, useId, useRef } from "react";
import { cssVars } from "../lib/format";
import type { NormRect } from "../lib/ipc";
import {
  HANDLES,
  type Handle,
  REGION_MIN_SIZE,
  moveRegion,
  regionFromCorners,
  resizeRegion,
  resizeRegionBy,
} from "../lib/region";

type Drag =
  | { kind: "move"; startX: number; startY: number; origin: NormRect }
  | { kind: "resize"; handle: Handle; startX: number; startY: number; origin: NormRect }
  | { kind: "draw"; startX: number; startY: number; origin: NormRect; moved: boolean };

/** Pointer travel (as a fraction of the stage) before a background drag draws a new region. */
const DRAW_THRESHOLD = 0.005;
const NUDGE = 0.01;

interface RegionEditorProps {
  region: NormRect;
  /** Image shown behind the region; the stage keeps `aspectRatio` either way. */
  imageUrl: string | null;
  aspectRatio: number;
  busy: boolean;
  onChange: (region: NormRect) => void;
}

export function RegionEditor({ region, imageUrl, aspectRatio, busy, onChange }: RegionEditorProps) {
  const stage = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const hintId = useId();

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
    const handle = target.closest<HTMLElement>("[data-handle]")?.dataset.handle as Handle | undefined;
    if (handle) {
      drag.current = { kind: "resize", handle, startX: fx, startY: fy, origin: region };
    } else if (target.closest("[data-region]")) {
      drag.current = { kind: "move", startX: fx, startY: fy, origin: region };
    } else {
      drag.current = { kind: "draw", startX: fx, startY: fy, origin: region, moved: false };
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
      onChange(moveRegion(active.origin, dx, dy));
    } else if (active.kind === "resize") {
      onChange(resizeRegion(active.origin, active.handle, dx, dy));
    } else {
      if (!active.moved && Math.hypot(dx, dy) < DRAW_THRESHOLD) return;
      active.moved = true;
      onChange(regionFromCorners(active.startX, active.startY, fx, fy));
    }
  };

  const endDrag = (e: PointerEvent<HTMLDivElement>) => {
    drag.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const delta = {
      ArrowLeft: [-NUDGE, 0],
      ArrowRight: [NUDGE, 0],
      ArrowUp: [0, -NUDGE],
      ArrowDown: [0, NUDGE],
    }[e.key];
    if (!delta) return;
    e.preventDefault();
    const [dx = 0, dy = 0] = delta;
    onChange(e.shiftKey ? resizeRegionBy(region, dx, dy) : moveRegion(region, dx, dy));
  };

  const rectVars = cssVars({ "--x": region.x, "--y": region.y, "--w": region.width, "--h": region.height });

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
            <div className="region-dim" style={rectVars} />
          </div>
          {busy && <div className="region-busy">Updating preview</div>}
          <div
            data-region
            className="region-rect"
            role="group"
            tabIndex={0}
            aria-label="Capture region"
            aria-describedby={hintId}
            style={rectVars}
            onKeyDown={onKeyDown}
          >
            {HANDLES.map((handle) => (
              <span key={handle} data-handle={handle} className={`region-handle region-handle--${handle}`} />
            ))}
          </div>
        </div>
      </div>
      <p className="region-hint" id={hintId}>
        Drag the frame to move it, drag its handles to resize, or drag on the picture to draw a new one. With the
        frame focused, arrow keys move it and Shift with arrow keys resizes it. Smallest size is{" "}
        {Math.round(REGION_MIN_SIZE * 100)}%.
      </p>
    </div>
  );
}
