import { useRef, useCallback, useEffect } from "preact/hooks";
import { sidebarWidth, sidebarCollapsed } from "../state/sessions";
import type { WshClient } from "../api/ws";
import { Sidebar } from "./Sidebar";
import { MainContent } from "./MainContent";

interface LayoutShellProps {
  client: WshClient;
}

export function LayoutShell({ client }: LayoutShellProps) {
  const dragging = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const collapsed = sidebarCollapsed.value;
  const width = sidebarWidth.value;

  const handleMouseDown = useCallback((e: MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!dragging.current || !containerRef.current) return;
      const rect = containerRef.current.getBoundingClientRect();
      const pct = ((e.clientX - rect.left) / rect.width) * 100;
      const clamped = Math.max(10, Math.min(30, pct));
      sidebarWidth.value = clamped;
      localStorage.setItem("wsh-sidebar-width", String(clamped));
    };
    const handleMouseUp = () => {
      if (dragging.current) {
        dragging.current = false;
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      }
    };
    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);
    return () => {
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
    };
  }, []);

  const toggleCollapse = useCallback(() => {
    sidebarCollapsed.value = !sidebarCollapsed.value;
    localStorage.setItem("wsh-sidebar-collapsed", String(sidebarCollapsed.value));
  }, []);

  return (
    <div class="layout-shell" ref={containerRef}>
      <div
        class={`layout-sidebar ${collapsed ? "collapsed" : ""}`}
        style={collapsed ? undefined : { width: `${width}%` }}
      >
        <Sidebar client={client} collapsed={collapsed} onToggleCollapse={toggleCollapse} />
      </div>
      {!collapsed && (
        <div class="layout-resize-handle" onMouseDown={handleMouseDown} />
      )}
      <div class="layout-main">
        <MainContent client={client} />
      </div>
    </div>
  );
}
