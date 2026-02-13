import { render } from "preact";
import { App } from "./app";
import "./styles/terminal.css";
import "./styles/themes.css";

render(<App />, document.getElementById("app")!);
