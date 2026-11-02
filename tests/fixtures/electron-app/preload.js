// The renderer's only way into the main process.
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("orders", {
  save: (order) => ipcRenderer.invoke("save-order", order),
  openSettings: () => ipcRenderer.invoke("open-settings"),
  electron: process.versions.electron,
});
