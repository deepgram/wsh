import { render } from "preact";
import { App } from "./app";
import "./styles/terminal.css";
import "./styles/themes.css";

// Desktop only: set --app-height from visualViewport for window resize.
// Mobile uses position:fixed + 100dvh in CSS instead.
function initViewportHeight() {
  if (window.matchMedia("(pointer: coarse)").matches) return;
  const vv = window.visualViewport;
  if (!vv) return;

  let timer: ReturnType<typeof setTimeout>;
  const apply = () => {
    document.documentElement.style.setProperty("--app-height", `${vv.height}px`);
  };
  apply();
  vv.addEventListener("resize", () => {
    clearTimeout(timer);
    timer = setTimeout(apply, 150);
  });
}

initViewportHeight();

render(<App />, document.getElementById("app")!);
