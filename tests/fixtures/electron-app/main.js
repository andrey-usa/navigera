// The main process: one window with an order form, saved through IPC, and
// a Settings window the form opens. `--pid-file=<path>` writes this
// process's id, so a test can check the app is gone after `quit`.
// `--helper-window` first opens a hidden window (as apps do for a dropdown
// or a tray menu); `--late-ui` mounts the form a second after the page
// loads (as an app does that boots its UI).
const { app, BrowserWindow, ipcMain } = require("electron");
const fs = require("fs");
const path = require("path");

const pidFile = process.argv.find((arg) => arg.startsWith("--pid-file="));
if (pidFile) {
  fs.writeFileSync(pidFile.slice("--pid-file=".length), String(process.pid));
}

const preload = path.join(__dirname, "preload.js");
let settings = null;

const helperWindow = process.argv.includes("--helper-window");
const lateUi = process.argv.includes("--late-ui");

function openOrders() {
  if (helperWindow) {
    const helper = new BrowserWindow({ show: false, width: 300, height: 200 });
    helper.loadFile(path.join(__dirname, "helper.html"));
  }
  const win = new BrowserWindow({ width: 900, height: 700, webPreferences: { preload } });
  win.loadFile(path.join(__dirname, "index.html"), lateUi ? { query: { late: "1" } } : undefined);
}

ipcMain.handle("save-order", (_event, order) => {
  const customer = String(order.customer || "").trim();
  const quantity = Number(order.quantity);
  if (!customer) {
    return { ok: false, error: "Customer is required" };
  }
  if (!Number.isInteger(quantity) || quantity < 1) {
    return { ok: false, error: "Quantity must be a whole number" };
  }
  return { ok: true, id: `A-${1000 + quantity}`, customer };
});

ipcMain.handle("open-settings", () => {
  if (settings && !settings.isDestroyed()) {
    settings.focus();
    return;
  }
  settings = new BrowserWindow({ width: 500, height: 400, webPreferences: { preload } });
  settings.loadFile(path.join(__dirname, "settings.html"));
});

app.whenReady().then(openOrders);
app.on("window-all-closed", () => app.quit());
