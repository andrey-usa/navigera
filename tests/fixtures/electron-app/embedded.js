// The page inside the Notes window's <webview>.
document.getElementById("act").addEventListener("click", () => {
  document.getElementById("out").textContent = "Clicked inside the webview";
});
