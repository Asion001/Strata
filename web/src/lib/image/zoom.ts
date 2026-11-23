// Zoom and pan for the image viewer, as plain math: a view is a scale and
// a translation (in CSS pixels, from the centre of the stage), applied as
// `translate(x, y) scale(s)` with the origin at the image's centre.

export interface View { s: number; x: number; y: number }

export const MIN = 1;
export const MAX = 8;
/** The zoom a double tap or double click goes to. */
export const STEP = 2.5;

export const fit: View = { s: 1, x: 0, y: 0 };

/** Keep the image covering the stage: no panning past its edges. `w`/`h`
 *  are the size of the image as shown at scale 1. */
export function clamp(v: View, w: number, h: number): View {
  const s = Math.min(MAX, Math.max(MIN, v.s));
  const mx = ((s - 1) * w) / 2;
  const my = ((s - 1) * h) / 2;
  return { s, x: Math.min(mx, Math.max(-mx, v.x)), y: Math.min(my, Math.max(-my, v.y)) };
}

/** Scale to `s` keeping the point `(px, py)` (from the stage centre) where
 *  it is on screen. */
export function zoomAt(v: View, s: number, px: number, py: number): View {
  const k = s / v.s;
  return { s, x: px - k * (px - v.x), y: py - k * (py - v.y) };
}

/** A double tap: zoom in on the point, or back out to fit. */
export function toggle(v: View, px: number, py: number): View {
  return v.s > 1.01 ? fit : zoomAt(v, STEP, px, py);
}
