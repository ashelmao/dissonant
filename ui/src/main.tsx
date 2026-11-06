import { useStore } from "./store";
import { usePresence } from "./collab";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { PluginWindow } from "./components/PluginWindow";
import "@fontsource/ibm-plex-sans-condensed/400.css";
import "@fontsource/ibm-plex-sans-condensed/500.css";
import "@fontsource/ibm-plex-sans-condensed/600.css";
import "@fontsource/ibm-plex-sans-condensed/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/700.css";
import "./styles.css";

// `?plugin=<trackId>` marks the plugin editor window. It is the same bundle, deliberately: the
// editor needs the same bridge and the same stylesheet, and a second entry point would mean two
// builds to keep in step.
const pluginTrack = new URLSearchParams(window.location.search).get("plugin");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{pluginTrack ? <PluginWindow trackId={pluginTrack} /> : <App />}</React.StrictMode>,
);

// For driving the app from a test, in dev builds only.
if (import.meta.env.DEV) Object.assign(window, { __dissonant: { useStore, usePresence } });
