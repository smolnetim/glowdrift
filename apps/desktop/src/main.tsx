import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { startBackendSync } from "./state/init";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root element");

startBackendSync();

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
