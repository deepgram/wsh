import { render } from "preact";
import { useState, useEffect } from "preact/hooks";
import { App } from "./app";
import { QueueView } from "./components/QueueView";
import "./styles/terminal.css";
import "./styles/themes.css";

function Router() {
  const [route, setRoute] = useState(location.hash);

  useEffect(() => {
    const onHash = () => setRoute(location.hash);
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  if (route === "#/queue") {
    return <QueueView />;
  }

  return <App />;
}

render(<Router />, document.getElementById("app")!);
