import { clamp } from "./format";
import type { NormRect } from "./ipc";

export const REGION_MIN_SIZE = 0.01;

export type Handle = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";
export const HANDLES: readonly Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

const round = (value: number): number => Math.round(value * 1000) / 1000;

/** Keeps a rectangle inside the unit square with at least the minimum size. */
export function clampRegion(region: NormRect): NormRect {
  const width = clamp(region.width, REGION_MIN_SIZE, 1);
  const height = clamp(region.height, REGION_MIN_SIZE, 1);
  return {
    x: round(clamp(region.x, 0, 1 - width)),
    y: round(clamp(region.y, 0, 1 - height)),
    width: round(width),
    height: round(height),
  };
}

export function moveRegion(origin: NormRect, dx: number, dy: number): NormRect {
  return clampRegion({ ...origin, x: origin.x + dx, y: origin.y + dy });
}

/** Moves the edges named by the handle by the given fractional delta. */
export function resizeRegion(origin: NormRect, handle: Handle, dx: number, dy: number): NormRect {
  let left = origin.x;
  let top = origin.y;
  let right = origin.x + origin.width;
  let bottom = origin.y + origin.height;
  if (handle.includes("w")) left = clamp(left + dx, 0, right - REGION_MIN_SIZE);
  if (handle.includes("e")) right = clamp(right + dx, left + REGION_MIN_SIZE, 1);
  if (handle.includes("n")) top = clamp(top + dy, 0, bottom - REGION_MIN_SIZE);
  if (handle.includes("s")) bottom = clamp(bottom + dy, top + REGION_MIN_SIZE, 1);
  return clampRegion({ x: left, y: top, width: right - left, height: bottom - top });
}

/** The rectangle spanned by two corner points, in unit-square fractions. */
export function regionFromCorners(ax: number, ay: number, bx: number, by: number): NormRect {
  return clampRegion({
    x: Math.min(ax, bx),
    y: Math.min(ay, by),
    width: Math.abs(bx - ax),
    height: Math.abs(by - ay),
  });
}

export const sameRegion = (a: NormRect, b: NormRect): boolean =>
  Math.abs(a.x - b.x) < 0.001 &&
  Math.abs(a.y - b.y) < 0.001 &&
  Math.abs(a.width - b.width) < 0.001 &&
  Math.abs(a.height - b.height) < 0.001;

export const REGION_PRESETS: readonly { label: string; region: NormRect }[] = [
  { label: "Full screen", region: { x: 0, y: 0, width: 1, height: 1 } },
  { label: "Centre 50%", region: { x: 0.25, y: 0.25, width: 0.5, height: 0.5 } },
  { label: "Top strip", region: { x: 0, y: 0, width: 1, height: 0.1 } },
  { label: "Bottom strip", region: { x: 0, y: 0.9, width: 1, height: 0.1 } },
  { label: "Left strip", region: { x: 0, y: 0, width: 0.1, height: 1 } },
  { label: "Right strip", region: { x: 0.9, y: 0, width: 0.1, height: 1 } },
];
