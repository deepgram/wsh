import { useCallback, useRef } from "preact/hooks";

interface ResizeHandleProps {
  containerRef: { current: HTMLDivElement | null };
  onResize: (ratio: number) => void;
}

export function ResizeHandle({ containerRef, onResize }: ResizeHandleProps) {
  const dragging = useRef(false);

  const handlePointerDown = useCallback(
    (e: PointerEvent) => {
      e.preventDefault();
      dragging.current = true;
      const target = e.currentTarget as HTMLElement;
      target.setPointerCapture(e.pointerId);

      const handlePointerMove = (e: PointerEvent) => {
        if (!dragging.current || !containerRef.current) return;
        const rect = containerRef.current.getBoundingClientRect();
        const ratio = Math.max(
          0.2,
          Math.min(0.8, (e.clientX - rect.left) / rect.width),
        );
        onResize(ratio);
      };

      const handlePointerUp = () => {
        dragging.current = false;
        target.removeEventListener("pointermove", handlePointerMove);
        target.removeEventListener("pointerup", handlePointerUp);
      };

      target.addEventListener("pointermove", handlePointerMove);
      target.addEventListener("pointerup", handlePointerUp);
    },
    [containerRef, onResize],
  );

  return (
    <div class="tile-resize-handle" onPointerDown={handlePointerDown} />
  );
}
