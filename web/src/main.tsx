import { render } from "preact";
import { signal } from "@preact/signals";
import { App } from "./app";
import { QueueView } from "./components/QueueView";
import "./styles/terminal.css";
import "./styles/themes.css";

type Route = "app" | "queue";

function hashToRoute(): Route {
  return location.hash === "#/queue" ? "queue" : "app";
}

const route = signal<Route>(hashToRoute());

window.addEventListener("hashchange", () => {
  route.value = hashToRoute();
});

function Router() {
  return route.value === "queue" ? <QueueView /> : <App />;
}

render(<Router />, document.getElementById("app")!);
