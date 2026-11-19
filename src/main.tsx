import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";
import { IS_LINUX } from "./format";

if (IS_LINUX) document.documentElement.classList.add("linux");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
