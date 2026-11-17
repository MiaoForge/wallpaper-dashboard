"use strict";

window.initConfiguredDock = function () {
  const docks = [document.getElementById("dockApp"), document.getElementById("dockGame"), document.getElementById("dock")];
  const title = document.getElementById("dockTitle");
  const endpoint = "http://127.0.0.1:8766/shortcuts/dock";
  let signature;
  const request = async (action, id) => {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 4500);
    try {
      const response = await fetch(endpoint, {
        method: "POST",
        headers: { "Content-Type": "text/plain" },
        body: JSON.stringify({ action, id, key: window.wallpaperShortcutKey }),
        cache: "no-store",
        signal: controller.signal,
      });
      if (!response.ok) throw new Error("HTTP " + response.status);
      return await response.json();
    } finally { clearTimeout(timeout); }
  };
  function render({ apps = [], games = [] } = {}) {
    const next = JSON.stringify([apps, games]);
    if (next === signature) return;
    signature = next;
    const button = item => {
      const element = document.createElement("button");
      element.type = "button";
      element.className = "configured-shortcut";
      element.title = item.name;
      element.setAttribute("aria-label", item.name);
      const addFallback = () => {
        if (item.icon) {
          const image = document.createElement("img");
          image.src = item.icon;
          image.alt = "";
          element.replaceChildren(image);
        } else {
          const tile = document.createElement("span");
          tile.className = "shortcut-tile";
          tile.textContent = item.kind === "game" ? item.name : Array.from(item.name.trim()).slice(0, 2).join("").toUpperCase();
          element.replaceChildren(tile);
        }
      };
      if (item.artwork) {
        const image = document.createElement("img");
        image.className = "game-art";
        image.src = new URL(item.artwork, endpoint).href;
        image.alt = "";
        image.addEventListener("error", addFallback, {once:true});
        element.append(image);
      } else {
        addFallback();
      }
      element.addEventListener("click", async event => {
        event.preventDefault();
        event.stopPropagation();
        if (element.disabled) return;
        element.disabled = true;
        try { await request("launch", item.id); }
        catch { element.title = item.name + " · 启动失败"; }
        finally { element.disabled = false; }
      });
      return element;
    };
    const groups = [apps, games, apps.slice(0, 4)];
    docks.forEach((dock, index) => {
      dock.replaceChildren(...groups[index].map(button));
      dock.hidden = groups[index].length === 0;
    });
    title.hidden = games.length === 0;
  }
  render();
  if (typeof window.wallpaperShortcutKey !== "string") return;
  async function refresh() {
    try { render(await request("list")); }
    catch { render(); }
    finally { setTimeout(refresh, 3500); }
  }
  refresh();
};
