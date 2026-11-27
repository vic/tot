// Registers coi-sw.js and reloads once, so the page comes back cross-origin isolated.
if (!window.crossOriginIsolated && "serviceWorker" in navigator) {
  navigator.serviceWorker.register("coi-sw.js").then(() => {
    if (!navigator.serviceWorker.controller) location.reload();
  });
}
