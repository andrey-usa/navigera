// Mounts the order form (a second after load with ?late=1) and wires it.
function mount() {
  document.body.prepend(document.getElementById("ui").content.cloneNode(true));
  const status = document.getElementById("status");
  document.getElementById("runtime").textContent = `Electron ${window.orders.electron}`;

  document.getElementById("order").addEventListener("submit", async (event) => {
    event.preventDefault();
    const result = await window.orders.save({
      customer: document.getElementById("customer").value,
      quantity: document.getElementById("quantity").value,
    });
    status.textContent = result.ok ? `Saved ${result.id} for ${result.customer}` : result.error;
  });

  document.getElementById("open-settings").addEventListener("click", () => window.orders.openSettings());
}

if (new URLSearchParams(location.search).get("late") === "1") {
  setTimeout(mount, 1000);
} else {
  mount();
}
