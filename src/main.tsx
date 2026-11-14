import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { Launcher } from "./features/Launcher";
import { parseView } from "./lib/view";

// One Vite entry serves every window; the URL query picks the view.
const { view } = parseView(window.location.search);

// Only the widget is a transparent window; the others need an opaque page.
document.documentElement.classList.add(`view-${view}`);
document.body.classList.add(`view-${view}`);

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>{view === "launcher" ? <Launcher /> : <App />}</StrictMode>,
  );
}
