import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

// Surface any mount-time error into the DOM — a blank webview otherwise hides it.
let app: ReturnType<typeof mount> | undefined;
try {
  app = mount(App, { target: document.getElementById("app")! });
} catch (e) {
  document.body.innerHTML = `<pre style="color:#ff6b6b;padding:20px;white-space:pre-wrap;font:13px monospace">${String(
    (e as Error)?.stack ?? e,
  )}</pre>`;
}

export default app;
