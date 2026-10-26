import { useEffect, useRef } from "react";
import { getBridge } from "../../bridge";

const FRAME_MS = 33;

/**
 * The preview channel's output, live: polls `previewScope()` at ~30 fps on animation frames
 * (so it idles when the window is hidden) and draws the zero-cross-aligned window.
 */
export function Scope({ width = 240, height = 84 }: { width?: number; height?: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    ctx.scale(dpr, dpr);
    const css = getComputedStyle(canvas);
    const stroke = css.getPropertyValue("--accent").trim() || "#b48cff";
    const grid = css.getPropertyValue("--line-1").trim() || "#26262d";
    let samples: number[] = [];
    let raf = 0;
    let last = 0;
    let inflight = false;
    let alive = true;
    const draw = () => {
      ctx.clearRect(0, 0, width, height);
      ctx.strokeStyle = grid;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, height / 2 + 0.5);
      ctx.lineTo(width, height / 2 + 0.5);
      ctx.stroke();
      if (samples.length < 2) return;
      ctx.strokeStyle = stroke;
      ctx.lineWidth = 1.4;
      ctx.beginPath();
      const n = samples.length;
      for (let i = 0; i < n; i++) {
        const xx = (i / (n - 1)) * width;
        const yy = height / 2 - Math.max(-1, Math.min(1, samples[i])) * (height / 2 - 3);
        if (i) ctx.lineTo(xx, yy);
        else ctx.moveTo(xx, yy);
      }
      ctx.stroke();
    };
    const tick = (now: number) => {
      raf = requestAnimationFrame(tick);
      if (inflight || now - last < FRAME_MS || document.hidden) return;
      last = now;
      inflight = true;
      void getBridge()
        .then((b) => b.previewScope())
        .then((s) => {
          if (!alive) return;
          samples = s;
          draw();
        })
        .catch(() => {})
        .finally(() => {
          inflight = false;
        });
    };
    draw();
    raf = requestAnimationFrame(tick);
    return () => {
      alive = false;
      cancelAnimationFrame(raf);
    };
  }, [width, height]);
  return <canvas ref={ref} className="sp-scope" style={{ width, height }} aria-label="scope: the sound you're playing" role="img" />;
}
