import { useRef, useCallback, useEffect, useState } from "preact/hooks";
import { sidebarWidth, sidebarCollapsed } from "../state/sessions";
import type { WshClient } from "../api/ws";
import { Sidebar } from "./Sidebar";
import { MainContent } from "./MainContent";
import { BottomSheet } from "./BottomSheet";

interface LayoutShellProps {
  client: WshClient;
}

type LayoutMode = "desktop" | "tablet" | "mobile";

export function LayoutShell({ client }: LayoutShellProps) {
  const [layoutMode, setLayoutMode] = useState<LayoutMode>("desktop");
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const dragging = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const collapsed = sidebarCollapsed.value;
  const width = sidebarWidth.value;

  // Detect layout mode
  useEffect(() => {
    const checkLayout = () => {
      if (window.innerWidth < 640) {
        setLayoutMode("mobile");
      } else if (window.innerWidth < 1024) {
        setLayoutMode("tablet");
      } else {
        setLayoutMode("desktop");
      }
    };
    checkLayout();
    window.addEventListener("resize", checkLayout);
    return () => window.removeEventListener("resize", checkLayout);
  }, []);

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

  if (layoutMode === "mobile") {
    return (
      <div class="layout-shell layout-mobile">
        <div class="layout-main">
          <MainContent client={client} />
        </div>
        <BottomSheet client={client} />
      </div>
    );
  }

  if (layoutMode === "tablet") {
    return (
      <div class="layout-shell layout-tablet">
        {sidebarOpen && (
          <>
            <div class="layout-overlay-backdrop" onClick={() => setSidebarOpen(false)} />
            <div class="layout-sidebar-overlay">
              <Sidebar client={client} collapsed={false} onToggleCollapse={() => setSidebarOpen(false)} />
            </div>
          </>
        )}
        {!sidebarOpen && (
          <button
            class="layout-tablet-menu-btn"
            onClick={() => setSidebarOpen(true)}
            title="Open sidebar"
          >
            &#9776;
          </button>
        )}
        <div class="layout-main">
          <MainContent client={client} />
        </div>
      </div>
    );
  }

  // Desktop mode (existing layout)
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
